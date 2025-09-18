use once_cell::sync::Lazy;
use sentra::AppConfig;
use std::sync::Mutex;

static ENV_MUTEX: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

#[test]
fn rejects_invalid_boolean_env_values() {
    let _guard = ENV_MUTEX.lock().unwrap();
    std::env::remove_var("LOG_ROTATE_COMPRESS");
    std::env::remove_var("LOG_MAX_BYTES");
    std::env::remove_var("LOG_ROTATE_KEEP");

    std::env::set_var("LOG_ROTATE_COMPRESS", "maybe");
    let err = AppConfig::from_env().expect_err("expected invalid boolean to error");
    assert!(format!("{}", err).contains("LOG_ROTATE_COMPRESS"));
    std::env::remove_var("LOG_ROTATE_COMPRESS");
}
