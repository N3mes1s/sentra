# Diagnostics & Attribution

`diagnostics`: structured JSON object only when `blockAction=true`; else null. `blockedBy`: name of the first (and only) blocking plugin. See also: `ARCHITECTURE.md` (pipeline), `METRICS.md` (operational counters / histograms), `SECURITY.md` (controls). Reason code legend is included below for quick reference.

## Single Plugin Block Example
```json
{
	"blockAction": true,
	"reasonCode": 201,
	"reason": "Detected AWS key",
	"blockedBy": "secrets",
	"diagnostics": { "plugin": "secrets", "code": "aws_key", "detail": "AKIA...redacted" }
}
```

## Diagnostics Object Common Fields
| Field | Type | Description |
|-------|------|-------------|
| `plugin` | string | Plugin identifier (`secrets`, `exfil`, `pii`, `email_bcc`, `domain_block`, `policy_pack`, `external_http`) |
| `code` | string | Short machine code per plugin (`aws_key`, `pattern`, `email`, `domain`, `policy`, etc.) |
| `detail` | string? | Optional contextual snippet (may be truncated) |
| `arg` | string? | Policy pack: argument name that matched |
| `value` | string? | Policy pack: value segment that matched (if emitted) |
| `ruleReasonCode` | number? | Policy pack: per‑rule reasonCode from configuration |

Plugins may add keys; clients should ignore unknown members. The `external_http` plugin emits minimal codes (`block`, `network_error`, `parse_error`, `read_error`) plus optional HTTP status. Policy pack rules can surface `ruleReasonCode` if configured.

## Error Diagnostics
Error responses may include diagnostics (optional, not guaranteed). External HTTP plugin may block with synthetic diagnostics describing the failure when configured `failOpen=false`.

## Stability
Existing keys keep semantics; new optional keys may appear. Parse defensively. Reason codes are stable per plugin unless user‑configurable (policy pack rules, external HTTP `reasonCode`).

## Policy Pack Example
```json
{
	"plugin": "policy_pack",
	"code": "policy",
	"arg": "subject",
	"value": "confidential Q4",
	"ruleReasonCode": 750
}
```

## External HTTP Examples

Successful block decision (remote returned a boolean or structural pointer matched):
```json
{
	"plugin": "external_http",
	"code": "block",
	"status": 200
}
```

Fail‑closed network error (example):
```json
{
	"plugin": "external_http",
	"code": "network_error"
}
```

## Reason Code Legend

Static (built‑in) defaults; some are configurable as noted.

| Code | Source | Meaning / Trigger | Configurable |
|------|--------|-------------------|--------------|
| 111 | exfil | Potential data exfiltration pattern | No |
| 112 | email_bcc | Suspicious BCC usage / pattern | No |
| 113 | domain_block | Domain present in block list | No |
| 201 | secrets | Generic secret / credential detected | No |
| 202 | pii | PII detected (email, phone, etc.) | No |
| 700 | policy_pack | Policy pack rule (default when rule omits reason_code) | Per rule (ruleReasonCode) |
| 7xx | policy_pack | User‑assigned per rule reason codes | Yes (config file) |
| 801 | external_http | External HTTP block (default) | Yes (plugin config) |
| 8xx | external_http | Any custom external HTTP reasonCode | Yes (plugin config) |
| 860 | external_http (Presidio example) | Structural non‑empty pointer (root entities array) | Yes (configured) |

Notes:
* Audit‑only mode does not change `reasonCode` in telemetry; outward HTTP response may show allow while telemetry captures the block.
* External HTTP failures (timeout, network, parse) use the plugin's configured `reasonCode` only when `failOpen=false` (fail‑closed). When `failOpen=true` they surface as allow (no reason code).
* Additional internal 4xxx/2xxx `errorCode` values exist for request validation/auth errors (not part of plugin legend).

