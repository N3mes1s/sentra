#[path = "common/mod.rs"]
mod common;

use common::EnvGuard;
use once_cell::sync::Lazy;
use sentra::{app, build_state_from_env};
use std::fs;
use tokio::net::TcpListener as TokioTcpListener;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

type GuardedHandle = (String, JoinHandle<()>);

static ENV_MUTEX: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

async fn spawn_app() -> GuardedHandle {
    let listener = TokioTcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let state = build_state_from_env().await.unwrap();
    let app = app(state);
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{}", addr), handle)
}

#[tokio::test]
async fn telemetry_marks_audit_suppressed_block() {
    let _lock = ENV_MUTEX.lock().await;
    let mut env = EnvGuard::new();
    let log_file = tempfile::NamedTempFile::new().unwrap();
    let log_path = log_file.path().to_path_buf();
    let log_path_str = log_path.to_string_lossy().to_string();
    env.set_many(&[
        ("SENTRA_PLUGINS", "secrets,pii"),
        ("SENTRA_AUDIT_ONLY", "true"),
    ]);
    env.set("LOG_FILE", log_path_str.as_str());

    let (addr, handle) = spawn_app().await;
    let url = format!("{}/analyze-tool-execution?api-version=2025-05-01", addr);
    let body = serde_json::json!({
        "plannerContext": { "userMessage": "Here is key AKIAABCDEF1234567890 now" },
        "toolDefinition": { "name": "SendEmail" },
        "inputValues": { "to": "bob@yourcompany.com" }
    });
    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .header("Authorization", "Bearer test")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());
    let v: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(v.get("blockAction").and_then(|b| b.as_bool()), Some(false));

    tokio::time::sleep(std::time::Duration::from_millis(40)).await;
    let content = fs::read_to_string(&log_path).expect("read log");
    handle.abort();
    assert!(!content.is_empty());
    let found = content.lines().any(|line| {
        serde_json::from_str::<serde_json::Value>(line)
            .ok()
            .and_then(|v| v.get("auditSuppressed").and_then(|b| b.as_bool()))
            == Some(true)
    });
    assert!(found, "expected telemetry line with auditSuppressed=true");
}
