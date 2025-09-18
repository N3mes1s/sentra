use super::{Plugin, PluginConfig};
use crate::util::EvalContext;
use crate::{AnalyzeRequest, AnalyzeResponse};
use regex::Regex;
use serde::Deserialize;

/// A user‑defined rule for the policy pack plugin.  A rule can specify
/// which tool and/or argument it applies to, and conditions on the
/// argument or entire text.  If any condition matches the rule blocks the
/// action.  Regular expressions are interpreted as case‑insensitive.
#[derive(Clone, Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PolicyRule {
    /// Optional tool name to restrict rule scope.  Comparison is
    /// case‑insensitive.
    pub tool: Option<String>,
    /// Optional argument key.  If set, the rule operates on the specified
    /// input field.  If not set, conditions are evaluated against the
    /// concatenated text and all inputs.
    pub arg: Option<String>,
    /// A list of substrings.  All entries are lower‑cased.  If any
    /// substring occurs in the target, the rule triggers.
    #[serde(default)]
    pub contains: Vec<String>,
    /// A list of regular expressions.  Regexes are compiled during rule
    /// normalisation.  They are applied case‑insensitively to the target.
    #[serde(default, rename = "regex")] // allow 'regex' in JSON
    pub patterns: Vec<String>,
    /// The reason code returned when the rule triggers.  Defaults to 700.
    pub reason_code: Option<i32>,
    /// A custom reason message.
    pub reason: Option<String>,
}

/// A compiled rule for efficient evaluation.  Conditions are stored
/// lower‑cased and regexes compiled once.
#[derive(Clone)]
struct CompiledRule {
    tool: Option<String>,
    arg: Option<String>,
    contains: Vec<String>,
    regexes: Vec<Regex>,
    reason_code: i32,
    reason: Option<String>,
}

impl From<&PolicyRule> for CompiledRule {
    fn from(r: &PolicyRule) -> Self {
        let mut regexes = Vec::new();
        for pat in &r.patterns {
            // Compile case‑insensitive.  We avoid untrusted regex complexity by
            // falling back to a literal if compilation fails.
            match Regex::new(&format!("(?i){}", pat)) {
                Ok(re) => regexes.push(re),
                Err(err) => {
                    tracing::warn!(pattern = %pat, error = ?err, "failed to compile regex in policy pack, ignoring");
                }
            }
        }
        CompiledRule {
            tool: r.tool.as_ref().map(|s| s.to_lowercase()),
            arg: r.arg.as_ref().map(|s| s.to_lowercase()),
            contains: r.contains.iter().map(|s| s.to_lowercase()).collect(),
            regexes,
            reason_code: r.reason_code.unwrap_or(700),
            reason: r.reason.clone(),
        }
    }
}

/// A plugin that evaluates user‑provided policy rules.  Rules are loaded
/// from the plugin configuration and compiled on construction.
pub struct PolicyPackPlugin {
    rules: Vec<CompiledRule>,
}

impl PolicyPackPlugin {
    pub fn new(rules: Vec<PolicyRule>) -> Self {
        // Safeguards: limit regex/pattern complexity per rule.
        const MAX_PATTERNS: usize = 50;
        const MAX_PATTERN_LEN: usize = 500;
        let mut filtered: Vec<CompiledRule> = Vec::new();
        for r in &rules {
            let mut safe = r.clone();
            if safe.patterns.len() > MAX_PATTERNS {
                tracing::warn!(
                    pattern_count = safe.patterns.len(),
                    limit = MAX_PATTERNS,
                    "policy rule regex list truncated"
                );
                safe.patterns.truncate(MAX_PATTERNS);
            }
            safe.patterns.retain(|p| {
                if p.len() > MAX_PATTERN_LEN {
                    tracing::warn!(
                        len = p.len(),
                        limit = MAX_PATTERN_LEN,
                        "dropping oversized policy regex pattern"
                    );
                    return false;
                }
                true
            });
            filtered.push(CompiledRule::from(&safe));
        }
        Self { rules: filtered }
    }
}

#[async_trait::async_trait]
impl Plugin for PolicyPackPlugin {
    fn name(&self) -> &str {
        "policy_pack"
    }

    async fn eval(
        &self,
        req: &AnalyzeRequest,
        ctx: &EvalContext,
        _cfg: &PluginConfig,
    ) -> Option<AnalyzeResponse> {
        // Evaluate each rule.  Return the first block.
        for rule in &self.rules {
            // Tool match: if rule.tool exists and does not match tool name, skip.
            if let Some(ref tool) = rule.tool {
                let name = req
                    .tool_definition
                    .name
                    .as_deref()
                    .unwrap_or("")
                    .to_lowercase();
                if name != *tool {
                    continue;
                }
            }
            // Determine the target string to test: either a specific argument
            // value or the concatenated input plus chat messages.
            let mut targets: Vec<&str> = Vec::new();
            if let Some(ref arg_name) = rule.arg {
                if let Some(val) = req.input_values.get(arg_name) {
                    if let Some(s) = val.as_str() {
                        targets.push(s);
                    }
                }
            } else {
                targets.push(&ctx.pre.full_text_lower);
                // also scan each input string individually
                for s in &ctx.pre.strings {
                    targets.push(s);
                }
            }
            // Run contains checks
            let mut matched = false;
            for t in &targets {
                let tl = t.to_lowercase();
                // substring match
                for c in &rule.contains {
                    if tl.contains(c) {
                        matched = true;
                        break;
                    }
                }
                if matched {
                    break;
                }
                // regex match
                for re in &rule.regexes {
                    if re.is_match(&tl) {
                        matched = true;
                        break;
                    }
                }
                if matched {
                    break;
                }
            }
            if matched {
                return Some(AnalyzeResponse {
                    block_action: true,
                    reason_code: Some(rule.reason_code),
                    reason: Some(
                        rule.reason
                            .clone()
                            .unwrap_or_else(|| "Policy rule triggered".into()),
                    ),
                    blocked_by: Some("policy_pack".into()),
                    diagnostics: Some(serde_json::json!({"plugin":"policy_pack","code":"policy"})),
                });
            }
        }
        None
    }
}
