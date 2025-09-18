use super::{Plugin, PluginConfig};
use crate::util::EvalContext;
use crate::{AnalyzeRequest, AnalyzeResponse};
use once_cell::sync::Lazy;
use regex::Regex;

#[derive(Default)]
pub struct SecretsPlugin;

static AWS_KEY_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)akia[0-9a-z]{14,20}").unwrap());

#[async_trait::async_trait]
impl Plugin for SecretsPlugin {
    fn name(&self) -> &str {
        "secrets"
    }

    async fn eval(
        &self,
        _req: &AnalyzeRequest,
        ctx: &EvalContext,
        _cfg: &PluginConfig,
    ) -> Option<AnalyzeResponse> {
        let hay = &ctx.pre.full_text_lower;
        if AWS_KEY_RE.is_match(hay) {
            let diag = serde_json::json!({"plugin":"secrets","code":"aws_key"});
            return Some(AnalyzeResponse {
                block_action: true,
                reason_code: Some(201),
                reason: Some(String::from("Detected AWS key")),
                blocked_by: Some("secrets".into()),
                diagnostics: Some(diag),
            });
        }

        for s in &ctx.pre.strings {
            if AWS_KEY_RE.is_match(s) {
                let diag = serde_json::json!({"plugin":"secrets","code":"aws_key"});
                return Some(AnalyzeResponse {
                    block_action: true,
                    reason_code: Some(201),
                    reason: Some(String::from("Detected AWS key")),
                    blocked_by: Some("secrets".into()),
                    diagnostics: Some(diag),
                });
            }
        }
        None
    }
}
