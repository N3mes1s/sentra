use axum::{routing::post, Json, Router};
use sentra::{AnalyzeRequest, PlannerContext, ToolDefinition};
use serde_json::json;
use std::net::SocketAddr;
use tokio::task::JoinHandle;

// Spin up a tiny external decision service used by the plugin integration test.
async fn start_mock_external() -> (SocketAddr, JoinHandle<()>) {
    async fn decide(Json(v): Json<serde_json::Value>) -> Json<serde_json::Value> {
        // Deterministic: block unless message explicitly contains 'allow'
        let user_msg = v.get("userMessage").and_then(|x| x.as_str()).unwrap_or("");
        if user_msg.contains("allow") {
            Json(json!({"block": false}))
        } else {
            Json(json!({"block": true, "detail":"blocked"}))
        }
    }
    let app = Router::new().route("/eval", post(decide));
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (addr, handle)
}

fn test_request(user_message: &str, tool: &str) -> AnalyzeRequest {
    AnalyzeRequest {
        planner_context: PlannerContext {
            user_message: Some(user_message.to_string()),
            ..Default::default()
        },
        tool_definition: ToolDefinition {
            name: Some(tool.to_string()),
            ..Default::default()
        },
        input_values: serde_json::Map::new(),
        conversation_metadata: None,
    }
}

#[tokio::test]
async fn external_http_blocks_and_allows() {
    let (ext_addr, _handle) = start_mock_external().await;
    let cfg = serde_json::json!({
        "externalHttp": [
            {"name":"external_test", "url": format!("http://{}/eval", ext_addr), "timeoutMs":200, "reasonCode":801, "failOpen":false}
        ]
    });
    let cfg: sentra::plugins::PluginConfig = serde_json::from_value(cfg).unwrap();
    let order = vec!["external_test".to_string()];
    let pipeline = sentra::plugins::PluginPipeline::new(&order, &cfg);

    // Block case
    let req_block = test_request("this should be blocked", "Tool");
    let ctx_block = sentra::util::EvalContext::from_request(&req_block, &cfg, 900, 200);
    let (resp_block, _) = pipeline
        .evaluate_with_timings(&req_block, &ctx_block, &cfg)
        .await;
    assert!(resp_block.block_action, "expected block");
    assert_eq!(resp_block.reason_code, Some(801));
    assert_eq!(resp_block.blocked_by.as_deref(), Some("external_test"));

    // Allow case
    let req_allow = test_request("please allow this", "Tool");
    let ctx_allow = sentra::util::EvalContext::from_request(&req_allow, &cfg, 900, 200);
    let (resp_allow, _) = pipeline
        .evaluate_with_timings(&req_allow, &ctx_allow, &cfg)
        .await;
    assert!(!resp_allow.block_action, "expected allow");
}

// Simulate network failure by pointing to an unused port; failOpen=true should allow.
#[tokio::test]
async fn external_http_fail_open_allows_on_error() {
    // Choose an unlikely high port (no listener). If by chance it's used the test still valid if returns quickly.
    let unused_url = format!("http://127.0.0.1:{}/eval", 65_535u16.saturating_sub(10));
    let cfg_val = serde_json::json!({
        "externalHttp": [
            {"name":"external_err","url":unused_url, "timeoutMs":100, "reasonCode":803, "failOpen":true}
        ]
    });
    let cfg: sentra::plugins::PluginConfig = serde_json::from_value(cfg_val).unwrap();
    let pipeline = sentra::plugins::PluginPipeline::new(&["external_err".to_string()], &cfg);
    let req = test_request("anything", "Tool");
    let ctx = sentra::util::EvalContext::from_request(&req, &cfg, 900, 200);
    let (resp, _) = pipeline.evaluate_with_timings(&req, &ctx, &cfg).await;
    assert!(!resp.block_action, "fail-open network error should allow");
}

