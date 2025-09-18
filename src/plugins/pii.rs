use super::{Plugin, PluginConfig};
use crate::util::{ac_for, EvalContext};
use crate::{AnalyzeRequest, AnalyzeResponse};
use once_cell::sync::Lazy;
use regex::Regex;

/// Detects personally identifiable information such as email addresses, IBANs
/// and phone numbers.  Additional keywords can be configured via
/// `pii_keywords` in `PluginConfig`.  If any match is found the action is
/// blocked.
#[derive(Default)]
pub struct PiiPlugin;

static EMAIL_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"[a-zA-Z0-9_.+-]+@[a-zA-Z0-9-]+\.[a-zA-Z0-9-.]+").unwrap());
static IBAN_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\b[A-Z]{2}\d{2}[A-Z0-9]{10,30}\b").unwrap());
static PHONE_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"\+?\d{1,3}[\s.-]?\(?(?:\d{1,4})\)?[\s.-]?\d{3,}[\s.-]?\d{3,}").unwrap()
});

impl PiiPlugin {
    /// Check if text contains email addresses that are NOT from the company domain
    fn contains_non_company_pii(&self, text: &str, cfg: &PluginConfig) -> bool {
        let domain_pattern = format!("@{}", cfg.company_domain);
        for m in EMAIL_RE.find_iter(text) {
            let email = m.as_str().to_lowercase();
            if !email.ends_with(&domain_pattern) {
                return true;
            }
        }
        false
    }
}

#[async_trait::async_trait]
impl Plugin for PiiPlugin {
    fn name(&self) -> &str {
        "pii"
    }

    async fn eval(
        &self,
        _req: &AnalyzeRequest,
        ctx: &EvalContext,
        cfg: &PluginConfig,
    ) -> Option<AnalyzeResponse> {
        // Check built‑in patterns on the flattened text
        let hay = &ctx.pre.full_text_lower;
        if self.contains_non_company_pii(hay, cfg)
            || IBAN_RE.is_match(hay)
            || PHONE_RE.is_match(hay)
        {
            let diag = serde_json::json!({"plugin":"pii","code":"builtin"});
            return Some(AnalyzeResponse {
                block_action: true,
                reason_code: Some(202),
                reason: Some("Detected potential PII in content.".into()),
                blocked_by: Some("pii".into()),
                diagnostics: Some(diag),
            });
        }
        // Check AC keyword list if configured
        if !cfg.pii_keywords.is_empty() {
            let ac = ac_for(&cfg.pii_keywords);
            if ac.is_match(hay) {
                let diag = serde_json::json!({"plugin":"pii","code":"keyword"});
                return Some(AnalyzeResponse {
                    block_action: true,
                    reason_code: Some(202),
                    reason: Some("Detected potential PII in content.".into()),
                    blocked_by: Some("pii".into()),
                    diagnostics: Some(diag),
                });
            }
        }
        // Check each input string individually for PII patterns
        for s in &ctx.pre.strings {
            if self.contains_non_company_pii(s, cfg) || IBAN_RE.is_match(s) || PHONE_RE.is_match(s)
            {
                let diag = serde_json::json!({"plugin":"pii","code":"input"});
                return Some(AnalyzeResponse {
                    block_action: true,
                    reason_code: Some(202),
                    reason: Some("Detected potential PII in content.".into()),
                    blocked_by: Some("pii".into()),
                    diagnostics: Some(diag),
                });
            }
            if !cfg.pii_keywords.is_empty() {
                let ac = ac_for(&cfg.pii_keywords);
                if ac.is_match(s) {
                    let diag = serde_json::json!({"plugin":"pii","code":"keyword"});
                    return Some(AnalyzeResponse {
                        block_action: true,
                        reason_code: Some(202),
                        reason: Some("Detected potential PII in content.".into()),
                        blocked_by: Some("pii".into()),
                        diagnostics: Some(diag),
                    });
                }
            }
        }
        None
    }
}
