#[path = "common/mod.rs"]
mod common;

use axum::Router;
use bytes::Bytes;
use common::EnvGuard;
use http_body::Frame;
use http_body_util::StreamBody;
use once_cell::sync::Lazy;
use reqwest::{Client, StatusCode};
use sentra::{app, build_state_from_env};
use std::convert::Infallible;
use tokio::net::TcpListener as TokioTcpListener;
use tokio::sync::Mutex;
use tokio_stream::iter;

type GuardedHandle = (String, tokio::task::JoinHandle<()>);

static ENV_MUTEX: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

async fn spawn_app() -> GuardedHandle {
    let listener = TokioTcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let state = build_state_from_env().await.unwrap();
    let app: Router = app(state);
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{}", addr), handle)
}

#[tokio::test]
async fn chunked_payload_over_limit_returns_error_response() {
    let _lock = ENV_MUTEX.lock().await;
    let mut env = EnvGuard::new();
    env.set_many(&[
        ("SENTRA_MAX_REQUEST_BYTES", "256"),
        ("STRICT_AUTH_ALLOWED_TOKENS", "test"),
    ]);

    let (base, handle) = spawn_app().await;
    let url = format!("{}/analyze-tool-execution?api-version=2025-05-01", base);

    let oversized_text = "X".repeat(2048);
    let payload = serde_json::json!({
        "plannerContext": { "userMessage": oversized_text },
        "toolDefinition": { "name": "SendEmail" },
        "inputValues": {}
    })
    .to_string();

    let chunk_bytes: Vec<_> = payload
        .as_bytes()
        .chunks(128)
        .map(Bytes::copy_from_slice)
        .collect();

    let stream = iter(
        chunk_bytes
            .into_iter()
            .map(|chunk| Ok::<_, Infallible>(Frame::data(chunk))),
    );
    let body = StreamBody::new(stream);
    let body = reqwest::Body::wrap(body);

    let client = Client::new();
    let resp = client
        .post(&url)
        .header("authorization", "Bearer test")
        .header("content-type", "application/json")
        .body(body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let json: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(json.get("errorCode").and_then(|v| v.as_i64()), Some(4001));
    handle.abort();
}
