use std::path::PathBuf;

use rua_desktop_lib::owner::{
    identity_matches, reconcile_decision, OwnerRecord, ProcessIdentity, ReconcileDecision,
    OWNER_RECORD_VERSION,
};

fn identity(pid: u32, start_time: u64, executable: &str) -> ProcessIdentity {
    ProcessIdentity {
        pid,
        start_time,
        executable: PathBuf::from(executable),
    }
}

fn record() -> OwnerRecord {
    OwnerRecord {
        version: OWNER_RECORD_VERSION,
        shell: identity(10, 100, "/Applications/Rua.app/Contents/MacOS/rua-desktop"),
        runtime: identity(20, 200, "/Applications/Rua.app/Contents/Resources/rua"),
        port: 43000,
        shutdown_token: "launch-secret".to_string(),
    }
}

#[test]
fn process_identity_rejects_pid_reuse() {
    let expected = identity(20, 200, "/opt/rua");
    let reused_pid = identity(20, 201, "/opt/rua");

    assert!(!identity_matches(&expected, &reused_pid));
}

#[test]
fn process_identity_rejects_a_different_executable() {
    let expected = identity(20, 200, "/opt/rua");
    let foreign = identity(20, 200, "/opt/not-rua");

    assert!(!identity_matches(&expected, &foreign));
}

#[test]
fn orphan_reconciliation_preserves_a_live_owning_shell() {
    let record = record();

    assert_eq!(
        reconcile_decision(&record, Some(&record.shell), Some(&record.runtime)),
        ReconcileDecision::PreserveActiveOwner
    );
}

#[test]
fn orphan_reconciliation_stops_only_a_matching_runtime_with_a_dead_shell() {
    let record = record();

    assert_eq!(
        reconcile_decision(&record, None, Some(&record.runtime)),
        ReconcileDecision::StopOrphan
    );
}

#[test]
fn orphan_reconciliation_removes_a_stale_record_without_killing_a_reused_pid() {
    let record = record();
    let reused = identity(
        record.runtime.pid,
        record.runtime.start_time + 1,
        "/opt/foreign",
    );

    assert_eq!(
        reconcile_decision(&record, None, Some(&reused)),
        ReconcileDecision::RemoveStaleRecord
    );
}
