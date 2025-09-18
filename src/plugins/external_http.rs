use super::{Plugin, PluginConfig};
use crate::util::EvalContext;
use crate::{AnalyzeRequest, AnalyzeResponse};

/// Definition for an external HTTP plugin. Kept here so it can remain public while
/// implementation details stay internal to this module.
#[derive(Clone, Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalHttpDefinition {
    pub name: String,
    pub url: String,
    #[serde(default)]
    pub bearer_token: Option<String>,
    #[serde(default = "external_http_default_timeout")]
    pub timeout_ms: u64,
    #[serde(default)]
    pub request_template: Option<String>,
    #[serde(default = "external_http_default_block_field")]
    pub block_field: String,
    #[serde(default = "external_http_default_reason_code")]
    pub reason_code: i32,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default = "external_http_default_fail_open")]
    pub fail_open: bool,
    /// If true and blockField is a JSON pointer, a non-empty array or object at that pointer will be treated as block.
    #[serde(default)]
    pub non_empty_pointer_blocks: bool,
}

fn external_http_default_timeout() -> u64 {
    500
}
fn external_http_default_block_field() -> String {
    "block".to_string()
}
fn external_http_default_reason_code() -> i32 {
    801
}
fn external_http_default_fail_open() -> bool {
    true
}

/// ExternalHttpPlugin performs a POST to an external service using a templated JSON body
/// and interprets a boolean block decision from the response.
pub struct ExternalHttpPlugin {
    def: ExternalHttpDefinition,
    client: reqwest::Client,
}

impl ExternalHttpPlugin {
    pub fn new(def: ExternalHttpDefinition) -> Self {
        let timeout = std::time::Duration::from_millis(def.timeout_ms);
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .expect("failed to build reqwest client");
        Self { def, client }
    }

    fn render_body(&self, req: &AnalyzeRequest) -> String {
        const DEFAULT_TEMPLATE: &str = r#"{
  "userMessage": "${userMessage}",
  "toolName": "${toolName}",
  "input": ${inputJson}
}"#;
        let template = self
            .def
            .request_template
            .as_deref()
            .unwrap_or(DEFAULT_TEMPLATE);

        let user_message_raw = req.planner_context.user_message.as_deref().unwrap_or("");
        let tool_name_raw = req.tool_definition.name.as_deref().unwrap_or("");

        let user_message = escape_json_string(user_message_raw);
        let tool_name = escape_json_string(tool_name_raw);
        let user_message_json =
            serde_json::to_string(user_message_raw).unwrap_or_else(|_| "\"\"".to_string());
        let tool_name_json =
            serde_json::to_string(tool_name_raw).unwrap_or_else(|_| "\"\"".to_string());
        let input_json = serde_json::Value::Object(req.input_values.clone()).to_string();

        let mut rendered = template.replace("${inputJson}", &input_json);
        rendered = rendered.replace("${userMessageJson}", &user_message_json);
        rendered = rendered.replace("${toolNameJson}", &tool_name_json);
        rendered = rendered.replace("${userMessage}", &user_message);
        rendered = rendered.replace("${toolName}", &tool_name);
        rendered
    }

    fn extract_block(&self, val: &serde_json::Value) -> Option<bool> {
        let field = self.def.block_field.as_str();
        if field == "block" {
            if let Some(b) = val.get("block").and_then(|v| v.as_bool()) {
                return Some(b);
            }
        } else if field == "allow" {
            if let Some(a) = val.get("allow").and_then(|v| v.as_bool()) {
                return Some(!a);
            }
        } else if field == "/" {
            // Treat '/' as root for convenience in configs (serde_json uses empty string for root pointer).
            if self.def.non_empty_pointer_blocks {
                match val {
                    serde_json::Value::Array(a) => return Some(!a.is_empty()),
                    serde_json::Value::Object(o) => return Some(!o.is_empty()),
                    serde_json::Value::Bool(b) => return Some(*b),
                    _ => {}
                }
            }
        } else if field.starts_with('/') || field.contains('/') {
            // Treat as JSON pointer (serde_json returns Option)
            if let Some(ptr) = val.pointer(field) {
                if let Some(b) = ptr.as_bool() {
                    return Some(b);
                }
                if self.def.non_empty_pointer_blocks {
                    match ptr {
                        serde_json::Value::Array(a) => return Some(!a.is_empty()),
                        serde_json::Value::Object(o) => return Some(!o.is_empty()),
                        _ => {}
                    }
                }
            }
        }
        None
    }
}

fn escape_json_string(value: &str) -> String {
    match serde_json::to_string(value) {
        Ok(mut json) => {
            if json.len() >= 2 {
                json.remove(0);
                json.pop();
            }
            json
        }
        Err(_) => String::new(),
    }
}

#[async_trait::async_trait]
impl Plugin for ExternalHttpPlugin {
    fn name(&self) -> &str {
        &self.def.name
    }

