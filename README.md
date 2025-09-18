# Sentra

Minimal, fast request evaluation service implementing the Microsoft Copilot Studio External Security Webhooks interface with a small, ordered plugin pipeline. First blocking plugin wins; otherwise the request is allowed. Vibecoded with love.

Goals:
* Deterministic, low‑latency evaluation (single in‑process loop)
* Transparent reasoning (`blockedBy` + structured `diagnostics`)
* Operational visibility (JSONL telemetry, Prometheus metrics, rotation)
* Safe rollout (audit‑only dry run mode)

## Specification Reference

This implementation aligns its external JSON request/response contract (field names, endpoint semantics, and versioning style) with the official Microsoft Copilot Studio External Security Webhooks specification:

https://learn.microsoft.com/en-us/microsoft-copilot-studio/external-security-webhooks-interface-developers

Key schema elements sourced from that spec:
- Validation endpoint: `POST /validate?api-version=2025-05-01` returning `isSuccessful`, `status` (camelCase)
- Evaluation endpoint: `POST /analyze-tool-execution?api-version=2025-05-01`
- Request payload objects: `plannerContext`, `toolDefinition`, `inputValues`, `conversationMetadata`
- Response fields: `blockAction`, `reasonCode`, `reason`, `diagnostics`, and error structure (`errorCode`, `message`, `httpStatus`, `diagnostics`)

Internal Rust struct fields use idiomatic `snake_case`; serde `rename_all = "camelCase"` ensures wire compatibility without leaking internal naming conventions.

## API Specification & Compatibility

An OpenAPI 3.0 definition is provided in `openapi.yaml` and mirrors the official Microsoft specification. Key compatibility notes:

1. Casing Strategy: All wire fields are camelCase per spec; internal code stays snake_case.
2. Forward Compatibility: The service ignores unknown JSON fields to remain resilient to non-breaking upstream additions.
3. Versioning: Current supported `api-version` is `2025-05-01`. Additional versions will be additive; older versions may continue to map to the latest behavior unless a breaking change requires branching.
4. Error Contract Stability: `errorCode`, `message`, `httpStatus`, and optional `diagnostics` are considered stable; new optional fields may appear but existing semantics will not regress.
5. Deterministic Serialization: Optional fields (`reasonCode`, `reason`, `diagnostics`) are omitted when `null` to keep payloads minimal.

### Migration Guidance

If you previously consumed an experimental snake_case variant (internal development builds), migrate by:
* Renaming request fields to camelCase: planner_context -> plannerContext, tool_definition -> toolDefinition, input_values -> inputValues, conversation_metadata -> conversationMetadata.
* Renaming response assertions: block_action -> blockAction, reason_code -> reasonCode, error_code -> errorCode, http_status -> httpStatus.
* Validating that clients treat unknown fields as ignorable.

### Validation & Testing

Schema conformance is enforced with:
* Integration tests under `tests/` exercising real HTTP endpoints.
* (Planned) Round‑trip serde tests to ensure model <-> JSON stability.
* The published `openapi.yaml` can be used with tools like `swagger-cli` or `spectral` for contract linting.

## Plugins (Current Set)

| Plugin | Purpose | Example Block Reason |
|--------|---------|----------------------|
| `secrets` | Detect credential formats (e.g. AWS key) | `Detected AWS key` |
| `pii` | Simple PII patterns (phone, IBAN, etc.) | `Detected phone number` |
| `email_bcc` | Enforce outbound email policy (e.g. company domain) | `External email without BCC` |
| `domain_block` | Blocklisted domain substrings | `Blocked domain` |
| `exfil` | Basic exfiltration patterns | `Exfil pattern` |
| `policy_pack` | Config‑driven substring rules per tool argument | `Policy: restricted subject keyword` |
| `external_*` | Outbound HTTP decision hook (custom service) | `External policy block` |

Order is defined by `SENTRA_PLUGINS` and determines short‑circuit priority.

## Quick Start

### Build & Run (Rust)

```bash
git clone <repository-url>
cd sentra
cargo build --release
./target/release/sentra
```

### Run Tests

