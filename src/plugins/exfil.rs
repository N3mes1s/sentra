use super::{Plugin, PluginConfig};
use crate::util::EvalContext;
use crate::{AnalyzeRequest, AnalyzeResponse};
use once_cell::sync::Lazy;

/// Detects prompt injection and data exfiltration patterns in the user message.
#[derive(Default)]
pub struct ExfilPlugin;

// Static list of lower‑cased substrings that, if present in user text, imply
// malicious prompt injection or data exfiltration.  These patterns are
// intentionally simple to keep evaluation cost minimal.
static EXFIL_PATTERNS: Lazy<Vec<&'static str>> = Lazy::new(|| {
    vec![
        "export all data",
        "ignore previous instructions",
        "reveal secrets",
        "print the system prompt",
        "exfiltrate",
    ]
});

#[async_trait::async_trait]
impl Plugin for ExfilPlugin {
    fn name(&self) -> &str {
        "exfil"
    }

    async fn eval(
        &self,
        _req: &AnalyzeRequest,
        ctx: &EvalContext,
        _cfg: &PluginConfig,
    ) -> Option<AnalyzeResponse> {
        let hay = &ctx.pre.full_text_lower;
        for pat in EXFIL_PATTERNS.iter() {
            if hay.contains(*pat) {
                return Some(AnalyzeResponse {
                    block_action: true,
                    reason_code: Some(111),
                    reason: Some("Detected data exfiltration pattern".into()),
                    blocked_by: Some("exfil".into()),
                    diagnostics: Some(
                        serde_json::json!({"plugin":"exfil","code":"pattern","detail":pat}),
                    ),
                });
            }
        }
        None
    }
}
