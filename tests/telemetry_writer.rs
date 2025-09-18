#[path = "common/mod.rs"]
mod common;

use common::EnvGuard;
use once_cell::sync::Lazy;
use sentra::{app, build_state_from_env};
use std::fs;
use tokio::net::TcpListener;
use tokio::sync::Mutex;

static TEST_GUARD: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

async fn start_server() -> (
    u16,
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
) {
    let state = build_state_from_env().await.expect("state");
    let app = app(state);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let handle = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = rx.await;
            })
            .await
            .ok();
    });
    (port, tx, handle)
}

async fn post_json(port: u16, auth: &str, body: &str) -> u16 {
    let client = reqwest::Client::new();
    let url = format!(
        "http://127.0.0.1:{}/analyze-tool-execution?api-version=2025-05-01",
        port
    );
    client
        .post(url)
        .header("Authorization", format!("Bearer {}", auth))
        .header("Content-Type", "application/json")
        .body(body.to_string())
        .send()
        .await
        .unwrap()
        .status()
        .as_u16()
}

#[tokio::test]
async fn telemetry_writes_when_enabled() {
    let temp_dir = tempfile::tempdir().unwrap();
    let telem_path = temp_dir.path().join("telemetry.jsonl");

    let _lock = TEST_GUARD.lock().await;
    let mut env = EnvGuard::new();
    env.remove("LOG_FILE");
    env.set_many(&[
        ("STRICT_AUTH_ALLOWED_TOKENS", "test"),
        ("SENTRA_PLUGINS", "secrets,pii"),
    ]);
    env.set("LOG_FILE", telem_path.to_str().unwrap());

    let (port, shutdown_tx, handle) = start_server().await;
    let body = r#"{ "plannerContext": {"userMessage": "hello"}, "toolDefinition": {"name": "SendEmail"}, "inputValues": {} }"#;
    for _ in 0..3 {
        assert_eq!(post_json(port, "test", body).await, 200);
    }
    let _ = shutdown_tx.send(());
    let _ = handle.await;
    let content = fs::read_to_string(&telem_path).expect("telemetry content");
    let lines: Vec<&str> = content.lines().collect();
    assert!(lines.len() >= 3, "expected >=3 lines, got {}", lines.len());
    for line in lines {
        let v: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_eq!(v["schemaVersion"].as_i64(), Some(1));
    }
}

#[tokio::test]
async fn telemetry_absent_when_disabled() {
    let _lock = TEST_GUARD.lock().await;
    let mut env = EnvGuard::new();
    env.remove("LOG_FILE");
    env.set_many(&[
        ("STRICT_AUTH_ALLOWED_TOKENS", "test"),
        ("SENTRA_PLUGINS", "secrets"),
    ]);

    let (port, shutdown_tx, handle) = start_server().await;
    let body = r#"{ "plannerContext": {"userMessage": "hello"}, "toolDefinition": {"name": "SendEmail"}, "inputValues": {} }"#;
    assert_eq!(post_json(port, "test", body).await, 200);
    let _ = shutdown_tx.send(());
    let _ = handle.await;
    // No implicit file should exist relative to cwd; we only assert that a typical default name is absent.
    assert!(!std::path::Path::new("telemetry.jsonl").exists());
}