    async fn eval(
        &self,
        req: &AnalyzeRequest,
        _ctx: &EvalContext,
        _cfg: &PluginConfig,
    ) -> Option<AnalyzeResponse> {
        let body = self.render_body(req);
        let mut rb = self
            .client
            .post(&self.def.url)
            .header("content-type", "application/json");
        if let Some(tok) = &self.def.bearer_token {
            rb = rb.bearer_auth(tok);
        }
        let resp = match rb.body(body).send().await {
            Ok(r) => r,
            Err(err) => {
                if !self.def.fail_open {
                    tracing::warn!(plugin=%self.def.name, error=?err, "external_http network error (fail-closed)");
                    return Some(AnalyzeResponse {
                        block_action: true,
                        reason_code: Some(self.def.reason_code),
                        reason: Some(
                            self.def
                                .reason
                                .clone()
                                .unwrap_or_else(|| "External HTTP error".into()),
                        ),
                        blocked_by: Some(self.def.name.clone()),
                        diagnostics: Some(
                            serde_json::json!({"plugin":"external_http","code":"network_error"}),
                        ),
                    });
                } else {
                    tracing::warn!(plugin=%self.def.name, error=?err, "external_http network error (fail-open)");
                    return None;
                }
            }
        };
        let status = resp.status();
        let text = match resp.text().await {
            Ok(t) => t,
            Err(err) => {
                if !self.def.fail_open {
                    return Some(AnalyzeResponse {
                        block_action: true,
                        reason_code: Some(self.def.reason_code),
                        reason: Some(
                            self.def
                                .reason
                                .clone()
                                .unwrap_or_else(|| "External HTTP read error".into()),
                        ),
                        blocked_by: Some(self.def.name.clone()),
                        diagnostics: Some(
                            serde_json::json!({"plugin":"external_http","code":"read_error"}),
                        ),
                    });
                }
                tracing::warn!(plugin=%self.def.name, error=?err, "external_http read error (fail-open)");
                return None;
            }
        };
        let json: serde_json::Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(err) => {
                if !self.def.fail_open {
                    return Some(AnalyzeResponse {
                        block_action: true,
                        reason_code: Some(self.def.reason_code),
                        reason: Some(
                            self.def
                                .reason
                                .clone()
                                .unwrap_or_else(|| "External HTTP parse error".into()),
                        ),
                        blocked_by: Some(self.def.name.clone()),
                        diagnostics: Some(
                            serde_json::json!({"plugin":"external_http","code":"parse_error","status":status.as_u16()}),
                        ),
                    });
                }
                tracing::warn!(plugin=%self.def.name, error=?err, "external_http parse error (fail-open)");
                return None;
            }
        };
        if let Some(block) = self.extract_block(&json) {
            if block {
                return Some(AnalyzeResponse {
                    block_action: true,
                    reason_code: Some(self.def.reason_code),
                    reason: Some(
                        self.def
                            .reason
                            .clone()
                            .unwrap_or_else(|| "External policy block".into()),
                    ),
                    blocked_by: Some(self.def.name.clone()),
                    diagnostics: Some(
                        serde_json::json!({"plugin":"external_http","code":"block","status":status.as_u16()}),
                    ),
                });
            }
            return None;
        }
        // If block field absent treat as allow
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AnalyzeRequest, PlannerContext, ToolDefinition};
    use serde_json::{json, Map, Value};

    fn make_plugin(template: Option<&str>) -> ExternalHttpPlugin {
        let def = ExternalHttpDefinition {
            name: "external_test".to_string(),
            url: "http://example.com".to_string(),
            bearer_token: None,
            timeout_ms: 500,
            request_template: template.map(|t| t.to_string()),
            block_field: "block".to_string(),
            reason_code: 801,
            reason: None,
            fail_open: true,
            non_empty_pointer_blocks: false,
        };
        ExternalHttpPlugin::new(def)
    }

    fn make_request(user_message: &str, tool_name: &str, input: Value) -> AnalyzeRequest {
        let input_map = match input {
            Value::Object(map) => map,
            other => {
                let mut map = Map::new();
                map.insert("value".to_string(), other);
                map
            }
        };
        AnalyzeRequest {
            planner_context: PlannerContext {
                user_message: Some(user_message.to_string()),
                ..PlannerContext::default()
            },
            tool_definition: ToolDefinition {
                name: Some(tool_name.to_string()),
                ..ToolDefinition::default()
            },
            input_values: input_map,
            ..AnalyzeRequest::default()
        }
    }

    #[test]
    fn render_body_handles_special_characters_in_default_template() {
        let plugin = make_plugin(None);
        let req = make_request(
            "line1\nline2 with \"quotes\" and \\slashes\\",
            "Tool\nName",
            json!({"nested": "value"}),
        );
        let body = plugin.render_body(&req);
        let parsed: Value = serde_json::from_str(&body).expect("body is valid JSON");
        assert_eq!(
            parsed.get("userMessage").and_then(Value::as_str),
            Some("line1\nline2 with \"quotes\" and \\slashes\\")
        );
        assert_eq!(
            parsed.get("toolName").and_then(Value::as_str),
            Some("Tool\nName")
        );
        assert_eq!(parsed.get("input"), Some(&json!({"nested": "value"})));
    }

    #[test]
    fn render_body_supports_json_placeholders() {
        let plugin = make_plugin(Some(
            "{\"msg\": ${userMessageJson}, \"tool\": ${toolNameJson}}",
        ));
        let req = make_request("escape", "Name", json!({}));
        let body = plugin.render_body(&req);
        let parsed: Value = serde_json::from_str(&body).expect("body is valid JSON");
        assert_eq!(parsed.get("msg").and_then(Value::as_str), Some("escape"));
        assert_eq!(parsed.get("tool").and_then(Value::as_str), Some("Name"));
    }
}
