use reqwest::Client;
use sentra::{app, build_state_from_env};
use tokio::net::TcpListener;

// Basic smoke test for /metrics endpoint including new series.
#[tokio::test]
async fn metrics_includes_new_series() {
    // Ensure deterministic plugin ordering for assertions.
    std::env::set_var(
        "SENTRA_PLUGINS",
        "exfil,secrets,email_bcc,pii,domain_block,policy_pack",
    );
    let state = build_state_from_env().await.unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = app(state.clone());
    let _h = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    // Hit analyze a couple times to generate observations.
    let analyze_url = format!(
        "http://{}/analyze-tool-execution?api-version=2025-05-01",
        addr
    );
    let body = serde_json::json!({
        "plannerContext": { "userMessage": "Ping" },
        "toolDefinition": { "name": "SendEmail" },
        "inputValues": { "to": "alice@yourcompany.com" }
    });
    for _ in 0..3u8 {
        let _ = Client::new()
            .post(&analyze_url)
            .header("Authorization", "Bearer test")
            .json(&body)
            .send()
            .await
            .unwrap();
    }
    // Fetch metrics
    let metrics_url = format!("http://{}/metrics", addr);
    let resp = Client::new().get(&metrics_url).send().await.unwrap();
    assert!(resp.status().is_success());
    let text = resp.text().await.unwrap();
    // Core counters
    assert!(text.contains("sentra_requests_total"));
    assert!(text.contains("sentra_blocks_total"));
    assert!(text.contains("sentra_request_latency_ms_bucket{le=\"+Inf\"}"));
    // New per-plugin counters
    assert!(text.contains("sentra_plugin_eval_ms_sum{plugin=\"exfil\"}"));
    assert!(text.contains("sentra_plugin_eval_ms_count{plugin=\"exfil\"}"));
    assert!(text.contains("sentra_plugin_blocks_total{plugin=\"exfil\"}"));
    // Plugin histogram buckets
    assert!(text.contains("sentra_plugin_latency_ms_bucket{plugin=\"exfil\",le=\"+Inf\"}"));
    assert!(text.contains("sentra_plugin_latency_ms_sum{plugin=\"exfil\"}"));
    assert!(text.contains("sentra_plugin_latency_ms_count{plugin=\"exfil\"}"));
    // Ensure HELP line appears only once for plugin histogram metric family
    let help_occurrences = text.matches("# HELP sentra_plugin_latency_ms").count();
    assert_eq!(
        help_occurrences, 1,
        "plugin latency HELP line should appear exactly once"
    );
    // Process metrics
    assert!(text.contains("sentra_process_start_time_seconds"));
    assert!(text.contains("sentra_process_uptime_seconds"));
    // Cleanup env to avoid impacting other tests
    std::env::remove_var("SENTRA_PLUGINS");
}