// Fail-closed variant: network error should block with configured reasonCode.
#[tokio::test]
async fn external_http_fail_closed_blocks_on_error() {
    let unused_url = format!("http://127.0.0.1:{}/eval", 65_534u16.saturating_sub(10));
    let cfg_val = serde_json::json!({
        "externalHttp": [
            {"name":"external_err_closed","url":unused_url, "timeoutMs":100, "reasonCode":804, "failOpen":false}
        ]
    });
    let cfg: sentra::plugins::PluginConfig = serde_json::from_value(cfg_val).unwrap();
    let pipeline = sentra::plugins::PluginPipeline::new(&["external_err_closed".to_string()], &cfg);
    let req = test_request("anything", "Tool");
    let ctx = sentra::util::EvalContext::from_request(&req, &cfg, 900, 200);
    let (resp, _) = pipeline.evaluate_with_timings(&req, &ctx, &cfg).await;
    assert!(resp.block_action, "fail-closed network error should block");
    assert_eq!(resp.reason_code, Some(804));
}

// JSON pointer blockField.
#[tokio::test]
async fn external_http_json_pointer_block_field() {
    async fn pointer_decide(Json(_v): Json<serde_json::Value>) -> Json<serde_json::Value> {
        Json(json!({"decision": {"block": true}}))
    }
    let app = Router::new().route("/ptr", post(pointer_decide));
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let cfg_val = serde_json::json!({
        "externalHttp": [
            {"name":"external_ptr","url": format!("http://{}/ptr", addr), "blockField":"/decision/block", "reasonCode":805}
        ]
    });
    let cfg: sentra::plugins::PluginConfig = serde_json::from_value(cfg_val).unwrap();
    let pipeline = sentra::plugins::PluginPipeline::new(&["external_ptr".to_string()], &cfg);
    let req = test_request("whatever", "Tool");
    let ctx = sentra::util::EvalContext::from_request(&req, &cfg, 900, 200);
    let (resp, _) = pipeline.evaluate_with_timings(&req, &ctx, &cfg).await;
    assert!(resp.block_action, "pointer block field should block");
    assert_eq!(resp.reason_code, Some(805));
    drop(handle);
}

// blockField = "allow" (invert semantics): when service returns {"allow": true} => allow; false => block
#[tokio::test]
async fn external_http_allow_block_field_inversion() {
    async fn allow_decide(Json(v): Json<serde_json::Value>) -> Json<serde_json::Value> {
        let user_msg = v.get("userMessage").and_then(|x| x.as_str()).unwrap_or("");
        if user_msg.contains("deny") {
            Json(json!({"allow": false}))
        } else {
            Json(json!({"allow": true}))
        }
    }
    let app = Router::new().route("/allow", post(allow_decide));
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let cfg_val = serde_json::json!({
        "externalHttp": [
            {"name":"external_allow","url": format!("http://{}/allow", addr), "blockField":"allow", "reasonCode":806}
        ]
    });
    let cfg: sentra::plugins::PluginConfig = serde_json::from_value(cfg_val).unwrap();
    let pipeline = sentra::plugins::PluginPipeline::new(&["external_allow".to_string()], &cfg);
    // allow path
    let req_allow = test_request("safe", "Tool");
    let ctx_allow = sentra::util::EvalContext::from_request(&req_allow, &cfg, 900, 200);
    let (resp_allow, _) = pipeline
        .evaluate_with_timings(&req_allow, &ctx_allow, &cfg)
        .await;
    assert!(!resp_allow.block_action);
    // deny path
    let req_block = test_request("please deny this", "Tool");
    let ctx_block = sentra::util::EvalContext::from_request(&req_block, &cfg, 900, 200);
    let (resp_block, _) = pipeline
        .evaluate_with_timings(&req_block, &ctx_block, &cfg)
        .await;
    assert!(resp_block.block_action);
    assert_eq!(resp_block.reason_code, Some(806));
    drop(handle);
}

