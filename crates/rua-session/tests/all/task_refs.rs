use rua_session::task_refs::ChatTaskRef;
use rua_task::{TaskRecord, TaskState};
use serde_json::json;

fn reference() -> ChatTaskRef {
    serde_json::from_value(json!({
        "task_id":"task-one", "owner":{"chat_address":{"chat_id":"chat-one"}},
        "execution":{"device_id":"phone","public_cwd":"/photos","future_location":true},
        "origin":{"turn_id":"turn-one","tool_call_id":"call-one","agent_id":"original-agent","configuration_epoch":"original-epoch"},
        "last_target_evidence":null,"future_reference":{"preserved":true}
    })).unwrap()
}

fn record(attempt: u32, state: TaskState) -> TaskRecord {
    serde_json::from_value(json!({
        "task_id":"task-one", "spec":{
            "owner":{"chat_address":{"chat_id":"chat-one"}},"instruction":"Inspect photos",
            "runner":{"type":"tool","tool_name":"bash","arguments":{"command":"pwd"}},
            "context_refs":[],"depends_on":[],"origin":{"turn_id":"turn-one","tool_call_id":"call-one"}
        },"state":state,"attempt_no":attempt,"created_at":"2026-11-21T00:00:00Z","updated_at":"2026-11-21T00:00:00Z",
        "attempts":[{"attempt_no":attempt,"state":state,"authorization_agent_id":format!("agent-{attempt}"),
            "configuration_epoch":format!("epoch-{attempt}"),"authorization_origin":{"turn_id":format!("turn-{attempt}")},
            "device":{"device_id":"phone","public_cwd":"/photos"},
            "created_at":"2026-11-21T00:00:00Z","updated_at":"2026-11-21T00:00:00Z"}]
    })).unwrap()
}

#[test]
fn unconfirmed_reference_survives_reload_and_preserves_unknown_fields_without_a_task_database() {
    let root = tempfile::tempdir().unwrap();
    let chat = root.path().join("sessions/chat-one");
    let reference = reference();
    assert!(ChatTaskRef::list(&chat, "chat-one").unwrap().is_empty());
    assert!(reference.register(&chat, "chat-one").unwrap());
    assert!(!reference.register(&chat, "chat-one").unwrap());
    assert_eq!(
        ChatTaskRef::load(&chat, "chat-one", "task-one").unwrap(),
        Some(reference.clone())
    );
    assert_eq!(
        ChatTaskRef::list(&chat, "chat-one").unwrap(),
        vec![reference.clone()]
    );
    let bytes = std::fs::read(chat.join("task_refs/task-one.json")).unwrap();
    let saved: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(saved["future_reference"], json!({"preserved":true}));
    assert_eq!(saved["execution"]["future_location"], true);
    assert!(saved["last_target_evidence"].is_null());
    assert!(!root.path().join("task-runtime.sqlite").exists());
    assert!(reference.register(&chat, "other-chat").is_err());
    assert!(ChatTaskRef::load(&chat, "other-chat", "task-one").is_err());
}

#[test]
fn target_revisions_persist_across_capture_reopen_and_retry_without_replacing_origin() {
    let root = tempfile::tempdir().unwrap();
    reference().register(root.path(), "chat-one").unwrap();
    assert!(ChatTaskRef::capture_target_evidence(
        root.path(),
        "chat-one",
        "task-one",
        "desktop",
        record(1, TaskState::Running)
    )
    .is_err());
    let first = ChatTaskRef::capture_target_evidence(
        root.path(),
        "chat-one",
        "task-one",
        "phone",
        record(1, TaskState::Running),
    )
    .unwrap();
    assert_eq!(first.last_target_evidence.as_ref().unwrap().revision, 1);
    assert_eq!(
        ChatTaskRef::capture_target_evidence(
            root.path(),
            "chat-one",
            "task-one",
            "phone",
            record(1, TaskState::Running)
        )
        .unwrap(),
        first
    );
    let terminal = ChatTaskRef::capture_target_evidence(
        root.path(),
        "chat-one",
        "task-one",
        "phone",
        record(1, TaskState::Failed),
    )
    .unwrap();
    assert_eq!(terminal.last_target_evidence.as_ref().unwrap().revision, 2);
    let mut retry = record(2, TaskState::Running);
    retry.attempts.insert(
        0,
        terminal.last_target_evidence.unwrap().record.attempts[0].clone(),
    );
    let next =
        ChatTaskRef::capture_target_evidence(root.path(), "chat-one", "task-one", "phone", retry)
            .unwrap();
    let evidence = next.last_target_evidence.as_ref().unwrap();
    assert_eq!(evidence.revision, 3);
    assert_eq!(evidence.record.attempts.len(), 1);
    assert_eq!(
        evidence.record.attempts[0].authorization_agent_id,
        "agent-2"
    );
    assert_eq!(
        evidence.record.attempts[0].configuration_epoch.as_deref(),
        Some("epoch-2")
    );
    assert_eq!(next.origin, reference().origin);
    assert_eq!(
        ChatTaskRef::load(root.path(), "chat-one", "task-one").unwrap(),
        Some(next)
    );
}

