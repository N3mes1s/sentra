use super::{Plugin, PluginConfig};
use crate::util::EvalContext;
use crate::{AnalyzeRequest, AnalyzeResponse};
use serde_json::Value;

/// Blocks email tools with non‑compliant BCC domains.  The allowed
/// domain suffix is read from the plugin configuration via
/// `company_domain`.
#[derive(Default)]
pub struct EmailBccPlugin;

#[async_trait::async_trait]
impl Plugin for EmailBccPlugin {
    fn name(&self) -> &str {
        "email_bcc"
    }

    async fn eval(
        &self,
        req: &AnalyzeRequest,
        _ctx: &EvalContext,
        cfg: &PluginConfig,
    ) -> Option<AnalyzeResponse> {
        // Only examine tools whose name contains "mail" or "email".  Names may
        // be absent in incomplete requests.
        let tool_name = req
            .tool_definition
            .name
            .as_deref()
            .unwrap_or("")
            .to_lowercase();
        if !tool_name.contains("mail") && !tool_name.contains("email") {
            return None;
        }
        // Look for bcc field in inputValues
        if let Some(Value::String(s)) = req.input_values.get("bcc") {
            let addr = s.trim().to_lowercase();
            if !addr.is_empty() {
                // Check if email ends with "@company_domain" (with @ prefix)
                let domain_pattern = format!("@{}", cfg.company_domain);
                if !addr.ends_with(&domain_pattern) {
                    let diag = serde_json::json!({"plugin":"email_bcc","code":"bcc","detail":addr});
                    return Some(AnalyzeResponse {
                        block_action: true,
                        reason_code: Some(112),
                        reason: Some("Noncompliant BCC domain.".into()),
                        blocked_by: Some("email_bcc".into()),
                        diagnostics: Some(diag),
                    });
                }
            }
        }
        None
    }
}
