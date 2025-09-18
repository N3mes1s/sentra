# Security

This document lists the controls actually implemented today. Removed or aspirational features (JWT auth, complex scan engine, per‑plugin isolation, DB storage) are intentionally excluded. See also: `ARCHITECTURE.md` (flow), `DIAGNOSTICS.md` (reason codes & structured diagnostics), `METRICS.md` (operational metrics).

Principles:
* Keep the evaluation path small (ordered async plugin loop, first block wins)
* Make decisions attributable (`blockedBy`, structured `diagnostics`)
* Provide safe rollout (audit‑only) and durable evidence (JSONL + rotation)
* Fail open on internal plugin errors (availability preferred) while surfacing telemetry for investigation

Rust provides memory & thread safety; we rely on standard language guarantees without custom unsafe code in the hot path.

## Request Guards

* Required `api-version` query param (missing → 400 / `errorCode=4000`)
* Optional strict token allowlist (`STRICT_AUTH_ALLOWED_TOKENS`) → 401 / `errorCode=2001` when disallowed
* Maximum request size (`SENTRA_MAX_REQUEST_BYTES`) → 413 / `errorCode=4001`
* Basic shape / required JSON fields enforced via serde model

## Authentication

No JWT / role system. Either:
* Strict mode: bearer token must be present in allowlist.
* Default: permissive (still expects Bearer prefix to avoid accidental raw credential logging upstream).

## Evaluation Model

Ordered plugin list; first block returns response immediately. Audit‑only mode can suppress the outward block while persisting an audit line with the would‑block decision for phased rollout.

## Plugins (Current)

`secrets`, `pii`, `email_bcc`, `domain_block`, `exfil`, `policy_pack`, `external_http`.
Internal plugins perform pattern / substring / rule checks on request content and may emit structured diagnostics. The `external_http` plugin can delegate a decision to a remote service (e.g., Presidio) with:
* Template variables: `${userMessage}`, `${toolName}`, `${inputJson}`
* Configurable block field (`block`, `allow`, JSON Pointer, or root `/`)
* Structural non‑empty blocking for pointer targets (arrays/objects)
* Timeout and fail-open (default) or fail-closed behavior
* Custom `reasonCode` & `reason`

No sandboxing / per‑plugin resource metering; all run in‑process. Heavy logic should be avoided or guarded by future timeouts.

## Telemetry & Audit

If `LOG_FILE` is set each evaluation appends a single JSON line: stable `schemaVersion`, timing, decision, `blockedBy`, structured `diagnostics`, plugin timings, correlation ID, optional `auditSuppressed` flag.

Audit‑only mode writes an additional audit line (to `AUDIT_LOG_FILE` if set) containing original request + would‑block response.

Rotation: size‑based (`LOG_MAX_BYTES` + backups `LOG_ROTATE_KEEP` + optional gzip `LOG_ROTATE_COMPRESS`). Gauge metric tracks active file size; write errors increment a counter (non‑fatal).

## Metrics

Prometheus text format at `/metrics`:
* Counters: requests, blocks, audit suppressed, telemetry lines, telemetry write errors
* Histograms: request latency ms + per‑plugin latency ms
* Per‑plugin eval time sum/count counters, per‑plugin block counters
* Gauges: build info (`version`, `schemaVersion`), process start time, process uptime, log file size

## Configuration Inputs (Security-Relevant)

See README: plugin order, config file path, size cap, audit mode, auth allowlist, rotation, timing budgets, stdout mirroring.

## Logging

Telemetry JSON lines are append‑only; failure to write is logged (non‑fatal). No structured redaction beyond what plugins choose to output.

## Failure Modes

* Internal plugin error → treated as allow (logged)
* External HTTP network / parse / read error → allow if `failOpen=true` else block with external reason code
* Timeout (external HTTP) → treated as above (network error path)
* Oversized payload → 413 reject
* Missing / bad auth (strict mode) → 401 reject
* Telemetry write error → request still succeeds (counter incremented)

## Supply Chain

Use `cargo audit` in CI to flag vulnerable crates. Dependency set kept intentionally small; no unsafe blocks on hot path.

## Reporting Issues

Open an issue or provide minimal PoC + description out-of-band to project maintainers. Avoid sending real secrets; use synthetic test data.