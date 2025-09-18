use axum::{routing::post, Json, Router};
use sentra::{AnalyzeRequest, PlannerContext, ToolDefinition};
use serde_json::json;

fn test_request(user_message: &str) -> AnalyzeRequest {
    AnalyzeRequest {
        planner_context: PlannerContext {
            user_message: Some(user_message.to_string()),
            ..Default::default()
        },
        tool_definition: ToolDefinition {
            name: Some("DemoTool".to_string()),
            ..Default::default()
        },
        input_values: serde_json::Map::new(),
        conversation_metadata: None,
    }
}

// Start a mock server that always returns a non-empty JSON array at the root.
async fn start_array_server() -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
    async fn respond(Json(_v): Json<serde_json::Value>) -> Json<serde_json::Value> {
        Json(json!([{"entity_type":"EMAIL_ADDRESS","score":0.91}]))
    }
    let app = Router::new().route("/detect", post(respond));
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (addr, handle)
}

#[tokio::test]
async fn external_http_non_empty_root_array_blocks() {
    let (addr, _handle) = start_array_server().await;
    // Configure plugin with blockField '/' and nonEmptyPointerBlocks true.
    let cfg_val = serde_json::json!({
        "externalHttp": [
            {"name":"external_array","url": format!("http://{}/detect", addr), "blockField":"/", "nonEmptyPointerBlocks": true, "reasonCode": 860, "failOpen": false}
        ]
    });
    let cfg: sentra::plugins::PluginConfig = serde_json::from_value(cfg_val).unwrap();
    let pipeline = sentra::plugins::PluginPipeline::new(&["external_array".to_string()], &cfg);
    let req = test_request("some message");
    let ctx = sentra::util::EvalContext::from_request(&req, &cfg, 900, 200);
    let (resp, _timings) = pipeline.evaluate_with_timings(&req, &ctx, &cfg).await;
    assert!(
        resp.block_action,
        "Expected block due to non-empty root array"
    );
    assert_eq!(resp.reason_code, Some(860));
    assert_eq!(resp.blocked_by.as_deref(), Some("external_array"));
}
