use axum::{routing::post, Json, Router};
use reqwest::Client;
use sentra::{app, build_state_from_env};
use serde_json::json;
use std::fs;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

// Spin up a mock external service that blocks when userMessage contains "ban".
async fn start_mock_service() -> (String, JoinHandle<()>) {
    async fn decide(Json(v): Json<serde_json::Value>) -> Json<serde_json::Value> {
        let msg = v.get("userMessage").and_then(|x| x.as_str()).unwrap_or("");
        if msg.contains("ban") {
            Json(json!({"block": true}))
        } else {
            Json(json!({"block": false}))
        }
    }
    let app = Router::new().route("/decision", post(decide));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://{}", addr);
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (url, handle)
}

async fn spawn_app_with_external(url: &str) -> (String, JoinHandle<()>) {
    // Write a temp plugin config file referencing the external endpoint
    let cfg = json!({
        "externalHttp": [
            {"name":"external_blocker","url": format!("{}/decision", url), "reasonCode": 810, "timeoutMs": 300, "failOpen": false}
        ]
    });
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ext_cfg.json");
    fs::write(&path, serde_json::to_string(&cfg).unwrap()).unwrap();
    std::env::set_var("SENTRA_PLUGIN_CONFIG", path.to_string_lossy().to_string());
    std::env::set_var("SENTRA_PLUGINS", "external_blocker,secrets,pii");
    // Build state and launch app
    let state = build_state_from_env().await.unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = app(state);
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{}", addr), handle)
}

#[tokio::test]
async fn external_http_integration_blocks_and_allows() {
    let (ext_url, _ext_handle) = start_mock_service().await;
    let (app_url, _app_handle) = spawn_app_with_external(&ext_url).await;
    let client = Client::new();
    // Allow case
    let allow_body = json!({
        "plannerContext": {"userMessage": "hello"},
        "toolDefinition": {"name": "SendEmail"},
        "inputValues": {}
    });
    let allow_resp = client
        .post(format!(
            "{}/analyze-tool-execution?api-version=2025-05-01",
            app_url
        ))
        .header("Authorization", "Bearer test")
        .json(&allow_body)
        .send()
        .await
        .unwrap();
    assert!(allow_resp.status().is_success());
    let allow_json: serde_json::Value = allow_resp.json().await.unwrap();
    assert_eq!(allow_json.get("blockAction"), Some(&json!(false)));

    // Block case
    let block_body = json!({
        "plannerContext": {"userMessage": "please ban this"},
        "toolDefinition": {"name": "SendEmail"},
        "inputValues": {}
    });
    let block_resp = client
        .post(format!(
            "{}/analyze-tool-execution?api-version=2025-05-01",
            app_url
        ))
        .header("Authorization", "Bearer test")
        .json(&block_body)
        .send()
        .await
        .unwrap();
    assert!(block_resp.status().is_success());
    let block_json: serde_json::Value = block_resp.json().await.unwrap();
    assert_eq!(block_json.get("blockAction"), Some(&json!(true)));
    assert_eq!(block_json.get("reasonCode"), Some(&json!(810)));
    assert_eq!(
        block_json.get("blockedBy"),
        Some(&json!("external_blocker"))
    );
}
