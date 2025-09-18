use axum::{body::Body, http::Request};
use http_body_util::BodyExt;
use std::fs;
use std::time::Duration;
use tower::ServiceExt;

use sentra::{app, build_state_from_env};

#[tokio::test]
async fn audit_mode_never_blocks_but_logs_would_block() {
    // Force secrets first for deterministic block condition
    std::env::set_var("SENTRA_PLUGINS", "secrets,pii");
    std::env::set_var("SENTRA_AUDIT_ONLY", "true");
    let audit_path = std::env::temp_dir().join(format!(
        "sentra_audit_{}.log",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::env::set_var("AUDIT_LOG_FILE", &audit_path);

    let body = serde_json::json!({
        "plannerContext": { "userMessage": "Leaking key AKIAABCDEF1234567890 now" },
        "toolDefinition": { "name": "SendEmail" },
        "inputValues": { "to": "bob@yourcompany.com" }
    });
    let state = build_state_from_env().await.unwrap();
    let app = app(state);
    let request = Request::builder()
        .method("POST")
        .uri("/analyze-tool-execution?api-version=2025-05-01")
        .header("Authorization", "Bearer test")
        .header("Content-Type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), hyper::StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    // Should NOT block outwardly
    assert_eq!(v.get("blockAction").unwrap(), &serde_json::json!(false));
    assert!(v.get("reasonCode").is_none());

    // Give a moment for file write
    tokio::time::sleep(Duration::from_millis(50)).await;
    let content = fs::read_to_string(&audit_path).expect("audit log readable");
    assert!(!content.is_empty());
    // Find a line that includes wouldBlock true
    let mut found = false;
    for line in content.lines() {
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(line) {
            if val.get("auditOnly").and_then(|b| b.as_bool()) == Some(true)
                && val.get("wouldBlock").and_then(|b| b.as_bool()) == Some(true)
            {
                // Validate nested wouldResponse shows the original blocking plugin
                let w = val.get("wouldResponse").expect("wouldResponse");
                assert_eq!(w.get("blockAction").and_then(|b| b.as_bool()), Some(true));
                assert_eq!(w.get("blockedBy").and_then(|b| b.as_str()), Some("secrets"));
                found = true;
                break;
            }
        }
    }
    assert!(found, "expected audit record with wouldBlock=true");

    std::env::remove_var("SENTRA_PLUGINS");
    std::env::remove_var("SENTRA_AUDIT_ONLY");
    std::env::remove_var("AUDIT_LOG_FILE");
    let _ = fs::remove_file(audit_path);
}
