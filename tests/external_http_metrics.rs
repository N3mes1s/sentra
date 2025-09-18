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
    let app = Router::new().route("/m", post(decide));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://{}", addr);
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (url, handle)
}

#[tokio::test]
async fn external_http_metrics_exposed() {
    let (ext_url, _h) = start_service().await;
    let cfg = json!({
        "externalHttp": [
            {"name":"external_metrics","url": format!("{}/m", ext_url), "reasonCode": 820, "failOpen": false}
        ]
    });
    let cfg_path = tempfile::NamedTempFile::new().unwrap();
    fs::write(cfg_path.path(), serde_json::to_string(&cfg).unwrap()).unwrap();
    std::env::set_var(
        "SENTRA_PLUGIN_CONFIG",
        cfg_path.path().to_string_lossy().to_string(),
    );
    std::env::set_var("SENTRA_PLUGINS", "external_metrics");

    let state = build_state_from_env().await.unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = app(state);
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    // Trigger a request to generate metrics
    let body = json!({"plannerContext":{"userMessage":"ping"},"toolDefinition":{"name":"SendEmail"},"inputValues":{}});
    let analyze_url = format!(
        "http://{}/analyze-tool-execution?api-version=2025-05-01",
        addr
    );
    let client = Client::new();
    let resp = client
        .post(&analyze_url)
        .header("Authorization", "Bearer test")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());

    // Fetch metrics
    let metrics_url = format!("http://{}/metrics", addr);
    let metrics_text = client
        .get(&metrics_url)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        metrics_text.contains("sentra_plugin_eval_ms_sum{plugin=\"external_metrics\""),
        "missing eval sum for external plugin: {}",
        metrics_text
    );
    assert!(
        metrics_text.contains("sentra_plugin_eval_ms_count{plugin=\"external_metrics\""),
        "missing eval count for external plugin"
    );
    assert!(
        metrics_text
            .contains("sentra_plugin_latency_ms_bucket{plugin=\"external_metrics\",le=\"1\"}"),
        "missing plugin latency histogram bucket for external plugin"
    );
}
