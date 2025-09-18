use std::collections::HashSet;
use std::env;
use std::fs;

use anyhow::{anyhow, Context, Result};

use crate::plugins::{parse_plugin_order, PluginConfig};

#[derive(Debug, Clone)]
pub struct RotationConfig {
    pub max_bytes: Option<u64>,
    pub keep: usize,
    pub compress: bool,
}

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub plugin_config: PluginConfig,
    pub plugin_order: Vec<String>,
    pub log_file: Option<String>,
    pub audit_log_file: Option<String>,
    pub allowed_tokens: Option<HashSet<String>>,
    pub rotation: RotationConfig,
    pub log_stdout: bool,
    pub max_request_bytes: Option<usize>,
    pub plugin_budget_ms: u64,
    pub plugin_warn_ms: u64,
    pub audit_only: bool,
    pub log_sample_n: Option<u64>,
}

impl AppConfig {
    pub fn from_env() -> Result<Self> {
        let plugin_config = if let Ok(path) = env::var("SENTRA_PLUGIN_CONFIG") {
            let content = fs::read_to_string(&path).with_context(|| {
                format!(
                    "Failed to read SENTRA_PLUGIN_CONFIG '{}': file unreadable",
                    path
                )
            })?;
            serde_json::from_str::<PluginConfig>(&content).with_context(|| {
                format!(
                    "Failed to parse SENTRA_PLUGIN_CONFIG '{}': invalid JSON configuration",
                    path
                )
            })?
        } else {
            PluginConfig::default()
        };

        let plugin_order = parse_plugin_order();

        let log_file = env::var("LOG_FILE").ok();
        let audit_log_file = env::var("AUDIT_LOG_FILE").ok();

        let allowed_tokens = env::var("STRICT_AUTH_ALLOWED_TOKENS")
            .ok()
            .map(|raw| {
                raw.split(',')
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string())
                    .collect::<HashSet<_>>()
            })
            .filter(|set: &HashSet<String>| !set.is_empty());

        let rotation = RotationConfig {
            max_bytes: parse_optional_u64("LOG_MAX_BYTES")?,
            keep: parse_optional_u64("LOG_ROTATE_KEEP")?.unwrap_or(1) as usize,
            compress: parse_bool_env("LOG_ROTATE_COMPRESS")?.unwrap_or(false),
        };

        let log_stdout = parse_bool_env("SENTRA_LOG_STDOUT")?.unwrap_or(false);
        let audit_only = parse_bool_env("SENTRA_AUDIT_ONLY")?.unwrap_or(false);
        let max_request_bytes = parse_optional_u64("SENTRA_MAX_REQUEST_BYTES")?.map(|v| v as usize);
        let plugin_budget_ms = parse_optional_u64("SENTRA_PLUGIN_BUDGET_MS")?.unwrap_or(900);
        let plugin_warn_ms = parse_optional_u64("SENTRA_PLUGIN_WARN_MS")?.unwrap_or(120);
        let log_sample_n = parse_optional_u64("SENTRA_LOG_SAMPLE_N")?.filter(|n| *n > 1);

        Ok(Self {
            plugin_config,
            plugin_order,
            log_file,
            audit_log_file,
            allowed_tokens,
            rotation,
            log_stdout,
            max_request_bytes,
            plugin_budget_ms,
            plugin_warn_ms,
            audit_only,
            log_sample_n,
        })
    }
}

fn parse_optional_u64(var: &str) -> Result<Option<u64>> {
    match env::var(var) {
        Ok(value) if !value.trim().is_empty() => value
            .trim()
            .parse::<u64>()
            .map(Some)
            .map_err(|_| anyhow!("{} must be a positive integer", var)),
        Ok(_) => Ok(None),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(err) => Err(err.into()),
    }
}

