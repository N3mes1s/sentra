#[path = "common/mod.rs"]
mod common;

use common::EnvGuard;
use once_cell::sync::Lazy;
use reqwest::Client;
use tokio::net::TcpListener as TokioTcpListener;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

use sentra::{app, build_state_from_env};

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
async fn strict_auth_scenarios() {
    let _lock = ENV_MUTEX.lock().await;
    let mut env = EnvGuard::new();
    env.set("STRICT_AUTH_ALLOWED_TOKENS", "good");

    let (addr1, handle1) = spawn_app().await;
    let url1 = format!("{}/validate?api-version=2025-05-01", addr1);
    let resp1 = Client::new()
        .post(&url1)
        .header("Authorization", "Bearer bad")
        .send()
        .await
        .unwrap();
    assert_eq!(resp1.status(), reqwest::StatusCode::UNAUTHORIZED);
    handle1.abort();

    env.set("STRICT_AUTH_ALLOWED_TOKENS", "tok1,tok2,good");
    let (addr2, handle2) = spawn_app().await;
    let url2 = format!("{}/validate?api-version=2025-05-01", addr2);
    let resp2 = Client::new()
        .post(&url2)
        .header("Authorization", "Bearer good")
        .send()
        .await
        .unwrap();
    assert_eq!(resp2.status(), reqwest::StatusCode::OK);
    handle2.abort();

    env.set("STRICT_AUTH_ALLOWED_TOKENS", "aaa");
    let (addr3, handle3) = spawn_app().await;
    let url3 = format!("{}/analyze-tool-execution?api-version=2025-05-01", addr3);
    let body = serde_json::json!({
        "plannerContext": { "userMessage": "Hello" },
        "toolDefinition": { "name": "SendEmail" },
        "inputValues": { "to": "alice@yourcompany.com" }
    });
    let client = Client::new();
    let resp_bad = client
        .post(&url3)
        .header("Authorization", "Bearer bad")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp_bad.status(), reqwest::StatusCode::UNAUTHORIZED);
    let resp_ok = client
        .post(&url3)
        .header("Authorization", "Bearer aaa")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp_ok.status(), reqwest::StatusCode::OK);
    handle3.abort();
}
