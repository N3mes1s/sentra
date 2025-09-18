# Testing Documentation

## Overview

Focus: correctness of blocking decisions, attribution fields, audit-only suppression, telemetry schema & rotation, concurrency integrity (no lost / interleaved lines), and metrics output. Performance is implicitly covered by keeping the path simple; no synthetic micro-benchmarks are maintained.

## Testing Framework

### Built-in Rust Testing
Rust native test harness (`cargo test`) drives all checks. Most coverage is via integration tests invoking real HTTP handlers.

### Key Dev Dependencies
Refer to `Cargo.toml` (not duplicated here).

## Running Tests

### Basic Commands
`cargo test` (parallel by default). Add `-- --nocapture` for verbose output. Use standard filters (e.g. `cargo test rotation`).

### Integration Tests
Located under `tests/`. Key files:
* `sentra_tests.rs` – general blocking/benign cases
* `telemetry_writer.rs` – telemetry enabled/disabled behavior
* `concurrency_stress.rs` – line count integrity under parallel requests
* `rotation.rs` – deterministic rotation (compressed & plain)
* `audit_mode.rs` – audit-only suppression
* `telemetry*.rs` – structured fields & metrics validation

### Performance
No Criterion benchmarks kept; latency assessed with `examples/load_test.rs` if needed.

## Test Structure

### Unit Tests
Minimal; most logic executed through integration flows.

### Integration Tests

Integration tests are located in `tests/sentra_tests.rs`:

```rust
// tests/sentra_tests.rs
use sentra::{Scanner, Config};
use tokio_test;

#[tokio::test]
async fn test_full_scan_workflow() {
    let config = Config::default();
    let scanner = Scanner::new(config).await.unwrap();
    
    let scan_request = ScanRequest {
        target: "http://testphp.vulnweb.com".to_string(),
        plugins: vec!["sql_injection".to_string(), "xss".to_string()],
        timeout: Some(30),
    };
    
    let result = scanner.scan(scan_request).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_server_startup() {
    let config = Config::default();
    let server = sentra::server::Server::new(config).unwrap();
    
    tokio::select! {
        result = server.run() => {
            panic!("Server stopped unexpectedly: {:?}", result);
        }
        _ = tokio::time::sleep(Duration::from_millis(100)) => {
            // Server started successfully
        }
    }
}
```

### Mocks
None required (no external network or DB in core path).

## Test Categories

### Security-Oriented Coverage
Plugins exercise pattern detection and policy pack logic. Concurrency test ensures telemetry durability under load.

### Benchmarks
Not maintained.

### Error Handling
Internal plugin errors are surfaced in logs; tests verify they do not crash or incorrectly block.

## Fixtures

### Test Configuration (Legacy examples trimmed)

```rust
// tests/common/mod.rs
pub fn test_config() -> Config {
    Config {
        server: ServerConfig {
            host: "127.0.0.1".to_string(),
            port: 0, // Use random port for tests
        },
        plugins: PluginConfig {
            enabled: vec!["sql_injection".to_string(), "xss".to_string()],
        },
        scan: ScanConfig {
            timeout: 10,
            max_concurrent: 2,
            user_agent: "Sentra Test".to_string(),
        },
    }
}
```

### Sample Data

```rust
pub struct TestData {
    pub vulnerable_urls: Vec<&'static str>,
    pub safe_urls: Vec<&'static str>,
    pub malicious_payloads: Vec<&'static str>,
}

impl TestData {
    pub fn new() -> Self {
        Self {
            vulnerable_urls: vec![
                "http://testphp.vulnweb.com/listproducts.php?cat=1",
                "http://demo.testfire.net/bank/login.jsp",
            ],
            safe_urls: vec![
                "https://httpbin.org/get",
                "https://example.com",
            ],
            malicious_payloads: vec![
                "' OR '1'='1",
                "<script>alert('xss')</script>",
                "../../../../etc/passwd",
            ],
        }
    }
}
```

## Coverage

### Measuring (Optional)

```bash
# Install cargo-tarpaulin for coverage
cargo install cargo-tarpaulin

# Run tests with coverage
cargo tarpaulin --out Html --output-dir coverage

# Run coverage for specific package
cargo tarpaulin --packages sentra --out Html

# Exclude integration tests from coverage
cargo tarpaulin --skip-clean --ignore-tests
```

### Targets
No numeric threshold enforced; emphasis on critical paths & edge cases (rotation boundaries, concurrency, audit suppression, plugin ordering).

## CI

### Example Workflow

```yaml
name: Tests

on: [push, pull_request]

jobs:
  test:
    runs-on: ubuntu-latest
    steps:
    - uses: actions/checkout@v3
    
    - name: Install Rust
      uses: actions-rs/toolchain@v1
      with:
        toolchain: stable
        components: clippy, rustfmt
    
    - name: Run tests
      run: cargo test --verbose
    
    - name: Run clippy
      run: cargo clippy -- -D warnings
    
    - name: Check formatting
      run: cargo fmt -- --check
    
    - name: Run security audit
      run: |
        cargo install cargo-audit
        cargo audit
    
    - name: Generate coverage
      run: |
        cargo install cargo-tarpaulin
        cargo tarpaulin --out Xml
    
    - name: Upload coverage
      uses: codecov/codecov-action@v3
```

## Environment

### Local

```bash
# Set up test environment
export RUST_LOG=debug
export TEST_MODE=true

# Run tests with debug output
RUST_BACKTRACE=1 cargo test -- --nocapture

# Run tests with specific log level
RUST_LOG=sentra=debug cargo test
```

### Container

```dockerfile
# Dockerfile.test
FROM rust:1.70

WORKDIR /app
COPY . .

RUN cargo test --release
```

```bash
# Build and run tests in container
docker build -f Dockerfile.test -t sentra-test .
docker run sentra-test
```

## Practices

### Organization

1. **Unit Tests**: Test individual functions and methods
2. **Integration Tests**: Test complete workflows
3. **Property Tests**: Test invariants with random data
4. **Regression Tests**: Prevent known issues from recurring

### Quality

- **Clear Test Names**: Descriptive test function names
- **Isolated Tests**: Each test should be independent
- **Deterministic Tests**: Tests should produce consistent results
- **Fast Execution**: Unit tests should run quickly

### Error Testing

- **Happy Path**: Test successful operations
- **Error Paths**: Test all error conditions
- **Edge Cases**: Test boundary conditions
- **Recovery**: Test error recovery mechanisms

### Security Testing

- **Input Validation**: Test all input validation
- **Authentication**: Test auth success and failure
- **Authorization**: Test permission checks
- **Rate Limiting**: Test abuse prevention