fn parse_bool_env(var: &str) -> Result<Option<bool>> {
    match env::var(var) {
        Ok(value) if !value.trim().is_empty() => parse_bool(&value)
            .map(Some)
            .ok_or_else(|| anyhow!("{} must be a boolean (true/false/1/0)", var)),
        Ok(_) => Ok(None),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(err) => Err(err.into()),
    }
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use once_cell::sync::Lazy;
    use std::sync::Mutex;
    use tempfile::NamedTempFile;

    static ENV_MUTEX: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

    #[test]
    fn parses_environment_defaults() {
        let _guard = ENV_MUTEX.lock().unwrap();
        std::env::remove_var("SENTRA_PLUGIN_CONFIG");
        std::env::remove_var("SENTRA_PLUGINS");
        std::env::remove_var("STRICT_AUTH_ALLOWED_TOKENS");
        std::env::remove_var("LOG_FILE");
        std::env::remove_var("AUDIT_LOG_FILE");
        std::env::remove_var("LOG_MAX_BYTES");
        std::env::remove_var("LOG_ROTATE_KEEP");
        std::env::remove_var("LOG_ROTATE_COMPRESS");
        std::env::remove_var("SENTRA_LOG_STDOUT");
        std::env::remove_var("SENTRA_MAX_REQUEST_BYTES");
        std::env::remove_var("SENTRA_PLUGIN_BUDGET_MS");
        std::env::remove_var("SENTRA_PLUGIN_WARN_MS");
        std::env::remove_var("SENTRA_AUDIT_ONLY");
        std::env::remove_var("SENTRA_LOG_SAMPLE_N");

        let cfg = AppConfig::from_env().unwrap();
        assert!(cfg.log_file.is_none());
        assert_eq!(cfg.rotation.keep, 1);
        assert!(!cfg.log_stdout);
        assert_eq!(cfg.plugin_budget_ms, 900);
        assert_eq!(cfg.plugin_warn_ms, 120);
    }

    #[test]
    fn parses_full_configuration() {
        let _guard = ENV_MUTEX.lock().unwrap();

        let mut temp = NamedTempFile::new().unwrap();
        let config = serde_json::json!({
            "piiKeywords": ["secret"],
            "domainBlocklist": ["evil.com"],
            "externalHttp": []
        });
        use std::io::Write;
        write!(temp, "{}", config).unwrap();

        std::env::set_var("SENTRA_PLUGIN_CONFIG", temp.path());
        std::env::set_var("SENTRA_PLUGINS", "secrets,pii,external_presidio");
        std::env::set_var("STRICT_AUTH_ALLOWED_TOKENS", "a,b,c");
        std::env::set_var("LOG_FILE", "/tmp/telemetry.log");
        std::env::set_var("AUDIT_LOG_FILE", "/tmp/audit.log");
        std::env::set_var("LOG_MAX_BYTES", "1024");
        std::env::set_var("LOG_ROTATE_KEEP", "5");
        std::env::set_var("LOG_ROTATE_COMPRESS", "true");
        std::env::set_var("SENTRA_LOG_STDOUT", "1");
        std::env::set_var("SENTRA_MAX_REQUEST_BYTES", "2048");
        std::env::set_var("SENTRA_PLUGIN_BUDGET_MS", "750");
        std::env::set_var("SENTRA_PLUGIN_WARN_MS", "90");
        std::env::set_var("SENTRA_AUDIT_ONLY", "true");
        std::env::set_var("SENTRA_LOG_SAMPLE_N", "4");

        let cfg = AppConfig::from_env().unwrap();
        assert_eq!(
            cfg.plugin_order,
            vec!["secrets", "pii", "external_presidio"]
        );
        assert_eq!(cfg.log_file.as_deref(), Some("/tmp/telemetry.log"));
        assert_eq!(cfg.audit_log_file.as_deref(), Some("/tmp/audit.log"));
        assert_eq!(cfg.rotation.max_bytes, Some(1024));
        assert_eq!(cfg.rotation.keep, 5);
        assert!(cfg.rotation.compress);
        assert!(cfg.log_stdout);
        assert_eq!(cfg.max_request_bytes, Some(2048));
        assert_eq!(cfg.plugin_budget_ms, 750);
        assert_eq!(cfg.plugin_warn_ms, 90);
        assert!(cfg.audit_only);
        assert_eq!(cfg.log_sample_n, Some(4));
        let tokens = cfg.allowed_tokens.unwrap();
        assert!(tokens.contains("a") && tokens.contains("b") && tokens.contains("c"));

        std::env::remove_var("SENTRA_PLUGIN_CONFIG");
        std::env::remove_var("SENTRA_PLUGINS");
        std::env::remove_var("STRICT_AUTH_ALLOWED_TOKENS");
        std::env::remove_var("LOG_FILE");
        std::env::remove_var("AUDIT_LOG_FILE");
        std::env::remove_var("LOG_MAX_BYTES");
        std::env::remove_var("LOG_ROTATE_KEEP");
        std::env::remove_var("LOG_ROTATE_COMPRESS");
        std::env::remove_var("SENTRA_LOG_STDOUT");
        std::env::remove_var("SENTRA_MAX_REQUEST_BYTES");
        std::env::remove_var("SENTRA_PLUGIN_BUDGET_MS");
        std::env::remove_var("SENTRA_PLUGIN_WARN_MS");
        std::env::remove_var("SENTRA_AUDIT_ONLY");
        std::env::remove_var("SENTRA_LOG_SAMPLE_N");
    }
}
