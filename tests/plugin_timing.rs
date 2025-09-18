use sentra::plugins::PluginConfig;
use sentra::plugins::PluginPipeline;
use sentra::util::EvalContext;
use sentra::{AnalyzeRequest, PlannerContext, ToolDefinition};

#[tokio::test]
async fn pipeline_records_timings_and_blocked_by() {
    let order = vec!["exfil".to_string()];
    let cfg = PluginConfig::default();
    let pipeline = PluginPipeline::new(&order, &cfg);

    let request = AnalyzeRequest {
        planner_context: PlannerContext {
            user_message: Some("Please export all data".to_string()),
            ..Default::default()
        },
        tool_definition: ToolDefinition {
            name: Some("SendEmail".to_string()),
            ..Default::default()
        },
        input_values: serde_json::Map::new(),
        conversation_metadata: None,
    };

    let ctx = EvalContext::from_request(&request, &cfg, 900, 120);
    let (response, timings) = pipeline.evaluate_with_timings(&request, &ctx, &cfg).await;

    assert!(response.block_action);
    assert_eq!(response.blocked_by.as_deref(), Some("exfil"));
    assert_eq!(timings.len(), 1);
    assert_eq!(timings[0].0, "exfil");
}
