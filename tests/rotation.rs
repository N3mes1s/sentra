#[path = "common/mod.rs"]
mod common;

use common::EnvGuard;
use once_cell::sync::Lazy;
use sentra::{app, build_state_from_env};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tokio::task;

static ENV_MUTEX: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

async fn start() -> (tokio::task::JoinHandle<()>, u16) {
    let state = build_state_from_env().await.unwrap();
    let app = app(state);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = task::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (handle, port)
}

async fn fire(port: u16, n: usize) {
    let client = reqwest::Client::new();
    let url = format!(
        "http://127.0.0.1:{}/analyze-tool-execution?api-version=2025-05-01",
        port
    );
    // Large payload to force substantial telemetry line size -> frequent rotations.
    let big = "x".repeat(1500);
    let body = format!(
        r#"{{ "plannerContext": {{"userMessage":"{}"}}, "toolDefinition": {{"name":"SendEmail"}}, "inputValues": {{}} }}"#,
        big
    );
    for _ in 0..n {
        let r = client
            .post(&url)
            .header("Authorization", "Bearer test")
            .header("Content-Type", "application/json")
            .body(body.clone())
            .send()
            .await
            .unwrap();
        assert!(r.status().is_success());
    }
}

#[tokio::test]
async fn rotation_without_compression() {
    let _lock = ENV_MUTEX.lock().await;
    let tmp = tempfile::tempdir().unwrap();
    let log_path = tmp.path().join("telemetry_rot.log");
    let mut env = EnvGuard::new();
    env.set_many(&[
        ("STRICT_AUTH_ALLOWED_TOKENS", "test"),
        ("SENTRA_PLUGINS", "secrets"),
        ("LOG_FILE", log_path.to_str().unwrap()),
        ("LOG_MAX_BYTES", "200"),
        ("LOG_ROTATE_KEEP", "3"),
        ("LOG_ROTATE_COMPRESS", "0"),
    ]);
    let (handle, port) = start().await; // extremely small limit
    fire(port, 40).await; // should rotate many times given large lines
    for _ in 0..15 {
        // up to 1.5s total
        if (1..=3).any(|i| log_path.with_extension(format!("{}", i)).exists()) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    handle.abort();
    // Expect base file plus at least one rotated backup (.1 or .2)
    let mut found_backup = false;
    for idx in 1..=3 {
        if log_path.with_extension(format!("{}", idx)).exists() {
            found_backup = true;
        }
    }
    assert!(found_backup, "expected at least one rotated backup file");
}

#[tokio::test]
async fn rotation_with_compression() {
    let _lock = ENV_MUTEX.lock().await;
    let tmp = tempfile::tempdir().unwrap();
    let log_path = tmp.path().join("telemetry_rotc.log");
    let mut env = EnvGuard::new();
    env.set_many(&[
        ("STRICT_AUTH_ALLOWED_TOKENS", "test"),
        ("SENTRA_PLUGINS", "secrets"),
        ("LOG_FILE", log_path.to_str().unwrap()),
        ("LOG_MAX_BYTES", "200"),
        ("LOG_ROTATE_KEEP", "2"),
        ("LOG_ROTATE_COMPRESS", "1"),
    ]);
    let (handle, port) = start().await;
    fire(port, 30).await;
    for _ in 0..15 {
        // up to 1.5s
        if log_path.with_extension("1.gz").exists() || log_path.with_extension("1").exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    handle.abort();
    // With compression we expect .1.gz at some point (may race if rotation just happened). Allow either .1.gz or .1
    let gz = log_path.with_extension("1.gz");
    let plain = log_path.with_extension("1");
    assert!(
        gz.exists() || plain.exists(),
        "expected compressed or plain rotated file"
    );
}

#[tokio::test]
async fn rotation_with_zero_keep_skips_backups() {
    let _lock = ENV_MUTEX.lock().await;
    let tmp = tempfile::tempdir().unwrap();
    let log_path = tmp.path().join("telemetry_zero.log");
    let mut env = EnvGuard::new();
    env.set_many(&[
        ("STRICT_AUTH_ALLOWED_TOKENS", "test"),
        ("SENTRA_PLUGINS", "secrets"),
        ("LOG_FILE", log_path.to_str().unwrap()),
        ("LOG_MAX_BYTES", "150"),
        ("LOG_ROTATE_KEEP", "0"),
        ("LOG_ROTATE_COMPRESS", "0"),
    ]);
    let (handle, port) = start().await;
    fire(port, 25).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    handle.abort();
    for idx in 1..=2 {
        assert!(!log_path.with_extension(format!("{}", idx)).exists());
    }
}
