#[path = "common/mod.rs"]
mod common;

use axum::{http::Request, Router};
use common::EnvGuard;
use once_cell::sync::Lazy;
use sentra::*;
use std::fs;
use tokio::sync::Mutex;
use tower::ServiceExt;

static ENV_MUTEX: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

#[tokio::test]
async fn telemetry_writes_jsonl_line() {
    let _lock = ENV_MUTEX.lock().await;
    let mut env = EnvGuard::new();
    let log_file = tempfile::NamedTempFile::new().unwrap();
    let log_path = log_file.path().to_path_buf();
    let log_path_str = log_path.to_string_lossy().to_string();
    env.set_many(&[
        ("LOG_FILE", &log_path_str),
        ("STRICT_AUTH_ALLOWED_TOKENS", "test"),
    ]);

    let state = build_state_from_env().await.unwrap();
    let app: Router = app(state);
    let body = serde_json::json!({
        "plannerContext": { "userMessage": "Here is my key AKIAABCDEFGH123456" },
        "toolDefinition": { "name": "SendEmail" },
        "inputValues": { "to": "bob@yourcompany.com" }
    });
    let req = Request::builder()
        .method("POST")
        .uri("/analyze-tool-execution?api-version=2025-05-01")
        .header("Authorization", "Bearer test")
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert!(resp.status().is_success());

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let content = fs::read_to_string(&log_path).expect("log file readable");
    assert!(!content.trim().is_empty(), "log file should not be empty");
    let first_line = content.lines().next().unwrap();
    let v: serde_json::Value = serde_json::from_str(first_line).expect("line parses as JSON");

    for key in ["ts", "correlationId", "blockAction", "latencyMs"] {
        assert!(v.get(key).is_some(), "missing telemetry field {key}");
    }
    assert_eq!(v.get("correlationId").unwrap(), "");
    assert!(v.get("diagnostics").is_some());

    let timings = v.get("pluginTimings").expect("pluginTimings present");
    assert!(timings.is_array(), "pluginTimings must be array");
}
