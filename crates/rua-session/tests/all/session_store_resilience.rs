use rua_session::store::sanitize_filename;
use rua_session::{
    ChatMetadata, ChatPreview, ChatSessionHeader, NewConversationMessage, NewConversationTurn,
    TurnLifecycle, WorkspaceSessionStore,
};

fn conversation_message(message_id: &str, role: &str, text: &str) -> NewConversationMessage {
    NewConversationMessage {
        message_id: message_id.to_string(),
        role: role.to_string(),
        created_at: "2026-07-08T00:00:01Z".to_string(),
        status: "completed".to_string(),
        content: serde_json::json!([{"type": "text", "text": text}]),
        provider: None,
        tool: None,
        metadata: serde_json::Map::new(),
    }
}

fn chat_header(id: &str, title: &str, last_activity_at: &str) -> ChatSessionHeader {
    ChatSessionHeader::new(ChatMetadata {
        chat_id: id.to_string(),
        agent_id: "agent-1".to_string(),
        title: title.to_string(),
        created_at: "2026-06-19T00:00:00Z".to_string(),
        last_activity_at: last_activity_at.to_string(),
        pinned: false,
        archived: false,
        preview: Some(ChatPreview {
            last_message_sequence: Some(7),
            text: format!("preview for {title}"),
        }),
    })
}

fn turn_lifecycle(status: &str) -> TurnLifecycle {
    TurnLifecycle {
        user_submitted_at: "2026-07-08T00:00:00Z".to_string(),
        assistant_started_at: None,
        assistant_completed_at: None,
        status: status.to_string(),
    }
}

#[test]
fn sanitize_filename_replaces_path_and_windows_unsafe_characters() {
    assert_eq!(sanitize_filename("normal-name"), "normal-name");
    assert_eq!(sanitize_filename("discord:dm:12345"), "discord--dm--12345");
    assert_eq!(
        sanitize_filename(r#"a/b\c:d*e?f"g<h>i|j"#),
        "a--b--c--d--e--f--g--h--i--j"
    );
    assert_eq!(sanitize_filename("user_42@host"), "user_42@host");
}

#[test]
fn sanitize_filename_rejects_empty_and_dot_path_segments() {
    assert_eq!(sanitize_filename(""), "--empty--");
    assert_eq!(sanitize_filename("."), "--dot--");
    assert_eq!(sanitize_filename(".."), "--dotdot--");
    assert_eq!(
        sanitize_filename("../chat"),
        "..--chat",
        "normal path separators are still replaced before path joining"
    );
}

#[tokio::test]
async fn workspace_store_appends_assistant_output_after_eager_user_turn() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    store
        .save_chat_header(&chat_header("chat-1", "Evergreen", "2026-07-08T00:00:00Z"))
        .await
        .unwrap();
    store
        .append_conversation_turn(
            "chat-1",
            NewConversationTurn {
                turn_id: "turn-1".to_string(),
                lifecycle: turn_lifecycle("in_progress"),
                selected_variant_id: None,
                messages: vec![conversation_message("user-1", "user", "hello")],
            },
        )
        .await
        .unwrap();

    store
        .append_messages_to_turn(
            "chat-1",
            "turn-1",
            TurnLifecycle {
                assistant_started_at: Some("2026-07-08T00:00:01Z".to_string()),
                assistant_completed_at: Some("2026-07-08T00:00:02Z".to_string()),
                ..turn_lifecycle("completed")
            },
            Some("assistant-1".to_string()),
            vec![conversation_message("assistant-1", "assistant", "hi")],
        )
        .await
        .unwrap();

    let messages = store.read_message_range("chat-1", 1, 10).await.unwrap();
    let roles: Vec<_> = messages
        .iter()
        .map(|message| message.role.as_str())
        .collect();
    assert_eq!(roles, vec!["user", "assistant"]);

    let turns = store.read_turn_range("chat-1", 1, 10).await.unwrap();
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].turn_id, "turn-1");
    assert_eq!(turns[0].assistant_response_sequences, vec![2]);
    assert_eq!(turns[0].lifecycle.status, "completed");
}

#[test]
fn workspace_store_dot_like_ids_stay_inside_sessions_dir() {
    let store = WorkspaceSessionStore::new("/tmp/ws");

    assert_eq!(
        store
            .chat_dir(".")
            .strip_prefix("/tmp/ws/sessions")
            .unwrap(),
        std::path::Path::new("--dot--")
    );
    assert_eq!(
        store
            .chat_dir("..")
            .strip_prefix("/tmp/ws/sessions")
            .unwrap(),
        std::path::Path::new("--dotdot--")
    );
}

