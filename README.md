# Sentra

Sentra is a lightweight Rust service that acts as the external security provider for [Microsoft Copilot Studio](https://learn.microsoft.com/en-us/microsoft-copilot-studio/). Every tool execution request from Copilot hits Sentra, runs through an ordered plugin pipeline, and the first plugin that shouts “block” wins; otherwise the request is allowed. Vibecoded with love.

## How Sentra Fits Into Copilot Studio

1. In Copilot Studio create an **external security provider** and point it at your Sentra instance (official docs: [provider setup](https://learn.microsoft.com/en-us/microsoft-copilot-studio/external-security-provider) and [webhook contract](https://learn.microsoft.com/en-us/microsoft-copilot-studio/external-security-webhooks-interface-developers)).
2. Copilot Studio calls `POST /validate?api-version=2025-05-01` to check availability, then `POST /analyze-tool-execution?api-version=2025-05-01` for each tool run.
3. Sentra responds with `blockAction`, optional `reasonCode`, `blockedBy`, and `diagnostics`, giving you transparent reasoning for every decision.

## Quick Start

```bash
git clone https://github.com/N3mes1s/sentra
cd sentra
cargo run --release
```

Sentra listens on `0.0.0.0:8080` by default. Point Copilot Studio’s webhook URLs to:

- `https://<your-host>/validate`
- `https://<your-host>/analyze-tool-execution`

### Minimal configuration

```bash
export SENTRA_PLUGINS="secrets,pii,email_bcc,domain_block,policy_pack"
export STRICT_AUTH_ALLOWED_TOKENS="super-secret-token"
export LOG_FILE="/var/log/sentra/telemetry.jsonl"
export SENTRA_MAX_REQUEST_BYTES=1048576
```

Run `cargo run --release` and issue a `POST /validate` with the bearer token to confirm connectivity.

## Plugin Pipeline

| Plugin | What it checks |
|--------|----------------|
| `secrets` | AWS-style access keys via regex (extend as needed). |
| `pii` | Emails, phones, IBANs, plus configurable keywords. |
| `email_bcc` | BCC must stay on your company domain. |
| `domain_block` | Blocks mentions of disallowed domains. |
| `exfil` | Prompt-injection phrases such as “ignore previous instructions”. |
| `policy_pack` | Custom substring/regex rules from `SENTRA_PLUGIN_CONFIG`. |
| `external_*` | Calls your own policy service with a templated JSON body. |

Order matters: set `SENTRA_PLUGINS` accordingly; the first blocking plugin wins.

## Observability

- **JSONL telemetry** (`LOG_FILE`): one line per request with `blockAction`, `reasonCode`, `blockedBy`, `pluginTimings`, and `auditSuppressed` when audit-only hid a block. Rotation is controlled by `LOG_MAX_BYTES`, `LOG_ROTATE_KEEP`, and `LOG_ROTATE_COMPRESS`.
- **Prometheus metrics** (`GET /metrics`): request/block counters, audit suppression counter, request and per-plugin latency histograms, telemetry write metrics, build info, and uptime gauges.
- **Audit-only mode** (`SENTRA_AUDIT_ONLY=1`): evaluate everything but always return allow; telemetry/audit logs capture the would-block response so you can stage policies safely.

## Configuration Cheatsheet

| Variable | Purpose |
|----------|---------|
| `SENTRA_PLUGINS` | Ordered plugin list (comma separated). |
| `SENTRA_PLUGIN_CONFIG` | JSON config for policy pack, domain lists, keywords, external HTTP definitions. |
| `STRICT_AUTH_ALLOWED_TOKENS` | Comma-separated bearer tokens accepted in the `Authorization` header. Leave unset to accept any token. |
| `SENTRA_MAX_REQUEST_BYTES` | Reject payloads that exceed this size (covers both `Content-Length` and chunked uploads). |
| `SENTRA_PLUGIN_BUDGET_MS` | Soft time budget shared by plugins (used for deadline warnings). |
| `SENTRA_PLUGIN_WARN_MS` | Log a warning when a single plugin takes longer than this many milliseconds. |
| `LOG_FILE`, `AUDIT_LOG_FILE` | JSONL telemetry and audit file paths. |
| `SENTRA_LOG_STDOUT`, `SENTRA_LOG_SAMPLE_N` | Mirror telemetry/audit lines to stdout, optionally sampling 1/N lines. |
| `LOG_MAX_BYTES`, `LOG_ROTATE_KEEP`, `LOG_ROTATE_COMPRESS` | Configure telemetry log rotation and gzip. |

Check `examples/policy_config_example.json` for a full sample config and `examples/agent_crescendo.rs` for a scripted attack simulation.

## Working With Copilot Studio

1. Deploy Sentra (Docker or binary) behind HTTPS with the env vars above.
2. Register the Copilot Studio external security provider, supplying the Sentra URLs and bearer token.
3. Use Copilot Studio’s test UI to send sample tool requests; review telemetry/metrics to confirm blocks and timings.
4. Switch from audit-only to enforcement once you’re comfortable with the false-positive rate.

## Development

- `cargo test` – full unit + integration test suite.
- `cargo clippy --all-targets --all-features` – linting.
- `cargo fmt` – format the codebase.

## License

This project is licensed under the [MIT License](LICENSE).

---

Vibecoded with ❤️
