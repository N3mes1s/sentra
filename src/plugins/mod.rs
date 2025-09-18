//! Plugin infrastructure for Sentra.
//!
//! Each plugin encapsulates one class of check.  The `PluginPipeline`
//! orchestrates the registered plugins until the first blocking plugin
//! response. Aggregated (run-all) mode has been removed for production
//! simplicity.

use std::sync::Arc;

use crate::util::EvalContext;
use crate::{AnalyzeRequest, AnalyzeResponse};

pub mod domain_block;
pub mod email_bcc;
pub mod exfil;
pub mod external_http;
pub mod pii;
pub mod policy_pack;
pub mod secrets;

use self::domain_block::DomainBlockPlugin;
use self::email_bcc::EmailBccPlugin;
use self::exfil::ExfilPlugin;
use self::external_http::ExternalHttpPlugin;
use self::pii::PiiPlugin;
use self::policy_pack::PolicyPackPlugin;
use self::secrets::SecretsPlugin;

/// Configuration parameters for plugins loaded from environment or a JSON file.
#[derive(Clone, Debug, serde::Deserialize)]
pub struct PluginConfig {
    /// Keywords to detect PII via Aho–Corasick.  All entries should be in
    /// lower case.  An empty list disables keyword scanning.
    #[serde(default, alias = "piiKeywords")]
    pub pii_keywords: Vec<String>,
    /// Additional domains that should never appear in inputs.  Lower case.
    #[serde(default, alias = "domainBlocklist")]
    pub domain_blocklist: Vec<String>,
    /// Policy rules for the policy pack plugin.
    #[serde(default)]
    pub policies: Vec<policy_pack::PolicyRule>,
    /// The company domain used for email bcc validation.  Defaults to
    /// `yourcompany.com`.
    #[serde(default = "default_company_domain")]
    pub company_domain: String,
    /// External HTTP plugin definitions. Each entry becomes an explicit plugin instance
    /// addressable by its unique `name` in the SENTRA_PLUGINS ordering variable.
    #[serde(default, alias = "externalHttp")]
    pub external_http: Vec<external_http::ExternalHttpDefinition>,
}

fn default_company_domain() -> String {
    // Default company domain; explicit String from &str
    "yourcompany.com".to_owned()
}

impl Default for PluginConfig {
    fn default() -> Self {
        Self {
            pii_keywords: Vec::new(),
            domain_blocklist: Vec::new(),
            policies: Vec::new(),
            company_domain: default_company_domain(),
            external_http: Vec::new(),
        }
    }
}

/// Trait implemented by all plugins.  Given a request and evaluation
/// context, return `Some(AnalyzeResponse)` to indicate a block or
/// transformation.  Returning `None` means the plugin has no opinion and
/// evaluation should continue.
#[async_trait::async_trait]
pub trait Plugin: Send + Sync {
    fn name(&self) -> &str;
    async fn eval(
        &self,
        req: &AnalyzeRequest,
        ctx: &EvalContext,
        cfg: &PluginConfig,
    ) -> Option<AnalyzeResponse>;
}

/// The plugin pipeline runs registered plugins in order and stops after
/// the first blocking plugin result.
#[derive(Clone)]
pub struct PluginPipeline {
    plugins: Vec<Arc<dyn Plugin>>,
}

struct PluginRun {
    response: Option<AnalyzeResponse>,
    elapsed_ms: u64,
}