#[test]
fn stale_attempt_revision_and_unconfirmed_snapshots_cannot_erase_live_sources() {
    let target = tempfile::tempdir().unwrap();
    let replica = tempfile::tempdir().unwrap();
    reference().register(target.path(), "chat-one").unwrap();
    let first = ChatTaskRef::capture_target_evidence(
        target.path(),
        "chat-one",
        "task-one",
        "phone",
        record(1, TaskState::Running),
    )
    .unwrap();
    let second = ChatTaskRef::capture_target_evidence(
        target.path(),
        "chat-one",
        "task-one",
        "phone",
        record(2, TaskState::WaitingApproval),
    )
    .unwrap();
    second.register(replica.path(), "chat-one").unwrap();
    assert!(!first.register(replica.path(), "chat-one").unwrap());
    assert!(!reference().register(replica.path(), "chat-one").unwrap());
    let mut stale_attempt = first.clone();
    stale_attempt
        .last_target_evidence
        .as_mut()
        .unwrap()
        .revision = 99;
    assert!(!stale_attempt.register(replica.path(), "chat-one").unwrap());
    let mut conflict = second.clone();
    conflict.last_target_evidence.as_mut().unwrap().record = record(2, TaskState::Running);
    assert!(conflict.register(replica.path(), "chat-one").is_err());
    assert_eq!(
        ChatTaskRef::load(replica.path(), "chat-one", "task-one").unwrap(),
        Some(second)
    );
}

#[test]
fn fixed_identity_specification_and_terminal_attempt_cannot_be_rewritten() {
    let root = tempfile::tempdir().unwrap();
    reference().register(root.path(), "chat-one").unwrap();
    let original = ChatTaskRef::capture_target_evidence(
        root.path(),
        "chat-one",
        "task-one",
        "phone",
        record(1, TaskState::Completed),
    )
    .unwrap();
    assert!(ChatTaskRef::capture_target_evidence(
        root.path(),
        "chat-one",
        "task-one",
        "phone",
        record(1, TaskState::Running)
    )
    .is_err());
    let mut changed = reference();
    changed.execution.public_cwd = "/elsewhere".into();
    assert!(changed.register(root.path(), "chat-one").is_err());
    changed = reference();
    changed.execution.device_id = "desktop".into();
    assert!(changed.register(root.path(), "chat-one").is_err());
    changed = reference();
    changed.origin.agent_id = "different-origin".into();
    assert!(changed.register(root.path(), "chat-one").is_err());
    let mut changed_authorization = record(1, TaskState::Completed);
    changed_authorization.attempts[0].authorization_agent_id = "new-chat-agent".into();
    assert!(ChatTaskRef::capture_target_evidence(
        root.path(),
        "chat-one",
        "task-one",
        "phone",
        changed_authorization
    )
    .is_err());
    let mut changed_spec = record(2, TaskState::Running);
    changed_spec.spec.instruction = "Another task".into();
    assert!(ChatTaskRef::capture_target_evidence(
        root.path(),
        "chat-one",
        "task-one",
        "phone",
        changed_spec
    )
    .is_err());
    assert_eq!(
        ChatTaskRef::load(root.path(), "chat-one", "task-one").unwrap(),
        Some(original)
    );
}

