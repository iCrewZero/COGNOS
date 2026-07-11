use cognos_hal::{AuditFilter, AuditLog};
use serde_json::Value;
use tempfile::tempdir;

fn build_log_with_entries(count: usize) -> (AuditLog, tempfile::TempDir, std::path::PathBuf) {
    let dir = tempdir().unwrap();
    let log_path = dir.path().join("audit.log");
    let log = AuditLog::open_at(log_path.clone()).unwrap();
    for idx in 0..count {
        log.append(
            "hal",
            &format!("gate_request_{idx}"),
            "approved",
            Some(&format!("/tmp/target-{idx}")),
            Some(0.25 + (idx as f32 * 0.01)),
            Some("notify"),
            None,
            Some(true),
            Some("security regression fixture"),
            Some("test-session"),
        );
    }
    log.flush();
    (log, dir, log_path)
}

fn read_lines(path: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

fn write_lines(path: &std::path::Path, lines: &[String]) {
    let mut body = lines.join("\n");
    body.push('\n');
    std::fs::write(path, body).unwrap();
}

#[test]
fn fresh_audit_log_verifies() {
    let (log, _dir, _path) = build_log_with_entries(5);
    let result = log.verify();
    assert!(result.valid, "fresh chain should verify: {:?}", result.error);
}

#[test]
fn direct_tamper_in_middle_is_detected_with_index() {
    let (log, _dir, path) = build_log_with_entries(5);
    let mut lines = read_lines(&path);

    let mut entry: Value = serde_json::from_str(&lines[2]).unwrap();
    entry["outcome"] = Value::String("denied".to_string());
    lines[2] = serde_json::to_string(&entry).unwrap();
    write_lines(&path, &lines);

    let result = log.verify();
    assert!(!result.valid, "tampering must be detected");
    assert_eq!(result.broken_at, Some(3));
}

#[test]
fn truncation_is_detected_via_persisted_head() {
    let (log, _dir, path) = build_log_with_entries(5);
    let mut lines = read_lines(&path);
    lines.pop();
    write_lines(&path, &lines);

    let result = log.verify();
    assert!(!result.valid, "truncation must be detected");
    assert_eq!(result.broken_at, Some(5));
    assert!(
        result.error.as_deref().unwrap_or("").contains("Chain head mismatch"),
        "expected head mismatch error, got {:?}",
        result.error
    );
}

#[test]
fn reordering_is_detected_at_first_moved_entry() {
    let (log, _dir, path) = build_log_with_entries(5);
    let mut lines = read_lines(&path);
    lines.swap(1, 2);
    write_lines(&path, &lines);

    let result = log.verify();
    assert!(!result.valid, "reordering must be detected");
    assert_eq!(result.broken_at, Some(2));
}

#[test]
fn deleting_middle_entry_is_detected() {
    let (log, _dir, path) = build_log_with_entries(5);
    let mut lines = read_lines(&path);
    lines.remove(2);
    write_lines(&path, &lines);

    let result = log.verify();
    assert!(!result.valid, "deletion must be detected");
    assert_eq!(result.broken_at, Some(3));
}

#[test]
fn logical_read_still_sees_original_entry_count_before_tamper() {
    let (log, _dir, _path) = build_log_with_entries(5);
    let entries = log.query(&AuditFilter::default());
    assert_eq!(entries.len(), 5);
}
