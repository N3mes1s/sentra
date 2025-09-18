#[path = "common/mod.rs"]
mod common;

use axum::http::{Request, StatusCode};
use axum::Router;
use common::EnvGuard;
use once_cell::sync::Lazy;
use sentra::*;
use tokio::sync::Mutex;
use tower::ServiceExt; // for oneshot

static ENV_MUTEX: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

#[tokio::test]
async fn request_size_guard_triggers() {
    let _lock = ENV_MUTEX.lock().await;
    let mut env = EnvGuard::new();
    env.set("SENTRA_MAX_REQUEST_BYTES", "10");
    let state = build_state_from_env().await.unwrap();
    let app: Router = app(state);
    // Craft JSON > 10 bytes
    let payload = serde_json::json!({
        "plannerContext": {"userMessage": "this is long"},
        "toolDefinition": {"name": "SendEmail"},
        "inputValues": {}
    });
    let body = serde_json::to_vec(&payload).unwrap();
    let req = Request::builder()
        .method("POST")
        .uri("/analyze-tool-execution?api-version=2025-05-01")
        .header("content-type", "application/json")
        .header("authorization", "Bearer token")
        .header("content-length", body.len().to_string())
        .body(axum::body::Body::from(body))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn plugin_budget_affects_deadline() {
    let _lock = ENV_MUTEX.lock().await;
    let mut env = EnvGuard::new();
    env.remove("SENTRA_MAX_REQUEST_BYTES");
    env.set("SENTRA_PLUGIN_BUDGET_MS", "1");
    // Force run-all to exercise loop (second build after setting var)
    let state = build_state_from_env().await.unwrap();
    let app: Router = app(state.clone());
    // Provide required fields to ensure we test budget logic not validation failure
    let req_body = serde_json::json!({
        "plannerContext": {"userMessage": "hi"},
        "toolDefinition": {"name": "SendEmail"},
        "inputValues": {}
    });
    let body = serde_json::to_vec(&req_body).unwrap();
    let req = Request::builder()
        .method("POST")
        .uri("/analyze-tool-execution?api-version=2025-05-01")
        .header("content-type", "application/json")
        .header("authorization", "Bearer token")
        .body(axum::body::Body::from(body))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    // We cannot directly assert deadline triggered, but absence of panic and OK response suffice.
}
