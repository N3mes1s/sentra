use super::{Plugin, PluginConfig};
use crate::util::EvalContext;
use crate::{AnalyzeRequest, AnalyzeResponse};

fn domain_in_text(text: &str, domains: &[String]) -> Option<(String, String)> {
    for domain in domains {
        let mut search_start = 0;
        while let Some(rel) = text[search_start..].find(domain) {
            let abs_start = search_start + rel;
            let abs_end = abs_start + domain.len();

            let before_char = if abs_start == 0 {
                None
            } else {
                text[..abs_start].chars().next_back()
            };
            let after_char = if abs_end >= text.len() {
                None
            } else {
                text[abs_end..].chars().next()
            };

            let before_ok = before_char
                .map(|c| !c.is_ascii_alphanumeric() && c != '-')
                .unwrap_or(true);
            let after_ok = after_char
                .map(|c| !c.is_ascii_alphanumeric() && c != '-')
                .unwrap_or(true);

            if before_ok && after_ok {
                return Some((domain.clone(), format!("pos:{}", abs_start)));
            }

            search_start = abs_end;
        }
    }
    None
}

/// Blocks any request containing substrings from a configured domain
/// blocklist.  Domains should be lower case.  Both URLs and arbitrary
/// strings in `inputValues` are scanned.
#[derive(Default)]
pub struct DomainBlockPlugin;

#[async_trait::async_trait]
impl Plugin for DomainBlockPlugin {
    fn name(&self) -> &str {
        "domain_block"
    }

    async fn eval(
        &self,
        _req: &AnalyzeRequest,
        ctx: &EvalContext,
        cfg: &PluginConfig,
    ) -> Option<AnalyzeResponse> {
        let list = if cfg.domain_blocklist.is_empty() {
            // Use built‑in defaults if no config provided.
            static DEFAULT: &[&str] = &["example.com", "mailinator.com", "tempmail", "evil.com"];
            DEFAULT.iter().map(|s| s.to_string()).collect::<Vec<_>>()
        } else {
            cfg.domain_blocklist.clone()
        };
        // AC matcher no longer required after boundary-aware matching change.
        // Boundary aware domain detection on full text
        if let Some((dom, _loc)) = domain_in_text(&ctx.pre.full_text_lower, &list) {
            let diag = serde_json::json!({"plugin":"domain_block","code":"domain","detail":dom});
            return Some(AnalyzeResponse {
                block_action: true,
                reason_code: Some(113),
                reason: Some("Input contains disallowed domain.".into()),
                blocked_by: Some("domain_block".into()),
                diagnostics: Some(diag),
            });
        }
        for s in &ctx.pre.strings {
            if let Some((dom, _)) = domain_in_text(s, &list) {
                let diag =
                    serde_json::json!({"plugin":"domain_block","code":"domain","detail":dom});
                return Some(AnalyzeResponse {
                    block_action: true,
                    reason_code: Some(113),
                    reason: Some("Input contains disallowed domain.".into()),
                    blocked_by: Some("domain_block".into()),
                    diagnostics: Some(diag),
                });
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::domain_in_text;

    fn domains(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn detects_domain_after_non_ascii_boundary() {
        let text = "привет evil.com";
        let result = domain_in_text(text, &domains(&["evil.com"]));
        assert!(result.is_some());
    }

    #[test]
    fn ignores_embedded_domain_segment() {
        let text = "not blocked: evil.commerce";
        let result = domain_in_text(text, &domains(&["evil.com"]));
        assert!(result.is_none());
    }

    #[test]
    fn handles_unicode_following_character() {
        let text = "visit evil.com✨ now";
        let result = domain_in_text(text, &domains(&["evil.com"]));
        assert!(result.is_some());
    }
}
