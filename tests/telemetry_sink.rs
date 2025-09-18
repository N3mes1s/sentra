use once_cell::sync::Lazy;
use sentra::{AuditLogFields, RotatingWriter, TelemetryLogFields, TelemetrySink};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use tempfile::tempdir;

static TEST_GUARD: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

fn create_sink(path: &std::path::Path) -> TelemetrySink {
    let writer = RotatingWriter::open(path.to_str().unwrap(), None, 1, false).unwrap();
    let telemetry_writer = Some(Arc::new(Mutex::new(writer)));
    let audit_writer = None;
    let metric_lines = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let metric_errors = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let log_size = Arc::new(std::sync::atomic::AtomicU64::new(0));
    TelemetrySink::new(
        telemetry_writer,
        audit_writer,
        false,
        None,
        metric_lines,
        metric_errors,
        log_size,
    )
}

#[test]
fn emit_event_updates_metrics_and_file() {
    let _lock = TEST_GUARD.lock().unwrap();
    let tmp = tempdir().unwrap();
    let path = tmp.path().join("telemetry.log");
    let sink = create_sink(&path);

    let payload = serde_json::json!({"blockAction": false});
    sink.emit_event(
        &payload,
        &TelemetryLogFields {
            block_action: false,
            reason_code: None,
            blocked_by: None,
            latency_ms: 12u128,
            audit_suppressed: false,
            plugin_count: 0,
        },
    );

    let lines = std::fs::read_to_string(&path).unwrap();
    assert!(!lines.trim().is_empty(), "expected telemetry line in file");
    assert_eq!(sink.lines_total().load(Ordering::Relaxed), 1);
    assert_eq!(sink.write_errors_total().load(Ordering::Relaxed), 0);
}

#[test]
fn emit_audit_falls_back_to_telemetry_writer() {
    let _lock = TEST_GUARD.lock().unwrap();
    let tmp = tempdir().unwrap();
    let path = tmp.path().join("audit.log");
    let sink = create_sink(&path);

    let payload = serde_json::json!({"auditOnly": true});
    sink.emit_audit(
        &payload,
        &AuditLogFields {
            would_block: true,
            reason_code: Some(860),
            blocked_by: Some("external_presidio"),
            plugin_count: 2,
        },
    );

    let contents = std::fs::read_to_string(&path).unwrap();
    assert!(contents.contains("auditOnly"));
    assert_eq!(sink.lines_total().load(Ordering::Relaxed), 1);
}
