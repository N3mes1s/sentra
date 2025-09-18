# Metrics Reference

See also: `ARCHITECTURE.md` (flow & endpoints), `DIAGNOSTICS.md` (reason codes / diagnostics fields), `SECURITY.md` (controls & failure modes).

Prometheus exposition endpoint: `GET /metrics` (text format).

## Summary
| Metric | Type | Semantics |
|--------|------|-----------|
| `sentra_requests_total` | counter | Total analyze endpoint requests processed (regardless of outcome). |
| `sentra_blocks_total` | counter | Count of requests where a plugin decided to block (before audit-only override). |
| `sentra_audit_suppressed_total` | counter | Blocks that were converted to allow due to audit-only mode. |
| `sentra_telemetry_lines_total` | counter | Telemetry or audit JSON lines successfully written (includes audit lines). |
| `sentra_request_latency_ms_*` | histogram | Millisecond end-to-end handler latency distribution. |
| `sentra_build_info` | gauge | Constant 1; labels expose build metadata (version, schemaVersion). |
| `sentra_plugin_eval_ms_sum{plugin}` | counter | Cumulative evaluation time (ms) per plugin. |
| `sentra_plugin_eval_ms_count{plugin}` | counter | Number of evaluations per plugin. |
| `sentra_plugin_blocks_total{plugin}` | counter | Blocking decisions attributed to a plugin (pre audit suppression). |
| `sentra_telemetry_write_errors_total` | counter | Failed telemetry/audit line writes. |
| `sentra_log_file_size_bytes` | gauge | Current active telemetry log file size (0 if disabled). |
| `sentra_process_start_time_seconds` | gauge | Unix epoch seconds when process started. |
| `sentra_process_uptime_seconds` | gauge | Process uptime seconds. |
| `sentra_plugin_latency_ms_*{plugin}` | histogram | Per-plugin evaluation latency distribution (same buckets as request latency). |

## Counters
### `sentra_requests_total`
Incremented exactly once per successful invocation of the analyze handler after validation.

### `sentra_blocks_total`
Incremented when the *would-be* response has `blockAction=true` (even if hidden by audit-only mode).

### `sentra_audit_suppressed_total`
Incremented when audit-only mode is active AND a plugin would have blocked. Represents production-impacting blocks currently suppressed.

### `sentra_telemetry_lines_total`
Incremented for each JSON line written via the telemetry or audit writer. If audit mode produces an extra audit line, both lines contribute.

## Latency Histogram: `sentra_request_latency_ms`
Captures wall-clock latency (ms) from handler start to final response decision (post plugin evaluation, pre write flush). Buckets are cumulative per Prometheus histogram semantics.

Buckets (upper bounds, inclusive):
```
[1, 2, 5, 10, 20, 50, 100, 200, 500, 1000, 2000, +Inf]
```
Exports:
- `sentra_request_latency_ms_bucket{le="<bound>"}` cumulative count up to each bound
- `sentra_request_latency_ms_bucket{le="+Inf"}` total sample count
- `sentra_request_latency_ms_sum` sum of observed latency values (ms)
- `sentra_request_latency_ms_count` total observations (matches +Inf bucket)

Implementation notes:
- Buckets are fixed at process start; no dynamic resizing.
- Only the first bucket with `value <= upper_bound` is incremented (standard approach).
- Latency integer conversion uses `as_millis()`; sub-millisecond durations are truncated to 0ms.

## Build Info Gauge: `sentra_build_info`
`sentra_build_info{version="<crate-version>",schemaVersion="1"} 1`

Purpose: allow joining runtime samples with code / schema version. Single time series (label cardinality fixed).

## Cardinality & Cost Guidance
- All metrics avoid unbounded label cardinality.
- Safe to scrape at high frequency; dominant cost is string assembly on demand (no background aggregation threads).
- Request latency histogram series = bucket_count + 3 (sum, count, +Inf bucket line counted via buckets).
- Per-plugin latency histogram series = `#plugins * (bucket_count + 3)`.
- With default 6 plugins and 11 finite buckets: `(11 + 3) * 6 = 84` additional series (still low).
- Per-observation cost: single pass until first matching bucket (O(buckets) worst case, but early exit keeps average low). Buckets kept intentionally small (11 finite) to minimize branch work.
- Memory overhead: `(#plugins * bucket_count)` AtomicU64s plus per-plugin sum/count (negligible vs application footprint).

## Example Scrape Output (Excerpt)
```
sentra_requests_total 42
sentra_blocks_total 9
sentra_audit_suppressed_total 2
sentra_telemetry_lines_total 51
sentra_request_latency_ms_bucket{le="1"} 3
...
sentra_request_latency_ms_bucket{le="+Inf"} 42
sentra_request_latency_ms_sum 1234
sentra_request_latency_ms_count 42
sentra_build_info{version="0.3.0",schemaVersion="1"} 1
```

## Operational Usage Tips
- Block rate: `sentra_blocks_total / sentra_requests_total`
- Effective (user-visible) block rate under audit-only: `(sentra_blocks_total - sentra_audit_suppressed_total) / sentra_requests_total`
- Mean latency (ms): `sentra_request_latency_ms_sum / sentra_request_latency_ms_count`
- P95 estimate: use Prometheus `histogram_quantile(0.95, sum(rate(sentra_request_latency_ms_bucket[5m])) by (le))`

## Future Potential Metrics (Not Implemented)
- Separate request latency histograms split by block vs benign
- Telemetry flush duration metric (log write + fsync if enabled)
- Size/age of rotated log backups

