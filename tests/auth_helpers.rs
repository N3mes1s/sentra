#[path = "common/mod.rs"]
mod common;

use axum::{body::Body, http::Request, http::StatusCode};

use common::EnvGuard;
use once_cell::sync::Lazy;
use sentra::{app, build_state_from_env};
use tokio::sync::Mutex;
use tower::ServiceExt;

static ENV_MUTEX: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

#[tokio::test]
async fn bearer_header_is_case_insensitive() {
    let _lock = ENV_MUTEX.lock().await;
    let mut env = EnvGuard::new();
    env.remove("STRICT_AUTH_ALLOWED_TOKENS");
    env.set("STRICT_AUTH_ALLOWED_TOKENS", "case_token");

    let state = build_state_from_env().await.unwrap();
    let app = app(state);
    let request = Request::builder()
        .method("POST")
        .uri("/validate?api-version=2025-05-01")
        .header("Authorization", "bEaReR case_token")
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn rejects_token_not_in_allow_list() {
    let _lock = ENV_MUTEX.lock().await;
    let mut env = EnvGuard::new();
    env.remove("STRICT_AUTH_ALLOWED_TOKENS");
    env.set("STRICT_AUTH_ALLOWED_TOKENS", "alpha,beta");

    let state = build_state_from_env().await.unwrap();
    let app = app(state);

    let ok_request = Request::builder()
        .method("POST")
        .uri("/validate?api-version=2025-05-01")
        .header("Authorization", "Bearer beta")
        .body(Body::empty())
        .unwrap();
    let ok_response = app.clone().oneshot(ok_request).await.unwrap();
    assert_eq!(ok_response.status(), StatusCode::OK);

    let forbid_request = Request::builder()
        .method("POST")
        .uri("/validate?api-version=2025-05-01")
        .header("Authorization", "Bearer gamma")
        .body(Body::empty())
        .unwrap();
    let forbidden = app.oneshot(forbid_request).await.unwrap();
    assert_eq!(forbidden.status(), StatusCode::UNAUTHORIZED);
}
