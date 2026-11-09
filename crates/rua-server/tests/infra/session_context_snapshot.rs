use rua_channel::core::console::SessionPersistence;
use rua_contract::agent::AgentInputMessage;
use rua_core::message::ContentPart;
use rua_server::services::console_chat::{
    build_prompt_file_snapshots, clear_pending_action_snapshot, SessionContextSnapshot,
    WorkspaceSessionPersistence,
};

#[tokio::test]
async fn session_context_snapshot_persists_runtime_fields_without_prompt_file_content() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path();
    std::fs::create_dir_all(workspace.join("prompts")).unwrap();
    std::fs::write(
        workspace.join("prompts").join("AGENT.md"),
        "---\ntitle: Agent\n---\nsecret prompt body",
    )
    .unwrap();

    let store = rua_session::WorkspaceSessionStore::new(workspace);

    let snapshot = SessionContextSnapshot {
        sys_prompt: Some("effective system prompt".to_string()),
        active_prompt_files: build_prompt_file_snapshots(
            workspace,
            &["AGENT.md".to_string(), "MISSING.md".to_string()],
        ),
        pending_action: Some(rua_config::SessionPendingActionSnapshot {
            action_type: "ask_user".to_string(),
            request_id: Some("ask-1".to_string()),
            call_id: None,
            name: None,
            prompt: Some("Pick one".to_string()),
            questions: Some(serde_json::json!([{"question": "Pick one"}])),
            arguments: None,
            status: "waiting".to_string(),
            extra: serde_json::Map::new(),
        }),
    };
    store
        .save_chat_header(&rua_session::ChatSessionHeader::new(
            rua_session::ChatMetadata {
                chat_id: "sess-1".into(),
                agent_id: "agent-1".into(),
                title: "Context".into(),
                created_at: "2026-06-20T00:00:00Z".into(),
                last_activity_at: "2026-06-20T00:00:00Z".into(),
                pinned: false,
                archived: false,
                preview: None,
            },
        ))
        .await
        .unwrap();
    let persistence = WorkspaceSessionPersistence::new(workspace.to_path_buf())
        .with_session_context_snapshot(snapshot);

    persistence
        .save_messages(
            "agent-1",
            "sess-1",
            &[AgentInputMessage {
                role: "user".to_string(),
                content: vec![ContentPart::Text {
                    text: "hello".to_string(),
                }],
            }],
        )
        .await
        .unwrap();

    let header = store
        .load_chat_header("sess-1")
        .await
        .unwrap()
        .expect("Evergreen header");
    assert_eq!(
        header.context_snapshot.sys_prompt.as_deref(),
        Some("effective system prompt")
    );
    let pending = header.context_snapshot.pending_action.as_ref().unwrap();
    assert_eq!(pending["action_type"], "ask_user");
    assert_eq!(pending["status"], "waiting");

    let prompt_files = &header.context_snapshot.active_prompt_files;
    assert_eq!(prompt_files.len(), 2);
    assert_eq!(prompt_files[0]["id"], "AGENT.md");
    assert_eq!(prompt_files[0]["name"], "AGENT.md");
    assert_eq!(prompt_files[0]["included"], true);
    assert!(prompt_files[0]["content_hash"].as_str().unwrap().len() >= 64);
    assert_eq!(prompt_files[1]["id"], "MISSING.md");
    assert_eq!(prompt_files[1]["included"], false);
    assert!(prompt_files[1].get("content_hash").is_none());

    let serialized = serde_json::to_string(prompt_files).unwrap();
    assert!(
        !serialized.contains("secret prompt body"),
        "prompt file snapshot must not inline prompt file content"
    );
}

#[tokio::test]
async fn session_context_snapshot_preserves_existing_ask_user_pending_action() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path();
    let store = rua_session::WorkspaceSessionStore::new(workspace);
    let mut header = rua_session::ChatSessionHeader::new(rua_session::ChatMetadata {
        chat_id: "sess-1".to_string(),
        agent_id: "agent-1".to_string(),
        title: "sess-1".to_string(),
        created_at: "2026-06-20T00:00:00Z".to_string(),
        last_activity_at: "2026-06-20T00:00:00Z".to_string(),
        pinned: false,
        archived: false,
        preview: None,
    });
    header.context_snapshot.pending_action = Some(serde_json::json!({
        "action_type": "ask_user",
        "request_id": "ask-1",
        "status": "waiting"
    }));
    store.save_chat_header(&header).await.unwrap();

    let persistence = WorkspaceSessionPersistence::new(workspace.to_path_buf())
        .with_session_context_snapshot(SessionContextSnapshot {
            sys_prompt: Some("effective system prompt".to_string()),
            active_prompt_files: Vec::new(),
            pending_action: None,
        });

    persistence
        .save_messages(
            "agent-1",
            "sess-1",
            &[AgentInputMessage {
                role: "user".to_string(),
                content: vec![ContentPart::Text {
                    text: "hello".to_string(),
                }],
            }],
        )
        .await
        .unwrap();

    let saved = store
        .load_chat_header("sess-1")
        .await
        .unwrap()
        .expect("Evergreen header");
    let pending = saved.context_snapshot.pending_action.as_ref().unwrap();
    assert_eq!(pending["action_type"], "ask_user");
    assert_eq!(pending["request_id"], "ask-1");
    assert_eq!(pending["status"], "waiting");
}