#[tokio::test]
async fn workspace_store_deletes_chat_id_storage_without_user_metadata() {
    let root = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(root.path());
    let chat_dir = store.chat_dir("orphan-chat");
    tokio::fs::create_dir_all(chat_dir.join("messages"))
        .await
        .unwrap();

    store
        .delete_chat_storage_by_chat_id("orphan-chat")
        .await
        .unwrap();

    assert!(!chat_dir.exists());
}

#[tokio::test]
async fn workspace_store_creates_directory_backed_chat_header_without_memory_shape() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    let header = chat_header("chat-1", "Evergreen Chat", "2026-06-19T01:00:00Z");

    store.save_chat_header(&header).await.unwrap();

    let path = dir.path().join("sessions/chat-1/session.json");
    assert!(path.exists(), "session header should be directory-backed");
    let raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(raw["schema_version"], 3);
    assert_eq!(raw["chat_id"], "chat-1");
    assert_eq!(raw["metadata"]["title"], "Evergreen Chat");
    assert_eq!(raw["tag"], "none");
    assert!(
        raw.get("agent").is_none(),
        "new Evergreen Chat headers must not emit legacy agent.memory state: {raw}"
    );
    assert!(
        raw.pointer("/metadata/_compressed_summary").is_none(),
        "compressed context summary should live in Summary Checkpoints, not Chat Metadata"
    );

    let loaded = store.load_chat_header("chat-1").await.unwrap().unwrap();
    assert_eq!(loaded.metadata.title, "Evergreen Chat");
    assert_eq!(loaded.message_log.latest_sequence, 0);
    assert_eq!(loaded.turn_index.latest_turn, 0);

    let mut legacy = raw;
    legacy.as_object_mut().unwrap().remove("tag");
    let legacy: ChatSessionHeader = serde_json::from_value(legacy).unwrap();
    assert_eq!(legacy.tag, rua_session::ChatTag::None);
}

#[tokio::test]
async fn workspace_store_lists_chat_headers_from_metadata_without_reading_message_chunks() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    store
        .save_chat_header(&chat_header("old", "Old", "2026-06-19T01:00:00Z"))
        .await
        .unwrap();
    store
        .save_chat_header(&chat_header("new", "New", "2026-06-19T03:00:00Z"))
        .await
        .unwrap();
    let mut pinned = chat_header("pinned", "Pinned", "2026-06-19T02:00:00Z");
    pinned.metadata.pinned = true;
    store.save_chat_header(&pinned).await.unwrap();

    let messages = dir.path().join("sessions/new/messages");
    std::fs::create_dir_all(&messages).unwrap();
    std::fs::write(messages.join("000000.jsonl"), "{this is not parsed by list").unwrap();

    let page = store.list_chat_headers(0, 2).await.unwrap();
    let ids: Vec<_> = page.iter().map(|h| h.chat_id.as_str()).collect();

    assert_eq!(ids, vec!["pinned", "new"]);
    assert_eq!(
        page[1].metadata.preview.as_ref().unwrap().text,
        "preview for New"
    );

    let second_page = store.list_chat_headers(2, 10).await.unwrap();
    let ids: Vec<_> = second_page.iter().map(|h| h.chat_id.as_str()).collect();
    assert_eq!(ids, vec!["old"]);
}

#[cfg(unix)]
#[tokio::test]
async fn workspace_store_chat_header_update_preserves_previous_header_on_atomic_write_failure() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    store
        .save_chat_header(&chat_header("stable", "Stable", "2026-06-19T01:00:00Z"))
        .await
        .unwrap();

    let chat_dir = dir.path().join("sessions/stable");
    let original_permissions = std::fs::metadata(&chat_dir).unwrap().permissions();
    std::fs::set_permissions(&chat_dir, std::fs::Permissions::from_mode(0o555)).unwrap();

    let result = store
        .save_chat_header(&chat_header("stable", "Broken", "2026-06-19T02:00:00Z"))
        .await;

    std::fs::set_permissions(&chat_dir, original_permissions).unwrap();

    assert!(result.is_err(), "read-only chat dir should reject update");

    let loaded = store.load_chat_header("stable").await.unwrap().unwrap();
    assert_eq!(loaded.metadata.title, "Stable");
    assert_eq!(loaded.metadata.last_activity_at, "2026-06-19T01:00:00Z");
}
