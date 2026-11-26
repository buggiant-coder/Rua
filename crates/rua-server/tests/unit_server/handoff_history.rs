#[path = "../../src/services/conversation_input_intake/handoff_history.rs"]
mod history;

use rua_session::{ChatMetadata, ChatSessionHeader, WorkspaceSessionStore};
use serde_json::json;

async fn fixture() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(root.path());
    let mut header = ChatSessionHeader::new(ChatMetadata {
        chat_id: "chat".into(),
        agent_id: "agent".into(),
        title: "History".into(),
        created_at: "2026-11-22T00:00:00Z".into(),
        last_activity_at: "2026-11-22T00:00:00Z".into(),
        pinned: false,
        archived: false,
        preview: None,
    });
    header
        .extra
        .insert("hook_configuration".into(), json!({"epoch":"old"}));
    store
        .save_chat_header_with_configuration(&header)
        .await
        .unwrap();
    rua_config::atomic_write_json(
        &store.chat_dir("chat").join("checkpoints/old.json"),
        &json!({"summary":"original", "covered_sequence_end":1}),
    )
    .unwrap();
    root
}

#[tokio::test]
async fn matching_tail_does_not_hide_missing_or_changed_old_history() {
    let source = fixture().await;
    let target = fixture().await;
    let expected = history::capture(source.path(), "chat").await.unwrap();
    assert!(history::capture(target.path(), "chat")
        .await
        .unwrap()
        .contains(&expected));
    let checkpoint = target.path().join("sessions/chat/checkpoints/old.json");
    rua_config::atomic_write_json(
        &checkpoint,
        &json!({"summary":"changed", "covered_sequence_end":1}),
    )
    .unwrap();
    assert!(!history::capture(target.path(), "chat")
        .await
        .unwrap()
        .contains(&expected));
    rua_config::remove_file_committed(&checkpoint).unwrap();
    assert!(!history::capture(target.path(), "chat")
        .await
        .unwrap()
        .contains(&expected));
}

#[tokio::test]
async fn stale_target_projection_is_not_hidden_by_an_equal_turn_tail() {
    let source = fixture().await;
    let target = fixture().await;
    let expected = history::capture(source.path(), "chat").await.unwrap();
    rua_config::atomic_write_json(
        &target
            .path()
            .join("sessions/chat/projection_exclusions.json"),
        &json!({"excluded_turn_ids":["earlier-turn"]}),
    )
    .unwrap();
    assert!(!history::capture(target.path(), "chat")
        .await
        .unwrap()
        .contains(&expected));
}

#[tokio::test]
async fn copies_and_candidate_do_not_block_handoff_but_media_identity_does() {
    let source = fixture().await;
    let target = fixture().await;
    for (root, devices) in [(&source, json!(["a"])), (&target, json!(["a", "c"]))] {
        rua_config::atomic_write_json(&root.path().join("sessions/chat/media_refs/photo.json"),
            &json!({"media_ref":"media/photo", "sha256":"original", "origin":{"device_id":"a", "path":"/photos/a.jpg"}, "device_ids":devices})).unwrap();
    }
    let expected = history::capture(source.path(), "chat").await.unwrap();
    rua_config::atomic_write_json(
        &target
            .path()
            .join("sessions/chat/configurations/candidate.json"),
        &json!({"epoch":"candidate"}),
    )
    .unwrap();
    rua_config::atomic_write(
        &target.path().join("ordinary-file.jpg"),
        b"not part of Chat state",
    )
    .unwrap();
    assert!(history::capture(target.path(), "chat")
        .await
        .unwrap()
        .contains(&expected));
    rua_config::atomic_write_json(&target.path().join("sessions/chat/media_refs/photo.json"),
        &json!({"media_ref":"media/photo", "sha256":"different", "origin":{"device_id":"a", "path":"/photos/a.jpg"}, "device_ids":["a", "c"]})).unwrap();
    assert!(!history::capture(target.path(), "chat")
        .await
        .unwrap()
        .contains(&expected));
}

#[tokio::test]
async fn source_resume_changes_snapshot_but_target_wake_state_is_installed_separately() {
    let source = fixture().await;
    let target = fixture().await;
    let store = WorkspaceSessionStore::new(source.path());
    store
        .set_collaboration_wake_suppressed("chat", true)
        .await
        .unwrap();
    let expected = history::capture(source.path(), "chat").await.unwrap();
    assert!(expected.wake_state.suppressed);
    assert!(history::capture(target.path(), "chat")
        .await
        .unwrap()
        .contains(&expected));
    store
        .set_collaboration_wake_suppressed("chat", false)
        .await
        .unwrap();
    assert_ne!(
        history::capture(source.path(), "chat").await.unwrap(),
        expected
    );
}

#[tokio::test]
async fn unreadable_target_projection_cannot_masquerade_as_absent_source_projection() {
    let source = fixture().await;
    let target = fixture().await;
    history::capture(source.path(), "chat").await.unwrap();
    rua_config::atomic_write(
        &target
            .path()
            .join("sessions/chat/projection_exclusions.json"),
        b"invalid JSON",
    )
    .unwrap();
    let error = history::capture(target.path(), "chat").await.unwrap_err();
    assert!(error.contains("projection_exclusions.json"), "{error}");
}

#[tokio::test]
async fn unreadable_source_history_cannot_be_silently_omitted_from_requirements() {
    let source = fixture().await;
    rua_config::atomic_write(
        &source.path().join("sessions/chat/checkpoints/old.json"),
        b"invalid JSON",
    )
    .unwrap();
    let error = history::capture(source.path(), "chat").await.unwrap_err();
    assert!(error.contains("checkpoints/old.json"), "{error}");
}