```bash
cargo test
```

### Core Environment Variables

| Variable | Purpose |
|----------|---------|
| `SENTRA_PLUGINS` | Comma list controlling ordered evaluation. |
| `SENTRA_PLUGIN_CONFIG` | Optional JSON file for policy pack & domain / keyword data. |
| `LOG_FILE` | JSONL telemetry output path (one line per analyze request). |
| `AUDIT_LOG_FILE` | Separate audit log path (falls back to `LOG_FILE`). |
| `SENTRA_AUDIT_ONLY` | If `1`/`true`, never block; record would‑block lines + mark normal telemetry with `auditSuppressed`. |
| `STRICT_AUTH_ALLOWED_TOKENS` | Comma allowlist of bearer tokens (enables strict auth). |
| `SENTRA_MAX_REQUEST_BYTES` | Hard reject payloads above this size (413 / `errorCode=4001`). |
| `SENTRA_PLUGIN_BUDGET_MS` | Soft total evaluation budget (deadline inside loop). |
| `SENTRA_PLUGIN_WARN_MS` | Warn log threshold per plugin execution. |
| `SENTRA_LOG_STDOUT` | Mirror each telemetry / audit line to stdout when set. |
| `SENTRA_LOG_SAMPLE_N` | When set to an integer > 1 and used with `SENTRA_LOG_STDOUT`, only every Nth telemetry / audit line is mirrored to stdout (file logs remain complete). |
| `LOG_MAX_BYTES` | Rotate active log once size >= bytes. |
| `LOG_ROTATE_KEEP` | Number of rotated backups retained. |
| `LOG_ROTATE_COMPRESS` | If set, gzip compress newest rotated file (`.1.gz`). |

`SENTRA_PLUGIN_CONFIG` schema: see `examples/policy_config_example.json`.

## Decision & Diagnostics

Response fields (subset of the Copilot spec):
| Field | Meaning |
|-------|---------|
| `blockAction` | Boolean final decision. |
| `reasonCode` | Numeric plugin / policy specific code (nullable). |
| `blockedBy` | Plugin name when blocked. Null otherwise. |
| `diagnostics` | Structured JSON object (plugin‑specific) when blocked. |

Clients should ignore unknown additional top‑level fields for forward compatibility.

## Telemetry & Attribution

If `LOG_FILE` is set, every evaluation appends a JSON object (one per line) containing decision, timings, attribution, and version:

```json
{
    "blockAction": true,
    "reasonCode": 201,
    "reason": "Detected AWS key",
    "blockedBy": "secrets",
    "diagnostics": { "plugin": "secrets", "code": "aws_key" }
}
```

Notes:
* Stable `schemaVersion` (currently `1`).
* `pluginTimings` lists `{plugin, ms}` entries for executed plugins in order.
* `auditSuppressed=true` only when audit mode masked a block.

### Log Rotation

If `LOG_MAX_BYTES` is set, rotation occurs before writing when the active file size >= limit:
* Active becomes `.1` (existing backups shifted up to `.N` respecting `LOG_ROTATE_KEEP`).
* If compression enabled, newly rotated `.1` becomes `.1.gz`.
* Writes then continue on a fresh active file.

```json
{
    "schemaVersion": 1,
    "ts": "2025-09-13T12:34:56Z",
    "correlationId": "corr-123",
    "blockAction": true,
    "reasonCode": 201,
    "blockedBy": "secrets",
    "latencyMs": 42,
    "diagnostics": { "plugin": "secrets", "code": "aws_key" },
    "pluginTimings": [ { "plugin": "secrets", "ms": 1 }, { "plugin": "exfil", "ms": 0 } ],
    "auditSuppressed": true
}
```

Set `SENTRA_LOG_STDOUT=1` to mirror each telemetry / audit line to stdout (helpful when also shipping files). To reduce volume, set `SENTRA_LOG_SAMPLE_N` to an integer >1 (e.g. `SENTRA_LOG_SAMPLE_N=10`) which will emit only every 10th line to stdout while still writing all lines to the underlying log files.

### Audit-Only Mode

