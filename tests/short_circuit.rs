use reqwest::Client;
use tokio::net::TcpListener as TokioTcpListener;
use tokio::task::JoinHandle;

use sentra::{app, build_state_from_env};

async fn spawn_app() -> (String, JoinHandle<()>) {
    let listener = TokioTcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let state = build_state_from_env().await.unwrap();
    let app = app(state);
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{}", addr), handle)
}

// This test asserts that a blocking early plugin (secrets) stops evaluation
// before later plugins (can't directly observe skipped timings in response,
// but we can assert the reasonCode and that diagnostics originate from secrets only).
#[tokio::test]
async fn short_circuits_after_first_blocking_plugin() {
    std::env::set_var("SENTRA_PLUGINS", "secrets,pii,exfil,email_bcc,domain_block");
    let (addr, _h) = spawn_app().await;
    let url = format!("{}/analyze-tool-execution?api-version=2025-05-01", addr);
    // This input should trip the secrets plugin (AWS key pattern) immediately.
    let body = serde_json::json!({
        "plannerContext": { "userMessage": "Leaking key AKIAABCDEF1234567890 now" },
        "toolDefinition": { "name": "SendEmail" },
        "inputValues": { "to": "alice@yourcompany.com" }
    });
    let resp = Client::new()
        .post(&url)
        .header("Authorization", "Bearer test")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let v: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(v.get("blockAction").unwrap(), &serde_json::json!(true));
    assert_eq!(v.get("reasonCode").unwrap(), &serde_json::json!(201));
    // diagnostics is now a structured object and blockedBy should indicate the plugin
    assert_eq!(v.get("blockedBy").and_then(|b| b.as_str()), Some("secrets"));
    let parsed = v.get("diagnostics").expect("diagnostics present");
    assert!(parsed.is_object(), "diagnostics should be object now");
    assert_eq!(
        parsed.get("plugin").and_then(|p| p.as_str()),
        Some("secrets")
    );
    std::env::remove_var("SENTRA_PLUGINS");
}

// Benign request should pass through with blockAction false and null diagnostics/reasonCode.
#[tokio::test]
async fn benign_request_passes_without_block() {
    std::env::set_var("SENTRA_PLUGINS", "secrets,pii,exfil");
    let (addr, _h) = spawn_app().await;
    let url = format!("{}/analyze-tool-execution?api-version=2025-05-01", addr);
    let body = serde_json::json!({
        "plannerContext": { "userMessage": "Schedule a team meeting" },
        "toolDefinition": { "name": "CalendarAdd" },
        "inputValues": { "title": "Sync" }
    });
    let resp = Client::new()
        .post(&url)
        .header("Authorization", "Bearer test")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let v: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(v.get("blockAction").unwrap(), &serde_json::json!(false));
    assert!(v.get("reasonCode").is_none() || v.get("reasonCode").unwrap().is_null());
    match v.get("diagnostics") {
        None => {} // Accept missing diagnostics field (should be null but tolerate omission)
        Some(val) => assert!(
            val.is_null(),
            "diagnostics should be null when benign, got: {val}"
        ),
    }
    assert!(v.get("blockedBy").is_none() || v.get("blockedBy").unwrap().is_null());
    std::env::remove_var("SENTRA_PLUGINS");
}
