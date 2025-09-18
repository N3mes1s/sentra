#[path = "common/mod.rs"]
mod common;

use common::EnvGuard;
use once_cell::sync::Lazy;
use sentra::{app, build_state_from_env};
use std::fs;
use tokio::sync::Mutex;

type GuardedApp = (tokio::task::JoinHandle<()>, u16);

static ENV_MUTEX: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

async fn start() -> GuardedApp {
    let state = build_state_from_env().await.unwrap();
    let app = app(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = tokio::task::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (handle, port)
}

async fn fire(port: u16) {
    let client = reqwest::Client::new();
    let url = format!(
        "http://127.0.0.1:{}/analyze-tool-execution?api-version=2025-05-01",
        port
    );
    let body = r#"{ "plannerContext": {"userMessage":"hi"}, "toolDefinition": {"name":"SendEmail"}, "inputValues": {} }"#;
    let r = client
        .post(&url)
        .header("Authorization", "Bearer test")
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success());
}

#[tokio::test]
async fn concurrency_stress_telemetry_lines_match() {
    let _lock = ENV_MUTEX.lock().await;
    let mut env = EnvGuard::new();
    let temp_dir = tempfile::tempdir().unwrap();
    let log_path = temp_dir.path().join("telemetry_stress.jsonl");
    let log_path_string = log_path.to_string_lossy().to_string();
    env.set_many(&[
        ("STRICT_AUTH_ALLOWED_TOKENS", "test"),
        ("SENTRA_PLUGINS", "secrets,pii"),
    ]);
    env.set("LOG_FILE", log_path_string.as_str());

    let total = 200u32;
    let concurrency = 32usize;

    let (handle, port) = start().await;
    let mut tasks = Vec::new();
    for _ in 0..total {
        tasks.push(tokio::spawn(fire(port)));
        if tasks.len() >= concurrency {
            for task in tasks.drain(..) {
                task.await.unwrap();
            }
        }
    }
    for task in tasks {
        task.await.unwrap();
    }

    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    handle.abort();
    let content = fs::read_to_string(&log_path).unwrap();
    let line_count = content.lines().count();
    assert_eq!(line_count as u32, total);
}
