use axum::{routing::post, Json, Router};
use reqwest::Client;
use sentra::{app, build_state_from_env};
use serde_json::json;
use std::fs;
use tokio::net::TcpListener;

async fn start_service() -> (String, tokio::task::JoinHandle<()>) {
    async fn decide(Json(_v): Json<serde_json::Value>) -> Json<serde_json::Value> {
        Json(json!({"block": false}))
    }
    let app = Router::new().route("/s", post(decide));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://{}", addr);
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (url, handle)
}

#[tokio::test]
async fn external_http_metrics_increments_across_calls() {
    let (ext_url, _h) = start_service().await;
    let cfg = json!({
        "externalHttp": [
            {"name":"external_seq","url": format!("{}/s", ext_url), "reasonCode": 840, "failOpen": false}
        ]
    });
    let cfg_path = tempfile::NamedTempFile::new().unwrap();
    fs::write(cfg_path.path(), serde_json::to_string(&cfg).unwrap()).unwrap();
    std::env::set_var(
        "SENTRA_PLUGIN_CONFIG",
        cfg_path.path().to_string_lossy().to_string(),
    );
    std::env::set_var("SENTRA_PLUGINS", "external_seq");

    let state = build_state_from_env().await.unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = app(state);
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let client = Client::new();
    let analyze_url = format!(
        "http://{}/analyze-tool-execution?api-version=2025-05-01",
        addr
    );
    let body = json!({"plannerContext":{"userMessage":"ping"},"toolDefinition":{"name":"SendEmail"},"inputValues":{}});

    // Send three sequential requests
    for _ in 0..3 {
        client
            .post(&analyze_url)
            .header("Authorization", "Bearer test")
            .json(&body)
            .send()
            .await
            .unwrap();
    }

    let metrics_url = format!("http://{}/metrics", addr);
    let metrics_text = client
        .get(&metrics_url)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    // Expect eval count at least 3
    let needle = "sentra_plugin_eval_ms_count{plugin=\"external_seq\"";
    let line = metrics_text
        .lines()
        .find(|l| l.starts_with(needle))
        .expect("missing eval count line");
    // Parse last number
    let count: u64 = line.split_whitespace().last().unwrap().parse().unwrap();
    assert!(
        count >= 3,
        "expected at least 3 eval count, got {} in line '{}'",
        count,
        line
    );
}