`SENTRA_AUDIT_ONLY=1` forces all external responses to benign while still evaluating plugins. When a plugin would have blocked:

```json
{
    "ts": "2025-09-13T12:34:56Z",
    "auditOnly": true,
    "wouldBlock": true,
    "wouldResponse": { "blockAction": true, "reasonCode": 201, "blockedBy": "secrets", "diagnostics": {"plugin":"secrets","code":"aws_key"} },
    "request": { "plannerContext": { "userMessage": "..." }, "toolDefinition": {"name": "SendEmail"}, "inputValues": {"to":"bob@yourcompany.com"} }
}
```

Normal telemetry line carries `auditSuppressed: true`. Use this for staged rollout.

## Strict Auth

If `STRICT_AUTH_ALLOWED_TOKENS` is set only those bearer tokens are accepted (comma separated). Missing or disallowed token → 401 (`errorCode=2001`).

## Version Policy

Unknown `api-version` values are accepted (logged) for forward compatibility; missing version => 400 (`errorCode=4000`).

### Error Codes (current)
| Code | Meaning |
|------|---------|
| 2001 | Unauthorized (token not allowed) |
| 4000 | Missing required `api-version` query param |
| 4001 | Payload too large |

## Health Endpoint

`GET /healthz` returns JSON for liveness/readiness:

```json
{ "status": "ok", "version": "2025-05-01", "pluginCount": 6, "budgetMs": 900 }
```

Use in container probes: `httpGet: path: /healthz`.

## Metrics Endpoint

`GET /metrics` exposes Prometheus counters, latency histogram, and build info gauge. See `METRICS.md` for detailed semantics and usage guidance.

Counters:
```
sentra_requests_total
sentra_blocks_total
sentra_audit_suppressed_total
sentra_telemetry_lines_total
```

Latency histogram (milliseconds): `sentra_request_latency_ms_bucket{le="..."}` with bucket bounds:
```
[1,2,5,10,20,50,100,200,500,1000,2000,+Inf]
```
plus `_sum` (sum of ms) and `_count`.

Build info gauge:
```
sentra_build_info{version="<crate-version>",schemaVersion="1"} 1
```

Example scrape job (Prometheus):
```
- job_name: sentra
    static_configs:
        - targets: ['sentra:8080']
    metrics_path: /metrics
```

## Examples

* `agent_crescendo` (example binary): simulates an escalating sequence of tool executions (benign -> multiple policy violations) to validate plugin detection ordering and blocking behavior.

## Available Plugins

| Plugin Name | Type | Purpose / Behavior | Key Config (JSON fields in `PluginConfig` or external def) | Block Condition | Default Reason Code |
|-------------|------|--------------------|-------------------------------------------------------------|------------------|---------------------|
| `exfil` | Built-in | Detect simple prompt-injection / exfiltration phrases (ignore instructions, export data, reveal secrets, etc.) | None (pattern list compiled in code) | Any phrase match | 111 |
| `secrets` | Built-in | Detect obvious AWS access key patterns | None | Regex match of AWS key | 201 |
| `pii` | Built-in | Detect email (non-company), IBAN, phone, plus optional keyword list | `pii_keywords`, `company_domain` | Any pattern or keyword match | 202 |
| `email_bcc` | Built-in | Enforce BCC address ends with company domain for mail tools | `company_domain` | BCC present and not ending with `@<company_domain>` | 112 |
| `domain_block` | Built-in | Block presence of disallowed domains in concatenated text or inputs | `domain_blocklist` (falls back to internal defaults if empty) | Domain token boundary match | 113 |
| `policy_pack` | Built-in | User-defined policy rules (substring / regex over tool or arg) | `policies[]`: tool, arg, contains[], regex[], reasonCode, reason | Any rule condition match | Rule-specific (default 700) |
| `external_*` | External HTTP | Call external policy service via POST and interpret response | `external_http[]` entries: name, url, timeoutMs, requestTemplate, blockField, reasonCode, reason, failOpen, nonEmptyPointerBlocks | Evaluated block field (bool or non-empty pointer/array/object) | Per definition (`reasonCode`, default 801) |

