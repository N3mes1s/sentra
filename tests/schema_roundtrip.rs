use sentra::{AnalyzeRequest, AnalyzeResponse};

#[test]
fn serialize_analyze_response_camel_case() {
    let resp = AnalyzeResponse {
        block_action: true,
        reason_code: Some(112),
        reason: Some("Blocked".into()),
        blocked_by: Some("email_bcc".into()),
        diagnostics: Some(serde_json::json!({"flaggedField":"bcc"})),
    };
    let json = serde_json::to_string(&resp).unwrap();
    // Ensure camelCase keys appear
    assert!(json.contains("\"blockAction\""));
    assert!(json.contains("\"reasonCode\""));
}

#[test]
fn deserialize_analyze_request_camel_case() {
    let raw = r#"{
        "plannerContext": { "userMessage": "Hi" },
        "toolDefinition": { "name": "SendEmail" },
        "inputValues": { "to": "alice@example.com" }
    }"#;
    let req: AnalyzeRequest = serde_json::from_str(raw).expect("deserialize");
    assert_eq!(req.planner_context.user_message.as_deref(), Some("Hi"));
    assert_eq!(req.tool_definition.name.as_deref(), Some("SendEmail"));
    assert!(req.input_values.get("to").is_some());
}
