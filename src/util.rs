//! Utility helpers for Sentra.
//!
//! This module exposes common structures used throughout the service such as
//! precomputed request context, deadline enforcement and shared pattern
//! compilation.  These helpers are deliberately lightweight and avoid
//! external dependencies beyond what is already needed by the main
//! application.

use ahash::AHasher;
use aho_corasick::{AhoCorasick, AhoCorasickBuilder};
use dashmap::DashMap;
use once_cell::sync::Lazy;
use serde_json::Value;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A small structure storing fields extracted from the incoming request to
/// minimise repeated traversals.  All text fields are lower‑cased once to
/// make subsequent substring checks cheaper.
#[derive(Clone, Debug)]
pub struct Precomputed {
    /// Concatenated lower‑cased free‑form text (user message and chat history).
    pub full_text_lower: String,
    /// All leaf string values from `inputValues` in the original JSON. Each entry
    /// is kept as a lower‑cased copy to avoid allocations in hot paths.
    pub strings: Vec<String>,
    /// URLs extracted from strings. Lower‑cased.
    pub urls_lower: Vec<String>,
}

impl Precomputed {
    /// Construct a new `Precomputed` by traversing the user message, chat
    /// history and input values.  All strings are copied and lower‑cased.
    pub fn from_request_message(
        user_message: Option<&str>,
        chat_history: Option<&[serde_json::Value]>,
        input_values: &serde_json::Map<String, Value>,
    ) -> Self {
        let mut full = String::new();
        if let Some(msg) = user_message {
            full.push_str(msg);
            full.push(' ');
        }
        if let Some(history) = chat_history {
            for item in history {
                if let Some(obj) = item.as_object() {
                    if let Some(content) = obj.get("content").and_then(|v| v.as_str()) {
                        full.push_str(content);
                        full.push(' ');
                    }
                }
            }
        }
        let full_text_lower = full.to_lowercase();

        // Gather all string leaves from input values. Also pick up simple URL
        // strings (containing http(s)://) separately for domain checks.
        let mut strings = Vec::new();
        let mut urls_lower = Vec::new();

        fn collect(val: &Value, strings: &mut Vec<String>, urls: &mut Vec<String>) {
            match val {
                Value::String(s) => {
                    let lower = s.to_lowercase();
                    strings.push(lower.clone());
                    // Rough URL detector: look for http:// or https:// or mailto:
                    if lower.contains("http://")
                        || lower.contains("https://")
                        || lower.contains("mailto:")
                    {
                        urls.push(lower);
                    }
                }
                Value::Array(arr) => {
                    for v in arr {
                        collect(v, strings, urls);
                    }
                }
                Value::Object(map) => {
                    for (_k, v) in map {
                        collect(v, strings, urls);
                    }
                }
                _ => {}
            }
        }

        for (_k, v) in input_values {
            collect(v, &mut strings, &mut urls_lower);
        }

        Precomputed {
            full_text_lower,
            strings,
            urls_lower,
        }
    }
}

/// Deadline structure for budgeting plugin execution time.  Calls to
/// `exceeded()` will return true when the specified budget has been
/// exhausted.  A small buffer is reserved automatically for system
/// overhead.
#[derive(Clone, Debug)]
pub struct Deadline {
    start: Instant,
    budget: Duration,
}

impl Deadline {
    /// Construct a new `Deadline` with a given number of milliseconds of
    /// available compute time.  A 100ms safety margin should be left by
    /// callers to allow for network overhead and serialization.
    pub fn new_ms(ms: u64) -> Self {
        Deadline {
            start: Instant::now(),
            budget: Duration::from_millis(ms),
        }
    }

    /// Returns true if the budget has already been exhausted.
    pub fn exceeded(&self) -> bool {
        self.start.elapsed() >= self.budget
    }

    /// Returns the remaining budget in milliseconds.
    pub fn remaining_ms(&self) -> u64 {
        self.budget.saturating_sub(self.start.elapsed()).as_millis() as u64
    }
}

/// A memoising wrapper around `AhoCorasick::new` to avoid recompiling
/// automata for repeated lists.  The cache key is a hash of the pattern list.
static AC_CACHE: Lazy<DashMap<u64, Arc<AhoCorasick>>> = Lazy::new(DashMap::new);

/// Given a list of literal patterns, return a shared `AhoCorasick` matcher.
/// If a matcher for the list already exists in the cache, a cloned Arc is
/// returned.  Otherwise a new matcher is constructed and inserted.  The
/// caller must ensure that the pattern set does not change between calls.
pub fn ac_for(list: &[String]) -> Arc<AhoCorasick> {
    // Compute a stable hash of the pattern list.
    let mut hasher = AHasher::default();
    for pat in list {
        pat.hash(&mut hasher);
    }
    let key = hasher.finish();
    if let Some(existing) = AC_CACHE.get(&key) {
        return existing.clone();
    }
    // Build AC: case insensitive by lower‑casing patterns
    let mut lower = Vec::with_capacity(list.len());
    for p in list {
        lower.push(p.to_lowercase());
    }
    let ac = AhoCorasickBuilder::new()
        .ascii_case_insensitive(true)
        .build(lower)
        .unwrap();
    let arc = Arc::new(ac);
    AC_CACHE.insert(key, arc.clone());
    arc
}

/// Evaluation context provided to each plugin.  Contains immutable
/// precomputed data and runtime flags.  A new context is created per
/// request via `EvalContext::from_request`.
#[derive(Clone, Debug)]
pub struct EvalContext {
    /// Precomputed strings extracted from the request.
    pub pre: Arc<Precomputed>,
    /// Remaining compute budget for plugins.
    pub deadline: Deadline,
    /// Per-plugin warn threshold (ms) for logging slow plugins.
    pub plugin_warn_ms: u64,
}

impl EvalContext {
    /// Construct a new context from the incoming request.  This consumes the
    /// planner context and input values to produce a precomputed structure
    /// that can be shared across plugins.  The evaluation mode and budget are
    /// derived from environment variables.
    pub fn from_request(
        req: &crate::AnalyzeRequest,
        _plugin_config: &crate::plugins::PluginConfig,
        plugin_budget_ms: u64,
        plugin_warn_ms: u64,
    ) -> Self {
        // Build precomputed fields from user message, chat history and input values.
        let pre = Precomputed::from_request_message(
            req.planner_context.user_message.as_deref(),
            req.planner_context.chat_history.as_deref(),
            &req.input_values,
        );
        // Use provided budget (default configured as 900ms) leaving headroom for IO.
        let deadline = Deadline::new_ms(plugin_budget_ms);
        EvalContext {
            pre: Arc::new(pre),
            deadline,
            plugin_warn_ms,
        }
    }
}
