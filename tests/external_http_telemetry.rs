use axum::{routing::post, Json, Router};
use reqwest::Client;
use sentra::{app, build_state_from_env};
use serde_json::json;
use std::fs;
use tokio::net::TcpListener;

async fn start_blocker() -> (String, tokio::task::JoinHandle<()>) {
    async fn decide(Json(_v): Json<serde_json::Value>) -> Json<serde_json::Value> {
        Json(json!({"block": true}))
    }
    let app = Router::new().route("/d", post(decide));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://{}", addr);
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (url, handle)
}

#[tokio::test]
async fn external_http_telemetry_logs_block() {
    // Prepare temp log file
    let telem = tempfile::NamedTempFile::new().unwrap();
    let telem_path = telem.path().to_string_lossy().to_string();
    std::env::set_var("LOG_FILE", &telem_path);

    let (ext_url, _h) = start_blocker().await;

    // Config with external plugin first
    let cfg = json!({
        "externalHttp": [
            {"name":"external_log","url": format!("{}/d", ext_url), "reasonCode": 813, "failOpen": false}
        ]
    });
    let cfg_path = tempfile::NamedTempFile::new().unwrap();
    fs::write(cfg_path.path(), serde_json::to_string(&cfg).unwrap()).unwrap();
    std::env::set_var(
        "SENTRA_PLUGIN_CONFIG",
        cfg_path.path().to_string_lossy().to_string(),
    );
    std::env::set_var("SENTRA_PLUGINS", "external_log,secrets");

    let state = build_state_from_env().await.unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = app(state);
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let body = json!({"plannerContext":{"userMessage":"hi"},"toolDefinition":{"name":"SendEmail"},"inputValues":{}});
    let resp = Client::new()
        .post(format!(
            "http://{}/analyze-tool-execution?api-version=2025-05-01",
            addr
        ))
        .header("Authorization", "Bearer test")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());

    // Read telemetry file
    let contents = fs::read_to_string(&telem_path).unwrap();
    assert!(
        contents.contains("\"blockedBy\":\"external_log\""),
        "blockedBy missing: {}",
        contents
    );
    assert!(
        contents.contains("\"reasonCode\":813"),
        "reasonCode missing: {}",
        contents
    );
    assert!(
        contents.contains("pluginTimings"),
        "pluginTimings missing: {}",
        contents
    );
}
