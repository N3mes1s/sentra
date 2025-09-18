//! Core library for Sentra.  This module wires together the plugin
//! pipeline, request/response structures and HTTP handlers.  It
//! deliberately avoids any dependencies beyond those required by the
//! application to remain lightweight and easy to embed.

mod config;
pub mod plugins;
pub mod util;

pub use config::AppConfig;

use axum::extract::{
    rejection::{BytesRejection, FailedToBufferBody, JsonRejection},
    DefaultBodyLimit, State,
};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::{routing::post, Json, Router};
// WebSocket and broadcast telemetry removed for production simplification.
use flate2::write::GzEncoder;
use flate2::Compression;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};
use std::time::Instant;

use crate::plugins::{PluginConfig, PluginPipeline};
use crate::util::EvalContext;

pub use crate::plugins::policy_pack::PolicyRule;
pub use crate::util::{Deadline, Precomputed};

/// Structures representing the payload delivered by Copilot Studio.  Only
/// fields necessary for evaluation are captured here; unknown fields are
/// ignored.  See the official documentation for the complete schema.

#[derive(Debug, Deserialize, Serialize, Default, Clone)]
pub struct ChatItem {
    pub id: Option<String>,
    pub role: Option<String>,
    pub content: Option<String>,
    pub timestamp: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Default, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PrevToolOutput {
    pub tool_id: Option<String>,
    pub tool_name: Option<String>,
    pub outputs: Option<serde_json::Value>,
    pub timestamp: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Default, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PlannerContext {
    pub user_message: Option<String>,
    pub thought: Option<String>,
    pub chat_history: Option<Vec<serde_json::Value>>, // we use Value for flexibility
    pub previous_tool_outputs: Option<Vec<PrevToolOutput>>,
}

#[derive(Debug, Deserialize, Serialize, Default, Clone)]
pub struct ToolParam {
    pub name: String,
    pub description: Option<String>,
    #[serde(default, rename = "type")]
    pub param_type: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, Serialize, Default, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ToolDefinition {
    pub id: Option<String>,
    #[serde(rename = "type")]
    pub def_type: Option<String>,
    pub name: Option<String>,
    pub description: Option<String>,
    #[serde(default)]
    pub input_parameters: Vec<ToolParam>,
    #[serde(default)]
    pub output_parameters: Vec<ToolParam>,
}

#[derive(Debug, Deserialize, Serialize, Default, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ConversationAgent {
    pub id: Option<String>,
    pub tenant_id: Option<String>,
    pub environment_id: Option<String>,
    pub is_published: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize, Default, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ConversationUser {
    pub id: Option<String>,
    pub tenant_id: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Default, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ConversationTrigger {
    pub id: Option<String>,
    pub schema_name: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Default, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ConversationMetadata {
    pub agent: Option<ConversationAgent>,
    pub user: Option<ConversationUser>,
    pub trigger: Option<ConversationTrigger>,
    pub conversation_id: Option<String>,
    pub plan_id: Option<String>,
    pub plan_step_id: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Default, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AnalyzeRequest {
    pub planner_context: PlannerContext,
    pub tool_definition: ToolDefinition,
    #[serde(default)]
    pub input_values: serde_json::Map<String, serde_json::Value>,
    pub conversation_metadata: Option<ConversationMetadata>,
}

impl AnalyzeRequest {
    /// Validate required fields according to the Microsoft External Security Webhooks spec.
    /// Returns a vector of missing field descriptions (empty if valid).
    fn missing_required_fields(&self) -> Vec<&'static str> {
        let mut missing = Vec::new();
        // plannerContext.userMessage required & must be non-empty
        match self
            .planner_context
            .user_message
            .as_ref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
        {
            Some(_) => {}
            None => missing.push("plannerContext.userMessage"),
        }
        // toolDefinition.name required & non-empty
        match self
            .tool_definition
            .name
            .as_ref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
        {
            Some(_) => {}
            None => missing.push("toolDefinition.name"),
        }
        missing
    }
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AnalyzeResponse {
    pub block_action: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Name of the plugin that produced the blocking decision (present only when block_action=true)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked_by: Option<String>,
    /// Structured diagnostics object (plugin-specific details). For a benign response this is null.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<serde_json::Value>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ErrorResponse {
    pub error_code: i32,
    pub message: String,
    pub http_status: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<serde_json::Value>,
}

/// Internal application state shared across handlers.  Contains the
/// preconstructed plugin pipeline, evaluation flags and parsed configuration.
#[derive(Clone)]
pub struct AppState {
    pub pipeline: PluginPipeline,
    pub plugin_config: PluginConfig,
    pub log_file: Option<String>,
    pub allowed_tokens: Option<HashSet<String>>, // strict auth allowlist
    /// Maximum accepted raw request body size in bytes (None => unlimited)
    pub max_request_bytes: Option<usize>,
    /// Total plugin evaluation budget in milliseconds (default 900ms)
    pub plugin_budget_ms: u64,
    /// Per-plugin warning threshold in ms (log if exceeded)
    pub plugin_warn_ms: u64,
    /// Audit only mode (never block, still evaluate and log would-be blocks)
    pub audit_only: bool,
    /// Separate audit log file (optional). If unset falls back to LOG_FILE.
    pub audit_log_file: Option<String>,
    pub telemetry: TelemetrySink,
    // Metrics counters
    pub metric_requests_total: Arc<AtomicU64>,
    pub metric_blocks_total: Arc<AtomicU64>,
    pub metric_audit_suppressed_total: Arc<AtomicU64>,
    // Histogram buckets (fixed) for request latency in ms (upper bounds) and counts
    pub hist_buckets: Arc<Vec<u64>>,      // bucket upper bounds
    pub hist_counts: Arc<Vec<AtomicU64>>, // same length as hist_buckets
    pub hist_sum_ms: Arc<AtomicU64>,      // sum of observed latencies (ms)
    pub hist_count: Arc<AtomicU64>,       // total observations
    // Per-plugin metrics (sum ms, count, block count)
    pub plugin_metric_indices: Arc<std::collections::HashMap<String, usize>>,
    pub plugin_metrics: Arc<Vec<PluginMetrics>>, // index aligned with plugin order
    // Process start time (epoch secs) and instant for uptime computation
    pub process_start_epoch: f64,
    pub process_start_instant: Instant,
}

pub struct PluginMetrics {
    pub eval_sum_ms: AtomicU64,
    pub eval_count: AtomicU64,
    pub block_count: AtomicU64,
    // Per-plugin latency histogram: counts aligned with AppState.hist_buckets
    pub hist_counts: Vec<AtomicU64>,
    pub hist_sum_ms: AtomicU64,
    pub hist_count: AtomicU64,
}

/// Simple size-based rotating writer (single backup file <path>.1 kept).
pub struct RotatingWriter {
    path: PathBuf,
    file: std::fs::File,
    max_bytes: Option<u64>,
    keep: usize,
    compress: bool,
}

impl RotatingWriter {
    pub fn open(
        path: &str,
        max_bytes: Option<u64>,
        keep: usize,
        compress: bool,
    ) -> std::io::Result<Self> {
        let file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        Ok(Self {
            path: PathBuf::from(path),
            file,
            max_bytes,
            keep,
            compress,
        })
    }
    fn check_rotate(&mut self) {
        if let Some(limit) = self.max_bytes {
            if self.exceeds_limit(limit) {
                self.rotate_backups();
                self.compress_latest_backup();
                self.reopen_current();
            }
        }
    }
    fn write_line_result(&mut self, line: &str) -> std::io::Result<()> {
        self.check_rotate();
        writeln!(self.file, "{}", line)
    }
    fn current_size(&self) -> Option<u64> {
        self.path.metadata().ok().map(|m| m.len())
    }

    fn exceeds_limit(&self, limit: u64) -> bool {
        self.path
            .metadata()
            .map(|meta| meta.len() >= limit)
            .unwrap_or(false)
    }

    fn rotate_backups(&self) {
        if self.keep == 0 {
            return;
        }
        for idx in (1..=self.keep).rev() {
            let old = if idx == 1 {
                self.path.clone()
            } else {
                self.path.with_extension(format!("{}", idx - 1))
            };
            if old.exists() {
                let new = self.path.with_extension(format!("{}", idx));
                let _ = fs::rename(&old, &new);
            }
        }
    }

    fn compress_latest_backup(&self) {
        if !self.compress || self.keep == 0 {
            return;
        }
        let rotated = self.path.with_extension("1");
        if let Ok(data) = fs::read(&rotated) {
            let gz_path = rotated.with_extension("1.gz");
            let mut gz = GzEncoder::new(Vec::new(), Compression::default());
            if gz.write_all(&data).is_ok() {
                if let Ok(buf) = gz.finish() {
                    let _ = fs::write(&gz_path, buf);
                    let _ = fs::remove_file(&rotated);
                }
            }
        }
    }

    fn reopen_current(&mut self) {
        if let Ok(newf) = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&self.path)
        {
            self.file = newf;
        }
    }
}

#[derive(Clone)]
pub struct TelemetrySink {
    telemetry_writer: Option<Arc<Mutex<RotatingWriter>>>,
    audit_writer: Option<Arc<Mutex<RotatingWriter>>>,
    log_stdout: bool,
    log_sample_n: Option<u64>,
    log_sample_counter: Arc<AtomicU64>,
    metric_lines_total: Arc<AtomicU64>,
    metric_write_errors_total: Arc<AtomicU64>,
    log_file_size_bytes: Arc<AtomicU64>,
}

pub struct TelemetryLogFields<'a> {
    pub block_action: bool,
    pub reason_code: Option<i32>,
    pub blocked_by: Option<&'a str>,
    pub latency_ms: u128,
    pub audit_suppressed: bool,
    pub plugin_count: usize,
}

pub struct AuditLogFields<'a> {
    pub would_block: bool,
    pub reason_code: Option<i32>,
    pub blocked_by: Option<&'a str>,
    pub plugin_count: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TelemetryKind {
    Event,
    Audit,
}

impl TelemetrySink {
    pub fn new(
        telemetry_writer: Option<Arc<Mutex<RotatingWriter>>>,
        audit_writer: Option<Arc<Mutex<RotatingWriter>>>,
        log_stdout: bool,
        log_sample_n: Option<u64>,
        metric_lines_total: Arc<AtomicU64>,
        metric_write_errors_total: Arc<AtomicU64>,
        log_file_size_bytes: Arc<AtomicU64>,
    ) -> Self {
        Self {
            telemetry_writer,
            audit_writer,
            log_stdout,
            log_sample_n,
            log_sample_counter: Arc::new(AtomicU64::new(0)),
            metric_lines_total,
            metric_write_errors_total,
            log_file_size_bytes,
        }
    }

    pub fn emit_event(&self, payload: &serde_json::Value, log: &TelemetryLogFields<'_>) {
        let writer = self.telemetry_writer.as_ref();
        let wrote = self.write_line(payload, writer, TelemetryKind::Event);
        if (wrote || writer.is_none()) && self.should_log_stdout() {
            tracing::info!(
                target = "telemetry",
                event = "telemetry",
                blockAction = log.block_action,
                reasonCode = ?log.reason_code,
                blockedBy = ?log.blocked_by,
                latencyMs = log.latency_ms,
                auditSuppressed = log.audit_suppressed,
                pluginCount = log.plugin_count
            );
        }
    }

    pub fn emit_audit(&self, payload: &serde_json::Value, log: &AuditLogFields<'_>) {
        let writer = self
            .audit_writer
            .as_ref()
            .or(self.telemetry_writer.as_ref());
        let wrote = self.write_line(payload, writer, TelemetryKind::Audit);
        if !wrote && writer.is_none() {
            tracing::warn!("Audit record dropped: no audit or telemetry writer configured");
        }
        if (wrote || writer.is_none()) && self.should_log_stdout() {
            tracing::info!(
                target = "telemetry",
                event = "audit",
                audit = true,
                wouldBlock = log.would_block,
                reasonCode = ?log.reason_code,
                blockedBy = ?log.blocked_by,
                pluginCount = log.plugin_count
            );
        }
    }

    pub fn lines_total(&self) -> &Arc<AtomicU64> {
        &self.metric_lines_total
    }

    pub fn write_errors_total(&self) -> &Arc<AtomicU64> {
        &self.metric_write_errors_total
    }

    pub fn log_file_size_bytes(&self) -> &Arc<AtomicU64> {
        &self.log_file_size_bytes
    }

    fn write_line(
        &self,
        payload: &serde_json::Value,
        writer: Option<&Arc<Mutex<RotatingWriter>>>,
        kind: TelemetryKind,
    ) -> bool {
        let line = payload.to_string();
        if let Some(target) = writer {
            if let Ok(mut guard) = target.lock() {
                match guard.write_line_result(&line) {
                    Ok(_) => {
                        self.metric_lines_total.fetch_add(1, Ordering::Relaxed);
                        if let Some(sz) = guard.current_size() {
                            self.log_file_size_bytes.store(sz, Ordering::Relaxed);
                        }
                        return true;
                    }
                    Err(e) => {
                        match kind {
                            TelemetryKind::Event => {
                                tracing::warn!(error=%e, "Failed to write telemetry line");
                            }
                            TelemetryKind::Audit => {
                                tracing::warn!(error=%e, "Failed to write audit line");
                            }
                        }
                        self.metric_write_errors_total
                            .fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }
        false
    }

    fn should_log_stdout(&self) -> bool {
        if !self.log_stdout {
            return false;
        }
        if let Some(n) = self.log_sample_n {
            let prev = self.log_sample_counter.fetch_add(1, Ordering::Relaxed);
            prev % n == 0
        } else {
            true
        }
    }
}

/// Build state from environment variables.  This function reads the
/// following variables:
///
/// * `SENTRA_PLUGIN_CONFIG` (optional) – path to a JSON configuration file.
/// * `SENTRA_PLUGINS` (optional) – comma separated list of plugin names in order.
/// * `LOG_FILE` (optional) – path to append newline‑delimited JSON telemetry.
pub async fn build_state_from_env() -> Result<AppState, Box<dyn std::error::Error>> {
    let config = AppConfig::from_env().map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
    let AppConfig {
        plugin_config,
        plugin_order,
        log_file,
        audit_log_file,
        allowed_tokens,
        rotation,
        log_stdout,
        max_request_bytes,
        plugin_budget_ms,
        plugin_warn_ms,
        audit_only,
        log_sample_n,
    } = config;

    let pipeline = PluginPipeline::new(&plugin_order, &plugin_config);

    // Fixed histogram bucket upper bounds in ms (inclusive style for counting):
    let buckets: Vec<u64> = vec![1, 2, 5, 10, 20, 50, 100, 200, 500, 1000, 2000];

    // Pre-open writers (if configured). We do not create a default file implicitly; we warn if absent.
    let telemetry_writer = match log_file.as_deref() {
        Some(path) => {
            match RotatingWriter::open(path, rotation.max_bytes, rotation.keep, rotation.compress) {
                Ok(f) => Some(Arc::new(Mutex::new(f))),
                Err(e) => {
                    tracing::warn!(path=%path, error=%e, "Failed to open LOG_FILE for telemetry; telemetry disabled");
                    None
                }
            }
        }
        None => {
            tracing::warn!("Telemetry disabled: LOG_FILE not set");
            None
        }
    };
    let audit_writer = match audit_log_file.as_deref() {
        Some(path) => {
            match RotatingWriter::open(path, rotation.max_bytes, rotation.keep, rotation.compress) {
                Ok(f) => Some(Arc::new(Mutex::new(f))),
                Err(e) => {
                    tracing::warn!(path=%path, error=%e, "Failed to open AUDIT_LOG_FILE; audit records will fall back or be disabled");
                    None
                }
            }
        }
        None => None,
    };

    let metric_requests_total = Arc::new(AtomicU64::new(0));
    let metric_blocks_total = Arc::new(AtomicU64::new(0));
    let metric_audit_suppressed_total = Arc::new(AtomicU64::new(0));
    let metric_lines_total = Arc::new(AtomicU64::new(0));
    let metric_write_errors_total = Arc::new(AtomicU64::new(0));
    let log_file_size_bytes = Arc::new(AtomicU64::new(0));

    let telemetry = TelemetrySink::new(
        telemetry_writer,
        audit_writer,
        log_stdout,
        log_sample_n,
        metric_lines_total.clone(),
        metric_write_errors_total.clone(),
        log_file_size_bytes.clone(),
    );

    // Prepare per-plugin metrics structures based on declared order.
    let mut index_map = std::collections::HashMap::new();
    let mut plugin_metrics_vec = Vec::new();
    for (i, name) in plugin_order.iter().enumerate() {
        index_map.insert(name.clone(), i);
        plugin_metrics_vec.push(PluginMetrics {
            eval_sum_ms: AtomicU64::new(0),
            eval_count: AtomicU64::new(0),
            block_count: AtomicU64::new(0),
            hist_counts: buckets.iter().map(|_| AtomicU64::new(0)).collect(),
            hist_sum_ms: AtomicU64::new(0),
            hist_count: AtomicU64::new(0),
        });
    }

    let start_time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();

    Ok(AppState {
        pipeline,
        plugin_config,
        log_file,
        allowed_tokens,
        max_request_bytes,
        plugin_budget_ms,
        plugin_warn_ms,
        audit_only,
        audit_log_file,
        telemetry,
        metric_requests_total,
        metric_blocks_total,
        metric_audit_suppressed_total,
        hist_buckets: Arc::new(buckets.clone()),
        hist_counts: Arc::new(buckets.iter().map(|_| AtomicU64::new(0)).collect()),
        hist_sum_ms: Arc::new(AtomicU64::new(0)),
        hist_count: Arc::new(AtomicU64::new(0)),
        plugin_metric_indices: Arc::new(index_map),
        plugin_metrics: Arc::new(plugin_metrics_vec),
        process_start_epoch: start_time.as_secs_f64(),
        process_start_instant: Instant::now(),
    })
}

/// Build the Axum router and attach handlers.  The router holds a copy
/// of the `AppState` for each invocation.
pub fn app(state: AppState) -> Router {
    let max_request_bytes = state.max_request_bytes;

    let router = Router::new()
        .route("/validate", post(validate_handler))
        .route("/analyze-tool-execution", post(analyze_handler))
        .route("/healthz", axum::routing::get(healthz_handler))
        .route("/metrics", axum::routing::get(metrics_handler));

    let router = if let Some(limit) = max_request_bytes {
        router.layer(DefaultBodyLimit::max(limit))
    } else {
        router
    };

    router.with_state(state)
}

/// Query parameters/// Query parameters for versioning.  Only `api-version` matters.
#[derive(Debug, Deserialize)]
struct VersionQuery {
    #[serde(rename = "api-version")]
    api_version: Option<String>,
}

// Constant API version supported by this implementation.
const API_VERSION: &str = "2025-05-01";

fn respond_with_error(err: ErrorResponse) -> axum::response::Response {
    let status = StatusCode::from_u16(err.http_status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, Json(err)).into_response()
}

fn ensure_api_version(params: &VersionQuery) -> Result<(), ErrorResponse> {
    match params.api_version.as_deref() {
        None => Err(ErrorResponse {
            error_code: 4000,
            message: format!("Missing api-version (expected {})", API_VERSION),
            http_status: 400,
            diagnostics: None,
        }),
        Some(v) if v != API_VERSION => {
            tracing::info!(client_api_version=%v, supported=API_VERSION, "Proceeding with forward-compatible api-version");
            Ok(())
        }
        _ => Ok(()),
    }
}

fn authorization_error() -> ErrorResponse {
    ErrorResponse {
        error_code: 2001,
        message: "Unauthorized".into(),
        http_status: 401,
        diagnostics: None,
    }
}

fn extract_bearer_token(headers: &HeaderMap) -> Result<String, ErrorResponse> {
    let raw = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(authorization_error)?;

    if raw.len() < 7 || !raw[..6].eq_ignore_ascii_case("bearer") {
        return Err(authorization_error());
    }
    let token = raw[6..].trim();
    if token.is_empty() {
        return Err(authorization_error());
    }
    Ok(token.to_string())
}

fn ensure_authorized(
    headers: &HeaderMap,
    allowed_tokens: Option<&HashSet<String>>,
) -> Result<(), ErrorResponse> {
    let token = extract_bearer_token(headers)?;
    if let Some(tokens) = allowed_tokens {
        if !tokens.contains(&token) {
            return Err(authorization_error());
        }
    }
    Ok(())
}

/// Handler for the `/validate` endpoint.  Ensures the correct API version is
/// provided and that an authorized bearer token accompanies the request.
async fn validate_handler(
    state: State<AppState>,
    axum::extract::Query(params): axum::extract::Query<VersionQuery>,
    headers: HeaderMap,
) -> axum::response::Response {
    if let Err(err) = ensure_api_version(&params) {
        return respond_with_error(err);
    }
    if let Err(err) = ensure_authorized(&headers, state.allowed_tokens.as_ref()) {
        return respond_with_error(err);
    }
    let ok = serde_json::json!({ "isSuccessful": true, "status": "OK" });
    (StatusCode::OK, Json(ok)).into_response()
}

/// Handler for `/analyze-tool-execution`.  Parses the request, constructs
/// evaluation context and invokes the plugin pipeline.  Responds with an
/// `AnalyzeResponse` on success or an `ErrorResponse` if validation fails.
async fn analyze_handler(
    state: State<AppState>,
    axum::extract::Query(params): axum::extract::Query<VersionQuery>,
    headers: HeaderMap,
    payload: Result<Json<AnalyzeRequest>, JsonRejection>,
) -> axum::response::Response {
    // Size guard: rely on Content-Length header if provided.
    if let Some(limit) = state.max_request_bytes {
        if let Some(len_header) = headers.get("content-length").and_then(|v| v.to_str().ok()) {
            if let Ok(clen) = len_header.parse::<usize>() {
                if clen > limit {
                    let err = ErrorResponse {
                        error_code: 4001,
                        message: format!(
                            "Request too large ({} bytes > limit {} bytes)",
                            clen, limit
                        ),
                        http_status: 413,
                        diagnostics: None,
                    };
                    return respond_with_error(err);
                }
            }
        }
    }
    if let Err(err) = ensure_api_version(&params) {
        return respond_with_error(err);
    }
    if let Err(err) = ensure_authorized(&headers, state.allowed_tokens.as_ref()) {
        return respond_with_error(err);
    }

    let payload = match payload {
        Ok(Json(inner)) => inner,
        Err(rejection) => {
            return handle_json_rejection(&state, rejection);
        }
    };

    // Validate required payload fields (spec compliance)
    let missing = payload.missing_required_fields();
    if !missing.is_empty() {
        let all_missing = missing.join(", ");
        let err = ErrorResponse {
            error_code: 4002,
            message: format!("Missing or empty required field(s): {}", all_missing),
            http_status: 400,
            diagnostics: None,
        };
        return (StatusCode::BAD_REQUEST, Json(err)).into_response();
    }

    let start = Instant::now();
    // Build evaluation context per request
    let ctx = EvalContext::from_request(
        &payload,
        &state.plugin_config,
        state.plugin_budget_ms,
        state.plugin_warn_ms,
    );
    let (would_be_response, plugin_timings) = state
        .pipeline
        .evaluate_with_timings(&payload, &ctx, &state.plugin_config)
        .await;
    // If audit only mode is enabled and a block would occur, override outward response.
    let response = if state.audit_only && would_be_response.block_action {
        AnalyzeResponse {
            block_action: false,
            reason_code: None,
            reason: None,
            blocked_by: None,
            diagnostics: None,
        }
    } else {
        would_be_response.clone()
    };
    let latency_ms = start.elapsed().as_millis();
    // Histogram update
    let latency_u64 = latency_ms as u64;
    state.hist_sum_ms.fetch_add(latency_u64, Ordering::Relaxed);
    state.hist_count.fetch_add(1, Ordering::Relaxed);
    // find first bucket >= value
    for (idx, ub) in state.hist_buckets.iter().enumerate() {
        if latency_u64 <= *ub {
            state.hist_counts[idx].fetch_add(1, Ordering::Relaxed);
            break;
        }
    }

    // Construct telemetry event payload
    let corr = headers
        .get("x-ms-correlation-id")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let audit_suppressed = state.audit_only && would_be_response.block_action;
    let telem = serde_json::json!({
        "schemaVersion": 1,
        "ts": chrono::Utc::now().to_rfc3339(),
        "correlationId": corr,
        "blockAction": response.block_action,
        "reasonCode": response.reason_code,
        "blockedBy": response.blocked_by.clone(),
        "latencyMs": latency_ms,
        "diagnostics": response.diagnostics.clone(),
        "auditSuppressed": if audit_suppressed { Some(true) } else { None },
        "pluginTimings": plugin_timings.iter().map(|(n,t)| serde_json::json!({"plugin":n, "ms": t})).collect::<Vec<_>>()
    });
    state.telemetry.emit_event(
        &telem,
        &TelemetryLogFields {
            block_action: response.block_action,
            reason_code: response.reason_code,
            blocked_by: response.blocked_by.as_deref(),
            latency_ms,
            audit_suppressed,
            plugin_count: plugin_timings.len(),
        },
    );

    if state.audit_only && would_be_response.block_action {
        let record = serde_json::json!({
            "schemaVersion": 1,
            "ts": chrono::Utc::now().to_rfc3339(),
            "correlationId": corr,
            "auditOnly": true,
            "wouldBlock": true,
            "wouldResponse": &would_be_response,
            "request": &payload,
        });
        state.telemetry.emit_audit(
            &record,
            &AuditLogFields {
                would_block: would_be_response.block_action,
                reason_code: would_be_response.reason_code,
                blocked_by: would_be_response.blocked_by.as_deref(),
                plugin_count: plugin_timings.len(),
            },
        );
    }
    // Metrics increments
    state.metric_requests_total.fetch_add(1, Ordering::Relaxed);
    if would_be_response.block_action {
        state.metric_blocks_total.fetch_add(1, Ordering::Relaxed);
    }
    if state.audit_only && would_be_response.block_action {
        state
            .metric_audit_suppressed_total
            .fetch_add(1, Ordering::Relaxed);
    }
    // Per-plugin timing metrics
    for (name, ms) in &plugin_timings {
        if let Some(idx) = state.plugin_metric_indices.get(name.as_str()) {
            if let Some(pm) = state.plugin_metrics.get(*idx) {
                pm.eval_sum_ms.fetch_add(*ms, Ordering::Relaxed);
                pm.eval_count.fetch_add(1, Ordering::Relaxed);
                // Histogram update per plugin
                let ms_u64 = *ms;
                pm.hist_sum_ms.fetch_add(ms_u64, Ordering::Relaxed);
                pm.hist_count.fetch_add(1, Ordering::Relaxed);
                for (bidx, ub) in state.hist_buckets.iter().enumerate() {
                    if ms_u64 <= *ub {
                        pm.hist_counts[bidx].fetch_add(1, Ordering::Relaxed);
                        break;
                    }
                }
            }
        }
    }
    // Per-plugin block counter (would-be blocker)
    if let Some(blocked_by) = &would_be_response.blocked_by {
        if let Some(idx) = state.plugin_metric_indices.get(blocked_by.as_str()) {
            if let Some(pm) = state.plugin_metrics.get(*idx) {
                pm.block_count.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
    (StatusCode::OK, Json(response)).into_response()
}

fn handle_json_rejection(state: &AppState, rejection: JsonRejection) -> axum::response::Response {
    match rejection {
        JsonRejection::BytesRejection(BytesRejection::FailedToBufferBody(
            FailedToBufferBody::LengthLimitError(_),
        )) => {
            if let Some(limit) = state.max_request_bytes {
                tracing::warn!(limit, "request body exceeded configured limit");
            } else {
                tracing::warn!("request body exceeded limit but no max_request_bytes configured");
            }
            let message = match state.max_request_bytes {
                Some(limit) => format!("Request too large (body exceeded limit {} bytes)", limit),
                None => "Request too large".to_string(),
            };
            let err = ErrorResponse {
                error_code: 4001,
                message,
                http_status: 413,
                diagnostics: None,
            };
            respond_with_error(err)
        }
        JsonRejection::BytesRejection(bytes) => bytes.into_response(),
        other => other.into_response(),
    }
}

/// Simple health endpoint for container readiness / liveness checks.
async fn healthz_handler(State(state): State<AppState>) -> axum::response::Response {
    let json = serde_json::json!({
        "status": "ok",
        "version": API_VERSION,
        "pluginCount": state.pipeline.len(),
        "budgetMs": state.plugin_budget_ms,
    });
    (StatusCode::OK, Json(json)).into_response()
}

/// Prometheus-style metrics exposition. Text format with simple counters.
async fn metrics_handler(State(state): State<AppState>) -> axum::response::Response {
    // Histogram exposition
    let mut buf = String::new();
    use std::fmt::Write as _;
    let requests = state.metric_requests_total.load(Ordering::Relaxed);
    let blocks = state.metric_blocks_total.load(Ordering::Relaxed);
    let suppressed = state.metric_audit_suppressed_total.load(Ordering::Relaxed);
    let telem = state.telemetry.lines_total().load(Ordering::Relaxed);
    let telem_errs = state.telemetry.write_errors_total().load(Ordering::Relaxed);
    let sum_ms = state.hist_sum_ms.load(Ordering::Relaxed);
    let count = state.hist_count.load(Ordering::Relaxed);
    let log_size = state
        .telemetry
        .log_file_size_bytes()
        .load(Ordering::Relaxed);
    let uptime_secs = state.process_start_instant.elapsed().as_secs_f64();
    writeln!(
        &mut buf,
        "# HELP sentra_requests_total Total analyze requests processed"
    )
    .ok();
    writeln!(&mut buf, "# TYPE sentra_requests_total counter").ok();
    writeln!(&mut buf, "sentra_requests_total {}", requests).ok();
    writeln!(
        &mut buf,
        "# HELP sentra_blocks_total Total blocking decisions (pre audit override)"
    )
    .ok();
    writeln!(&mut buf, "# TYPE sentra_blocks_total counter").ok();
    writeln!(&mut buf, "sentra_blocks_total {}", blocks).ok();
    writeln!(
        &mut buf,
        "# HELP sentra_audit_suppressed_total Blocks suppressed due to audit-only mode"
    )
    .ok();
    writeln!(&mut buf, "# TYPE sentra_audit_suppressed_total counter").ok();
    writeln!(&mut buf, "sentra_audit_suppressed_total {}", suppressed).ok();
    writeln!(
        &mut buf,
        "# HELP sentra_telemetry_lines_total Telemetry/audit JSON lines written"
    )
    .ok();
    writeln!(&mut buf, "# TYPE sentra_telemetry_lines_total counter").ok();
    writeln!(&mut buf, "sentra_telemetry_lines_total {}", telem).ok();
    writeln!(
        &mut buf,
        "# HELP sentra_telemetry_write_errors_total Telemetry/audit JSON line write failures"
    )
    .ok();
    writeln!(
        &mut buf,
        "# TYPE sentra_telemetry_write_errors_total counter"
    )
    .ok();
    writeln!(
        &mut buf,
        "sentra_telemetry_write_errors_total {}",
        telem_errs
    )
    .ok();
    // Histogram
    writeln!(
        &mut buf,
        "# HELP sentra_request_latency_ms Request latency histogram milliseconds"
    )
    .ok();
    writeln!(&mut buf, "# TYPE sentra_request_latency_ms histogram").ok();
    let mut cumulative: u64 = 0;
    for (i, ub) in state.hist_buckets.iter().enumerate() {
        let c = state.hist_counts[i].load(Ordering::Relaxed);
        cumulative += c;
        writeln!(
            &mut buf,
            "sentra_request_latency_ms_bucket{{le=\"{}\"}} {}",
            ub, cumulative
        )
        .ok();
    }
    // +Inf bucket
    writeln!(
        &mut buf,
        "sentra_request_latency_ms_bucket{{le=\"+Inf\"}} {}",
        count
    )
    .ok();
    writeln!(&mut buf, "sentra_request_latency_ms_sum {}", sum_ms).ok();
    writeln!(&mut buf, "sentra_request_latency_ms_count {}", count).ok();
    // Build info gauge (value 1)
    writeln!(
        &mut buf,
        "# HELP sentra_build_info Build information\n# TYPE sentra_build_info gauge"
    )
    .ok();
    writeln!(
        &mut buf,
        "sentra_build_info{{version=\"{}\",schemaVersion=\"1\"}} 1",
        env!("CARGO_PKG_VERSION")
    )
    .ok();
    // Per-plugin metrics exposition (HELP/TYPE once per family)
    writeln!(
        &mut buf,
        "# HELP sentra_plugin_eval_ms_sum Cumulative evaluation time (ms) per plugin\n# TYPE sentra_plugin_eval_ms_sum counter"
    ).ok();
    writeln!(
        &mut buf,
        "# HELP sentra_plugin_eval_ms_count Evaluation count per plugin\n# TYPE sentra_plugin_eval_ms_count counter"
    ).ok();
    writeln!(
        &mut buf,
        "# HELP sentra_plugin_blocks_total Blocking decisions per plugin (would-be blocks)\n# TYPE sentra_plugin_blocks_total counter"
    ).ok();
    writeln!(
        &mut buf,
        "# HELP sentra_plugin_latency_ms Plugin evaluation latency histogram (ms) per plugin\n# TYPE sentra_plugin_latency_ms histogram"
    ).ok();
    for (name, idx) in state.plugin_metric_indices.iter() {
        if let Some(pm) = state.plugin_metrics.get(*idx) {
            let sum = pm.eval_sum_ms.load(Ordering::Relaxed);
            let c = pm.eval_count.load(Ordering::Relaxed);
            let b = pm.block_count.load(Ordering::Relaxed);
            writeln!(
                &mut buf,
                "sentra_plugin_eval_ms_sum{{plugin=\"{}\"}} {}",
                name, sum
            )
            .ok();
            writeln!(
                &mut buf,
                "sentra_plugin_eval_ms_count{{plugin=\"{}\"}} {}",
                name, c
            )
            .ok();
            writeln!(
                &mut buf,
                "sentra_plugin_blocks_total{{plugin=\"{}\"}} {}",
                name, b
            )
            .ok();
            // Per-plugin histogram buckets
            let mut cumulative: u64 = 0;
            for (i, ub) in state.hist_buckets.iter().enumerate() {
                let hc = pm.hist_counts[i].load(Ordering::Relaxed);
                cumulative += hc;
                writeln!(
                    &mut buf,
                    "sentra_plugin_latency_ms_bucket{{plugin=\"{}\",le=\"{}\"}} {}",
                    name, ub, cumulative
                )
                .ok();
            }
            let pcount = pm.hist_count.load(Ordering::Relaxed);
            writeln!(
                &mut buf,
                "sentra_plugin_latency_ms_bucket{{plugin=\"{}\",le=\"+Inf\"}} {}",
                name, pcount
            )
            .ok();
            let psum = pm.hist_sum_ms.load(Ordering::Relaxed);
            writeln!(
                &mut buf,
                "sentra_plugin_latency_ms_sum{{plugin=\"{}\"}} {}",
                name, psum
            )
            .ok();
            writeln!(
                &mut buf,
                "sentra_plugin_latency_ms_count{{plugin=\"{}\"}} {}",
                name, pcount
            )
            .ok();
        }
    }
    // Log file size gauge (0 if none)
    writeln!(
        &mut buf,
        "# HELP sentra_log_file_size_bytes Current size in bytes of active telemetry log file (0 if disabled)\n# TYPE sentra_log_file_size_bytes gauge"
    )
    .ok();
    writeln!(&mut buf, "sentra_log_file_size_bytes {}", log_size).ok();
    // Process start & uptime
    writeln!(
        &mut buf,
        "# HELP sentra_process_start_time_seconds Process start time (Unix epoch seconds)\n# TYPE sentra_process_start_time_seconds gauge"
    )
    .ok();
    writeln!(
        &mut buf,
        "sentra_process_start_time_seconds {}",
        state.process_start_epoch
    )
    .ok();
    writeln!(
        &mut buf,
        "# HELP sentra_process_uptime_seconds Process uptime seconds\n# TYPE sentra_process_uptime_seconds gauge"
    )
    .ok();
    writeln!(&mut buf, "sentra_process_uptime_seconds {}", uptime_secs).ok();
    let body = buf;
    (
        StatusCode::OK,
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4",
        )],
        body,
    )
        .into_response()
}
