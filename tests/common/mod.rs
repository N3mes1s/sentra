use std::collections::HashMap;

/// Tracks environment variable mutations and restores originals on drop.
pub struct EnvGuard {
    originals: HashMap<String, Option<String>>,
}

impl EnvGuard {
    pub fn new() -> Self {
        Self {
            originals: HashMap::new(),
        }
    }

    pub fn set(&mut self, key: &str, value: &str) {
        self.capture(key);
        std::env::set_var(key, value);
    }

    #[allow(dead_code)]
    pub fn set_many(&mut self, entries: &[(&str, &str)]) {
        for (key, value) in entries {
            self.set(key, value);
        }
    }

    #[allow(dead_code)]
    pub fn remove(&mut self, key: &str) {
        self.capture(key);
        std::env::remove_var(key);
    }

    fn capture(&mut self, key: &str) {
        if self.originals.contains_key(key) {
            return;
        }
        let original = std::env::var(key).ok();
        self.originals.insert(key.to_string(), original);
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, original) in self.originals.drain() {
            match original {
                Some(value) => std::env::set_var(&key, value),
                None => std::env::remove_var(&key),
            }
        }
    }
}
