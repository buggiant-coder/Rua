use rua_sync::revocation::{RevocationDetector, RevocationState};

#[test]
fn starts_active() {
    let detector = RevocationDetector::new();
    assert_eq!(*detector.state(), RevocationState::Active);
    assert!(!detector.is_revoked());
}

#[test]
fn single_cycle_all_failures_becomes_suspected() {
    let mut detector = RevocationDetector::new();
    detector.record_decrypt(false);
    detector.record_decrypt(false);
    detector.record_decrypt(false);
    detector.end_cycle();

    assert!(matches!(
        *detector.state(),
        RevocationState::Suspected { .. }
    ));
    assert!(!detector.is_revoked());
}

#[test]
fn two_full_failure_cycles_becomes_revoked() {
    let mut detector = RevocationDetector::new();

    detector.record_decrypt(false);
    detector.record_decrypt(false);
    detector.end_cycle();

    detector.record_decrypt(false);
    detector.record_decrypt(false);
    detector.end_cycle();

    assert_eq!(*detector.state(), RevocationState::Revoked);
    assert!(detector.is_revoked());
}

#[test]
fn one_success_resets_to_active() {
    let mut detector = RevocationDetector::new();

    detector.record_decrypt(false);
    detector.record_decrypt(false);
    detector.end_cycle();
    assert!(matches!(
        *detector.state(),
        RevocationState::Suspected { .. }
    ));

    detector.record_decrypt(false);
    detector.record_decrypt(true);
    detector.end_cycle();
    assert_eq!(*detector.state(), RevocationState::Active);
}

#[test]
fn empty_cycle_does_not_change_state() {
    let mut detector = RevocationDetector::new();
    detector.end_cycle();
    assert_eq!(*detector.state(), RevocationState::Active);
}

#[test]
fn mark_revoked_forces_state() {
    let mut detector = RevocationDetector::new();
    detector.mark_revoked();
    assert_eq!(*detector.state(), RevocationState::Revoked);
    assert!(detector.is_revoked());
}

#[test]
fn reset_clears_everything() {
    let mut detector = RevocationDetector::new();
    detector.mark_revoked();
    detector.reset();
    assert_eq!(*detector.state(), RevocationState::Active);
    assert!(!detector.is_revoked());
}
