use rua_session::store::TurnLifecycleStatus;

#[test]
fn turn_lifecycle_status_marks_only_stable_terminal_states_syncable() {
    assert!(!TurnLifecycleStatus::InProgress.is_syncable_conversation_state());
    assert!(TurnLifecycleStatus::Completed.is_syncable_conversation_state());
    assert!(TurnLifecycleStatus::Interrupted.is_syncable_conversation_state());
    assert!(TurnLifecycleStatus::Interjected.is_syncable_conversation_state());
    assert!(TurnLifecycleStatus::Failed.is_syncable_conversation_state());
}

#[test]
fn unknown_turn_lifecycle_status_is_not_syncable() {
    let parsed = TurnLifecycleStatus::parse("blocked_on_approval");

    assert_eq!(parsed, TurnLifecycleStatus::Unknown);
    assert!(!parsed.is_syncable_conversation_state());
}