// Timeout handling: server sleeps beyond timeout. We run two variants.
#[tokio::test]
async fn external_http_timeout_fail_open_allows() {
    async fn slow(Json(_): Json<serde_json::Value>) -> Json<serde_json::Value> {
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        Json(json!({"block": true}))
    }
    let app = Router::new().route("/slow", post(slow));
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let cfg_val = serde_json::json!({
        "externalHttp": [
            {"name":"external_timeout_open","url": format!("http://{}/slow", addr), "timeoutMs": 50, "failOpen": true, "reasonCode": 811}
        ]
    });
    let cfg: sentra::plugins::PluginConfig = serde_json::from_value(cfg_val).unwrap();
    let pipeline =
        sentra::plugins::PluginPipeline::new(&["external_timeout_open".to_string()], &cfg);
    let req = test_request("anything", "Tool");
    let ctx = sentra::util::EvalContext::from_request(&req, &cfg, 900, 200);
    let (resp, _) = pipeline.evaluate_with_timings(&req, &ctx, &cfg).await;
    assert!(!resp.block_action, "fail-open timeout should allow");
    drop(handle);
}

#[tokio::test]
async fn external_http_timeout_fail_closed_blocks() {
    async fn slow(Json(_): Json<serde_json::Value>) -> Json<serde_json::Value> {
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        Json(json!({"block": false}))
    }
    let app = Router::new().route("/slow", post(slow));
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let cfg_val = serde_json::json!({
        "externalHttp": [
            {"name":"external_timeout_closed","url": format!("http://{}/slow", addr), "timeoutMs": 50, "failOpen": false, "reasonCode": 812}
        ]
    });
    let cfg: sentra::plugins::PluginConfig = serde_json::from_value(cfg_val).unwrap();
    let pipeline =
        sentra::plugins::PluginPipeline::new(&["external_timeout_closed".to_string()], &cfg);
    let req = test_request("anything", "Tool");
    let ctx = sentra::util::EvalContext::from_request(&req, &cfg, 900, 200);
    let (resp, _) = pipeline.evaluate_with_timings(&req, &ctx, &cfg).await;
    assert!(resp.block_action, "fail-closed timeout should block");
    assert_eq!(resp.reason_code, Some(812));
    drop(handle);
}

// Parse error handling: external returns invalid JSON.
#[tokio::test]
async fn external_http_parse_error_fail_open_allows() {
    async fn invalid() -> (axum::http::StatusCode, &'static [u8]) {
        (axum::http::StatusCode::OK, b"{ not json" as &[u8])
    }
    let app = Router::new().route("/bad", post(invalid));
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let cfg_val = serde_json::json!({
        "externalHttp": [
            {"name":"external_parse_open","url": format!("http://{}/bad", addr), "timeoutMs": 200, "failOpen": true, "reasonCode": 830}
        ]
    });
    let cfg: sentra::plugins::PluginConfig = serde_json::from_value(cfg_val).unwrap();
    let pipeline = sentra::plugins::PluginPipeline::new(&["external_parse_open".to_string()], &cfg);
    let req = test_request("whatever", "Tool");
    let ctx = sentra::util::EvalContext::from_request(&req, &cfg, 900, 200);
    let (resp, _) = pipeline.evaluate_with_timings(&req, &ctx, &cfg).await;
    assert!(!resp.block_action, "fail-open parse error should allow");
    drop(handle);
}

#[tokio::test]
async fn external_http_parse_error_fail_closed_blocks() {
    async fn invalid() -> (axum::http::StatusCode, &'static [u8]) {
        (axum::http::StatusCode::OK, b"{ not json" as &[u8])
    }
    let app = Router::new().route("/bad", post(invalid));
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let cfg_val = serde_json::json!({
        "externalHttp": [
            {"name":"external_parse_closed","url": format!("http://{}/bad", addr), "timeoutMs": 200, "failOpen": false, "reasonCode": 831}
        ]
    });
    let cfg: sentra::plugins::PluginConfig = serde_json::from_value(cfg_val).unwrap();
    let pipeline =
        sentra::plugins::PluginPipeline::new(&["external_parse_closed".to_string()], &cfg);
    let req = test_request("whatever", "Tool");
    let ctx = sentra::util::EvalContext::from_request(&req, &cfg, 900, 200);
    let (resp, _) = pipeline.evaluate_with_timings(&req, &ctx, &cfg).await;
    assert!(resp.block_action, "fail-closed parse error should block");
    assert_eq!(resp.reason_code, Some(831));
    drop(handle);
}