#[tokio::test]
async fn session_context_snapshot_clears_stale_tool_approval_pending_action() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path();
    let store = rua_session::WorkspaceSessionStore::new(workspace);
    let mut header = rua_session::ChatSessionHeader::new(rua_session::ChatMetadata {
        chat_id: "sess-1".to_string(),
        agent_id: "agent-1".to_string(),
        title: "sess-1".to_string(),
        created_at: "2026-06-20T00:00:00Z".to_string(),
        last_activity_at: "2026-06-20T00:00:00Z".to_string(),
        pinned: false,
        archived: false,
        preview: None,
    });
    header.context_snapshot.pending_action = Some(serde_json::json!({
        "action_type": "tool_approval",
        "request_id": "approval-1",
        "status": "waiting"
    }));
    store.save_chat_header(&header).await.unwrap();
    store
        .append_conversation_turn(
            "sess-1",
            rua_session::NewConversationTurn {
                turn_id: "turn-1".to_string(),
                lifecycle: rua_session::TurnLifecycle {
                    user_submitted_at: "2026-06-20T00:00:00Z".to_string(),
                    assistant_started_at: None,
                    assistant_completed_at: None,
                    status: "in_progress".to_string(),
                },
                selected_variant_id: None,
                messages: vec![rua_session::NewConversationMessage {
                    message_id: "msg-user-1".to_string(),
                    role: "user".to_string(),
                    created_at: "2026-06-20T00:00:00Z".to_string(),
                    status: "completed".to_string(),
                    content: serde_json::json!([{ "type": "text", "text": "hello" }]),
                    provider: None,
                    tool: None,
                    metadata: serde_json::Map::new(),
                }],
            },
        )
        .await
        .unwrap();

    let persistence = WorkspaceSessionPersistence::new(workspace.to_path_buf())
        .with_active_turn_id("turn-1".to_string())
        .with_session_context_snapshot(SessionContextSnapshot {
            sys_prompt: Some("effective system prompt".to_string()),
            active_prompt_files: Vec::new(),
            pending_action: None,
        });

    persistence
        .save_messages(
            "agent-1",
            "sess-1",
            &[
                AgentInputMessage {
                    role: "user".to_string(),
                    content: vec![ContentPart::Text {
                        text: "hello".to_string(),
                    }],
                },
                AgentInputMessage {
                    role: "assistant".to_string(),
                    content: vec![ContentPart::Text {
                        text: "done".to_string(),
                    }],
                },
            ],
        )
        .await
        .unwrap();

    let saved = store
        .load_chat_header("sess-1")
        .await
        .unwrap()
        .expect("Evergreen header");
    assert!(saved.context_snapshot.pending_action.is_none());
    let turns = store.read_turn_range("sess-1", 1, 10).await.unwrap();
    assert_eq!(turns[0].lifecycle.status, "completed");
    assert!(turns[0].lifecycle.assistant_completed_at.is_some());
}

#[tokio::test]
async fn resolved_pending_action_snapshot_is_cleared_explicitly() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path();
    let store = rua_session::WorkspaceSessionStore::new(workspace);
    let mut header = rua_session::ChatSessionHeader::new(rua_session::ChatMetadata {
        chat_id: "sess-1".to_string(),
        agent_id: "agent-1".to_string(),
        title: "sess-1".to_string(),
        created_at: "2026-06-20T00:00:00Z".to_string(),
        last_activity_at: "2026-06-20T00:00:00Z".to_string(),
        pinned: false,
        archived: false,
        preview: None,
    });
    header.context_snapshot.pending_action = Some(serde_json::json!({
        "action_type": "ask_user",
        "request_id": "ask-1",
        "status": "waiting"
    }));
    store.save_chat_header(&header).await.unwrap();

    clear_pending_action_snapshot(workspace, "sess-1")
        .await
        .unwrap();

    let saved = store
        .load_chat_header("sess-1")
        .await
        .unwrap()
        .expect("Evergreen header");
    assert!(saved.context_snapshot.pending_action.is_none());
}