impl PluginPipeline {
    pub fn new(order: &[String], cfg: &PluginConfig) -> Self {
        // Map string names to plugin implementations.  Unknown names are
        // silently ignored.
        let mut plugins: Vec<Arc<dyn Plugin>> = Vec::new();
        for name in order {
            match name.as_str() {
                "exfil" => plugins.push(Arc::new(ExfilPlugin {})),
                "secrets" => plugins.push(Arc::new(SecretsPlugin {})),
                "pii" => plugins.push(Arc::new(PiiPlugin {})),
                "email_bcc" => plugins.push(Arc::new(EmailBccPlugin {})),
                "domain_block" => plugins.push(Arc::new(DomainBlockPlugin {})),
                "policy_pack" => {
                    if !cfg.policies.is_empty() {
                        plugins.push(Arc::new(PolicyPackPlugin::new(cfg.policies.clone())));
                    }
                }
                name if name.starts_with("external_") => {
                    // Look up external http definition by exact name match
                    if let Some(def) = cfg.external_http.iter().find(|d| d.name == *name) {
                        plugins.push(Arc::new(ExternalHttpPlugin::new(def.clone())));
                    } else {
                        tracing::warn!(plugin=%name, "external_http definition not found");
                    }
                }
                _ => {
                    tracing::warn!(plugin = %name, "unknown plugin name, skipping");
                }
            }
        }
        Self { plugins }
    }

    /// Evaluate all plugins against the request and context.  Returns an
    /// `AnalyzeResponse` where `blockAction` indicates whether the tool
    /// invocation should be blocked.  Stops at first blocking plugin.
    pub async fn evaluate_with_timings(
        &self,
        req: &AnalyzeRequest,
        ctx: &EvalContext,
        cfg: &PluginConfig,
    ) -> (AnalyzeResponse, Vec<(String, u64)>) {
        let mut timings: Vec<(String, u64)> = Vec::new();
        for plugin in &self.plugins {
            let pname = plugin.name();
            if ctx.deadline.exceeded() {
                tracing::warn!(
                    plugin_count = self.plugins.len(),
                    "deadline exceeded, aborting further plugin checks"
                );
                break;
            }
            tracing::trace!(plugin = %pname, remaining_ms = ctx.deadline.remaining_ms(), "evaluating plugin");
            let run = Self::run_plugin(plugin, req, ctx, cfg, pname).await;
            timings.push((pname.to_string(), run.elapsed_ms));
            if let Some(mut resp) = run.response {
                if resp.block_action {
                    tracing::info!(plugin = %pname, reason_code = ?resp.reason_code, "blocking");
                    if resp.blocked_by.is_none() {
                        resp.blocked_by = Some(pname.to_string());
                    }
                    return (resp, timings);
                }
                tracing::debug!(plugin = %pname, "plugin allowed");
            }
        }
        (
            AnalyzeResponse {
                block_action: false,
                reason_code: None,
                reason: None,
                blocked_by: None,
                diagnostics: None,
            },
            timings,
        )
    }

    /// Number of registered plugins.
    pub fn len(&self) -> usize {
        self.plugins.len()
    }

    /// Returns true if the pipeline has no registered plugins.
    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }

    async fn run_plugin(
        plugin: &Arc<dyn Plugin>,
        req: &AnalyzeRequest,
        ctx: &EvalContext,
        cfg: &PluginConfig,
        name: &str,
    ) -> PluginRun {
        let start = std::time::Instant::now();
        let response = plugin.eval(req, ctx, cfg).await;
        let elapsed_ms = start.elapsed().as_millis() as u64;
        if elapsed_ms > ctx.plugin_warn_ms {
            tracing::warn!(
                plugin = %name,
                elapsed_ms,
                warn_ms = ctx.plugin_warn_ms,
                "plugin exceeded warn threshold"
            );
        }
        PluginRun {
            response,
            elapsed_ms,
        }
    }
}

/// Helper used by `build_state_from_env` to parse the list of plugin
/// identifiers from an environment variable.  If unset, a default list is
/// returned.  Strings are trimmed and lower‑cased.
pub fn parse_plugin_order() -> Vec<String> {
    if let Ok(var) = std::env::var("SENTRA_PLUGINS") {
        var.split(',')
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty())
            .collect()
    } else {
        vec![
            "exfil".into(),
            "secrets".into(),
            "email_bcc".into(),
            "pii".into(),
            "domain_block".into(),
            "policy_pack".into(),
        ]
    }
}