Notes:
* External HTTP plugin instances are registered by including their `name` (prefixed `external_`) in `SENTRA_PLUGINS` ordering.
* `nonEmptyPointerBlocks` enables structural blocking when the JSON pointer targets an array/object (used for the direct Presidio analyzer root array output).
* If an external plugin errors and `failOpen` is true, the error is logged and evaluation continues; if false the plugin blocks with its `reasonCode`.

---

`examples/load_test.rs` – simple concurrency load generator.
`examples/presidio/presidio_real_plugin_config.json` + `examples/presidio/presidio_real.md` – production-style direct Presidio analyzer integration (no wrapper required).

### Direct Presidio Analyzer E2E

Use the official `presidio-analyzer` container directly. Resources:

* Compose: `docker-compose.presidio.real.yml`
* Config: `examples/presidio/presidio_real_plugin_config.json`
* Guide: `examples/presidio/presidio_real.md`
* E2E script: `scripts/run_real_presidio_e2e.sh`

Blocking semantics: a non‑empty root JSON array (`blockField: "/"` + `nonEmptyPointerBlocks: true`) => block with `reasonCode` 860.

Run:

```bash
./scripts/run_real_presidio_e2e.sh
```

Success criteria (summary): at least 2 requests, >=1 block. Telemetry log: `telemetry_data/telemetry.log`.


## Policy Pack Example

An example policy configuration is provided at `examples/policy_config_example.json`:

```json
{
    "policies": [
        { "tool": "sendemail", "arg": "subject", "contains": ["confidential"], "reasonCode": 750, "reason": "Policy: restricted subject keyword" }
    ]
}
```

Use it by setting:

```bash
export SENTRA_PLUGIN_CONFIG=examples/policy_config_example.json
export SENTRA_PLUGINS=policy_pack,exfil,secrets,pii,email_bcc,domain_block
```

Only rules whose `tool` matches (case-insensitive) and whose `contains` substring appears in the specified argument value will block.

## External HTTP Plugins

Sentra can call out to external HTTP services as ordered plugins. Each external definition in `SENTRA_PLUGIN_CONFIG` produces a distinct plugin identified by its `name` and must be explicitly placed in `SENTRA_PLUGINS`.

### Configuration Schema (excerpt)

```jsonc
{
    "externalHttp": [
        {
            "name": "external_presidio",          // referenced in SENTRA_PLUGINS
            "url": "https://scanner.internal/api/eval", // POST endpoint
            "timeoutMs": 500,                      // optional (default 500)
            "bearerToken": "secret-token",        // optional static auth
            "requestTemplate": "{\n  \"userMessage\": \"${userMessage}\",\n  \"toolName\": \"${toolName}\",\n  \"input\": ${inputJson}\n}",
            "blockField": "block",                // "block", "allow", or JSON Pointer e.g. "/decision/block"
            "reasonCode": 801,                     // default 801 if omitted
            "reason": "External PII policy",      // optional
            "failOpen": true                       // default true; false => network/parse errors block
        }
    ],
    "policies": [],
    "piiKeywords": [],
    "domainBlocklist": []
}
```

Placeholders supported inside `requestTemplate`:
* `${userMessage}` – `plannerContext.userMessage`
* `${toolName}` – `toolDefinition.name`
* `${inputJson}` – Raw JSON object string of `inputValues`

If `requestTemplate` is omitted the minimal default body is used:

```json
{
    "userMessage": "${userMessage}",
    "toolName": "${toolName}",
    "input": ${inputJson}
}
```

`blockField` interpretation:
* `"block"` (default): look for top-level boolean field `block`.
* `"allow"`: invert top-level boolean `allow` -> block when `allow=false`.
* JSON Pointer (e.g. `/decision/block`): traverse and interpret boolean at pointer location.

Errors & timeouts:
* When `failOpen=true` (default) network / timeout / parse errors are logged and treated as allow.
* When `failOpen=false` the plugin returns a block with the configured `reasonCode` (default 801) and `diagnostics.code` of `network_error`, `read_error`, or `parse_error`.