#[test]
fn concurrent_duplicate_target_capture_assigns_only_one_persisted_revision() {
    let root = tempfile::tempdir().unwrap();
    reference().register(root.path(), "chat-one").unwrap();
    let workers: Vec<_> = (0..8)
        .map(|_| {
            let path = root.path().to_owned();
            std::thread::spawn(move || {
                ChatTaskRef::capture_target_evidence(
                    &path,
                    "chat-one",
                    "task-one",
                    "phone",
                    record(1, TaskState::Running),
                )
                .unwrap()
            })
        })
        .collect();
    for worker in workers {
        assert_eq!(
            worker
                .join()
                .unwrap()
                .last_target_evidence
                .unwrap()
                .revision,
            1
        );
    }
    assert_eq!(
        std::fs::read_dir(root.path().join("task_refs"))
            .unwrap()
            .count(),
        1
    );
}

#[cfg(unix)]
#[test]
fn task_reference_paths_reject_traversal_symlinks_and_oversized_metadata() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    symlink(outside.path(), root.path().join("task_refs")).unwrap();
    assert!(reference().register(root.path(), "chat-one").is_err());
    assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 0);
    std::fs::remove_file(root.path().join("task_refs")).unwrap();
    for id in ["../escape", "a/b", "a\\b", ".", "C:drive", ""] {
        let mut reference = reference();
        reference.task_id = id.into();
        assert!(reference.register(root.path(), "chat-one").is_err());
    }
    let mut oversized = reference();
    oversized.extra.insert(
        "oversized".into(),
        "x".repeat(rua_session::task_refs::MAX_TASK_REF_BYTES)
            .into(),
    );
    assert!(oversized.register(root.path(), "chat-one").is_err());
    std::fs::create_dir(root.path().join("task_refs")).unwrap();
    let external = outside.path().join("untouched.json");
    std::fs::write(&external, b"untouched").unwrap();
    symlink(&external, root.path().join("task_refs/task-one.json")).unwrap();
    assert!(reference().register(root.path(), "chat-one").is_err());
    assert_eq!(std::fs::read(external).unwrap(), b"untouched");
}

#[test]
fn confirmed_non_acceptance_is_monotonic_but_real_target_acceptance_always_wins() {
    let root = tempfile::tempdir().unwrap();
    let pending = reference();
    pending.register(root.path(), "chat-one").unwrap();
    let denied = ChatTaskRef::mark_not_accepted(root.path(), "chat-one", "task-one").unwrap();
    assert!(denied.not_accepted);
    assert!(denied.last_target_evidence.is_none());
    assert!(
        !pending.register(root.path(), "chat-one").unwrap(),
        "old pending sync must not revive an operation known to be unaccepted"
    );
    assert_eq!(denied.merged(&pending).unwrap(), denied);
    assert_eq!(pending.merged(&denied).unwrap(), denied);
    let accepted = ChatTaskRef::capture_target_evidence(
        root.path(),
        "chat-one",
        "task-one",
        "phone",
        record(1, TaskState::Running),
    )
    .unwrap();
    assert!(
        accepted.not_accepted,
        "the monotonic fact need not be normalized away"
    );
    assert_eq!(
        accepted.last_target_evidence.as_ref().unwrap().record.state,
        TaskState::Running
    );
    assert_eq!(
        denied.merged(&accepted).unwrap(),
        accepted,
        "a delayed rejection cannot erase actual acceptance"
    );
    assert_eq!(
        ChatTaskRef::mark_not_accepted(root.path(), "chat-one", "task-one").unwrap(),
        accepted
    );
    let mut other_device = denied;
    other_device.execution.device_id = "another-device".into();
    assert!(other_device.register(root.path(), "chat-one").is_err());
    assert!(ChatTaskRef::mark_not_accepted(root.path(), "other-chat", "task-one").is_err());
}

#[test]
fn waiting_approval_projection_preserves_the_runtimes_running_attempt() {
    let root = tempfile::tempdir().unwrap();
    reference().register(root.path(), "chat-one").unwrap();
    let mut waiting = record(1, TaskState::WaitingApproval);
    // TaskRuntime's approval phase changes Task state; the execution Attempt is still Running.
    waiting.attempts[0].state = TaskState::Running;
    let reference = ChatTaskRef::capture_target_evidence(
        root.path(),
        "chat-one",
        "task-one",
        "phone",
        waiting.clone(),
    )
    .unwrap();
    assert_eq!(reference.last_target_evidence.unwrap().record, waiting);
    let mut invalid = record(1, TaskState::Completed);
    invalid.attempts[0].state = TaskState::Running;
    assert!(ChatTaskRef::capture_target_evidence(
        root.path(),
        "chat-one",
        "task-one",
        "phone",
        invalid
    )
    .is_err());
}