Metrics: Each external plugin appears under standard per-plugin latency stats and timing telemetry (`pluginTimings`). Reason codes should be chosen from an allocated range (801+ reserved for external integrations in this project). Customize per definition via `reasonCode`.

### Example Environment

```bash
export SENTRA_PLUGIN_CONFIG=examples/external_http_example.json
export SENTRA_PLUGINS=secrets,external_presidio,pii,domain_block
```

With the above ordering `external_presidio` runs after `secrets` and before built-in PII & domain checks.

### Security Notes
* Keep external endpoints fast (< ~200ms) to preserve end-to-end SLA.
* Prefer TLS endpoints and short timeouts; adjust `timeoutMs` per integration.
* Avoid leaking full conversation history—default template only sends essential fields; extend template only if required.

### Troubleshooting
| Symptom | Likely Cause | Resolution |
|---------|--------------|-----------|
| Plugin name skipped | Missing from `externalHttp` array | Add definition and restart |
| Always allowing | `blockField` mismatch or failOpen masking errors | Enable `failOpen=false` temporarily & check logs |
| Blocks on errors | `failOpen=false` | Set `failOpen=true` to allow instead |

See `examples/external_http_example.json` for a multi-plugin sample.

### Testing Overview

Integration tests cover:
* Blocking vs benign responses
* Structured diagnostics & `blockedBy`
* Audit mode suppression logic
* Log rotation (compressed / uncompressed)
* Concurrency line count integrity

## Benchmarks

Microbenchmarks use Criterion (`cargo bench`) to measure the incremental overhead of an external HTTP plugin under different behaviors.

Current scenarios (`benches/external_http.rs`):

| Scenario | Description | Approx Median (local sample) |
|----------|-------------|------------------------------|
| `external_allow` | Fast allow (immediate `{ "block": false }`) | ~98 µs |
| `external_block` | Fast block (immediate `{ "block": true }`) | ~102 µs |
| `external_slow` | External sleeps 30 ms then allows | ~32.7 ms |
| `external_timeout_fail_open` | 80 ms external; client timeout 20 ms; fail-open => allow | ~22.4 ms (bounded by timeout + overhead) |
| `external_timeout_fail_closed` | Same timeout but failOpen=false triggers block decision path | ~22.4 ms (similar – decision difference only) |
| `external_chain_three_allow` | Three sequential fast allow externals chained | ~190 µs |

Interpretation:
* Allow vs block fast paths differ only a few microseconds (serialization + boolean branch).
* Slow path dominated by external latency — emphasizes importance of tight timeouts and ordering critical plugins first.
* Bench excludes network TLS overhead beyond loopback to focus on application & serialization cost.

Run locally:

```bash
cargo bench --bench external_http
```

Tip: Use `--profile=bench` optimized build; Criterion auto handles warmup & sampling. To compare changes, run twice and examine `target/criterion/report/index.html`.

Observations:
* Fast path per external call adds roughly ~100 µs on loopback including minimal serialization & HTTP overhead.
* Timeout scenarios show deterministic upper bound when `timeoutMs` << external latency; fail-open vs fail-closed only affects decision branching, not elapsed time.
* Chaining three externals is roughly linear (~3x single allow) indicating minimal shared overhead beyond per-request cost.

Previously planned additions have been implemented (timeout + multi-chain). Add new scenarios as needed (e.g., mixed allow/block chains or larger chains) without overwriting historical labels.

When contributing performance-sensitive changes, add a new labeled scenario rather than modifying existing ones to keep historical comparability.

## Contributing

Add plugins by implementing the trait in `src/plugins/` and referencing them in the ordered env list. Provide tests for block + benign cases and update docs where behavior changes.

## Non‑Goals / Omitted Features

Intentionally not implemented:
* Web UI / streaming progress channels
* Background job queue / aggregation fan‑in
* Per‑plugin sandboxing or resource isolation
* JWT / OAuth; only simple allowlist auth (can sit behind upstream auth + TLS)
* Complex scan orchestration – single pass only

## License

This project is licensed under the [MIT License](LICENSE).

## Support

For usage examples see tests & telemetry samples above.
