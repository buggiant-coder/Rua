use std::sync::Arc;

use agent_types::Role;
use rua_command::ConversationCommand;
use rua_session::{
    ChatMetadata, ChatSessionHeader, NewConversationMessage, NewConversationTurn, TurnLifecycle,
    WorkspaceSessionStore,
};
use serde_json::json;

use crate::mock_seams::*;

#[tokio::test]
async fn compact_no_messages() {
    let session = Arc::new(MockSession::new());
    let compactor = Arc::new(MockCompactor::returning("summary"));
    let mut ctx = make_ctx_with_session(session);
    ctx.context_compactor = Some(compactor.clone());

    let cmd = rua_agent::command::CompactCommand;
    let outcome = cmd.execute(&ctx, "").await;
    let text = outcome_text(&outcome);
    assert!(text.contains("No messages to compact"));
    assert!(!compactor.called.load(std::sync::atomic::Ordering::SeqCst));
}

#[tokio::test]
async fn compact_calls_compactor() {
    let session =
        Arc::new(MockSession::new().with_messages(vec![make_message(Role::User, "hello")]));
    let compactor = Arc::new(MockCompactor::returning("compressed summary"));
    let mut ctx = make_ctx_with_session(session.clone());
    ctx.context_compactor = Some(compactor.clone());

    let cmd = rua_agent::command::CompactCommand;
    let outcome = cmd.execute(&ctx, "").await;
    let text = outcome_text(&outcome);

    assert!(compactor.called.load(std::sync::atomic::Ordering::SeqCst));
    assert!(text.contains("Compact Complete"));
    assert!(text.contains("compressed summary"));
    assert!(session
        .publish_context_checkpoint_called
        .load(std::sync::atomic::Ordering::SeqCst));
}

#[tokio::test]
async fn compact_uses_projection_summary_when_folding_more_context() {
    let session = Arc::new(
        MockSession::new()
            .with_messages(vec![make_message(Role::User, "hello")])
            .with_summary("older folded summary"),
    );
    let compactor = Arc::new(MockCompactor::returning("updated full summary"));
    let mut ctx = make_ctx_with_session(session.clone());
    ctx.context_compactor = Some(compactor.clone());

    let cmd = rua_agent::command::CompactCommand;
    let outcome = cmd.execute(&ctx, "").await;
    let text = outcome_text(&outcome);

    assert!(text.contains("updated full summary"));
    assert_eq!(compactor.previous_summary.lock().await.as_deref(), None);
    assert_eq!(
        compactor.compacted_texts.lock().await.as_slice(),
        [
            "# Compact Summary of Previous Conversation\n\nolder folded summary",
            "hello"
        ]
    );
    let summary = session.active_summary().await.unwrap();
    assert_eq!(summary, "updated full summary");
}

#[tokio::test]
async fn compact_publishes_summary_checkpoint_for_evergreen_chat() {
    let workspace = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(workspace.path());
    let chat_id = "session-1";
    store
        .save_chat_header(&ChatSessionHeader::new(ChatMetadata {
            chat_id: chat_id.to_string(),
            agent_id: "agent-1".to_string(),
            title: "Evergreen".to_string(),
            created_at: "2026-06-19T00:00:00Z".to_string(),
            last_activity_at: "2026-06-19T00:00:00Z".to_string(),
            pinned: false,
            archived: false,
            preview: None,
        }))
        .await
        .unwrap();
    store
        .append_conversation_turn(
            chat_id,
            NewConversationTurn {
                turn_id: "turn-1".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-06-19T00:00:00Z".to_string(),
                    assistant_started_at: Some("2026-06-19T00:00:01Z".to_string()),
                    assistant_completed_at: Some("2026-06-19T00:00:02Z".to_string()),
                    status: "completed".to_string(),
                },
                selected_variant_id: Some("variant-1".to_string()),
                messages: vec![
                    NewConversationMessage {
                        message_id: "user-1".to_string(),
                        role: "user".to_string(),
                        created_at: "2026-06-19T00:00:00Z".to_string(),
                        status: "completed".to_string(),
                        content: json!([{"type": "text", "text": "hello"}]),
                        provider: None,
                        tool: None,
                        metadata: serde_json::Map::new(),
                    },
                    NewConversationMessage {
                        message_id: "assistant-1".to_string(),
                        role: "assistant".to_string(),
                        created_at: "2026-06-19T00:00:01Z".to_string(),
                        status: "completed".to_string(),
                        content: json!([{"type": "text", "text": "hi"}]),
                        provider: None,
                        tool: None,
                        metadata: serde_json::Map::new(),
                    },
                ],
            },
        )
        .await
        .unwrap();
    store
        .publish_manual_context_checkpoint(
            chat_id,
            "stale checkpoint summary".to_string(),
            rua_session::CheckpointModelIdentity {
                provider: "manual".to_string(),
                model: "compact".to_string(),
            },
        )
        .await
        .unwrap();
    store
        .append_conversation_turn(
            chat_id,
            NewConversationTurn {
                turn_id: "turn-2".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-06-19T00:00:40Z".to_string(),
                    assistant_started_at: Some("2026-06-19T00:00:41Z".to_string()),
                    assistant_completed_at: Some("2026-06-19T00:00:42Z".to_string()),
                    status: "completed".to_string(),
                },
                selected_variant_id: Some("variant-2".to_string()),
                messages: vec![
                    NewConversationMessage {
                        message_id: "user-2".to_string(),
                        role: "user".to_string(),
                        created_at: "2026-06-19T00:00:40Z".to_string(),
                        status: "completed".to_string(),
                        content: json!([{"type": "text", "text": "followup"}]),
                        provider: None,
                        tool: None,
                        metadata: serde_json::Map::new(),
                    },
                    NewConversationMessage {
                        message_id: "assistant-2".to_string(),
                        role: "assistant".to_string(),
                        created_at: "2026-06-19T00:00:41Z".to_string(),
                        status: "completed".to_string(),
                        content: json!([{"type": "text", "text": "answer"}]),
                        provider: None,
                        tool: None,
                        metadata: serde_json::Map::new(),
                    },
                ],
            },
        )
        .await
        .unwrap();
    store
        .append_conversation_turn(
            chat_id,
            NewConversationTurn {
                turn_id: "turn-old-compact".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-06-19T00:00:30Z".to_string(),
                    assistant_started_at: Some("2026-06-19T00:00:31Z".to_string()),
                    assistant_completed_at: Some("2026-06-19T00:00:32Z".to_string()),
                    status: "completed".to_string(),
                },
                selected_variant_id: Some("variant-old-compact".to_string()),
                messages: vec![
                    NewConversationMessage {
                        message_id: "user-old-compact".to_string(),
                        role: "user".to_string(),
                        created_at: "2026-06-19T00:00:30Z".to_string(),
                        status: "completed".to_string(),
                        content: json!([{"type": "text", "text": "/compact"}]),
                        provider: None,
                        tool: None,
                        metadata: serde_json::Map::new(),
                    },
                    NewConversationMessage {
                        message_id: "assistant-old-compact".to_string(),
                        role: "assistant".to_string(),
                        created_at: "2026-06-19T00:00:31Z".to_string(),
                        status: "completed".to_string(),
                        content: json!([{"type": "text", "text": "stale compact output"}]),
                        provider: None,
                        tool: None,
                        metadata: serde_json::Map::new(),
                    },
                ],
            },
        )
        .await
        .unwrap();
    store
        .append_conversation_turn(
            chat_id,
            NewConversationTurn {
                turn_id: "turn-compact".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-06-19T00:01:00Z".to_string(),
                    assistant_started_at: None,
                    assistant_completed_at: None,
                    status: "in_progress".to_string(),
                },
                selected_variant_id: None,
                messages: vec![NewConversationMessage {
                    message_id: "user-compact".to_string(),
                    role: "user".to_string(),
                    created_at: "2026-06-19T00:01:00Z".to_string(),
                    status: "completed".to_string(),
                    content: json!([{"type": "text", "text": "/compact"}]),
                    provider: None,
                    tool: None,
                    metadata: serde_json::Map::new(),
                }],
            },
        )
        .await
        .unwrap();

    let conversation_session = Arc::new(
        rua_agent::command_adapters::WorkspaceConversationSessionSeam::new(
            workspace.path().to_path_buf(),
        )
        .with_checkpoint_context(
            "agent-1".to_string(),
            rua_core::ChatAddress::agent("agent-1", chat_id),
            None,
            Some(rua_session::CheckpointModelIdentity {
                provider: "test-provider".to_string(),
                model: "test-model".to_string(),
            }),
            None,
        ),
    );
    let compactor = Arc::new(MockCompactor::returning("checkpoint summary"));
    let mut ctx = make_ctx_with_session(conversation_session);
    ctx.workspace_dir = workspace.path().to_string_lossy().into_owned();
    ctx.context_compactor = Some(compactor.clone());

    let cmd = rua_agent::command::CompactCommand;
    let outcome = cmd.execute(&ctx, "").await;

    assert!(outcome_text(&outcome).contains("checkpoint summary"));
    assert_eq!(
        compactor.compacted_texts.lock().await.as_slice(),
        [
            "# Compact Summary of Previous Conversation\n\nstale checkpoint summary",
            "followup",
            "answer"
        ],
        "Evergreen /compact must summarize the active projection, not only the /compact command"
    );
    assert_eq!(compactor.previous_summary.lock().await.as_deref(), None);
    let checkpoint = store
        .load_active_summary_checkpoint(chat_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(checkpoint.summary, "checkpoint summary");
    assert!(checkpoint.previous_checkpoint_id.is_some());
    assert_eq!(checkpoint.source_sequence_range.start_sequence, 3);
    assert_eq!(
        checkpoint.covered_sequence_range.end_sequence, 7,
        "the complete /compact command boundary is covered while the command stays out of summary text"
    );
    let projection = store
        .build_context_projection(
            chat_id,
            rua_session::ContextProjectionRequest {
                current_user_input: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        projection
            .tail_messages
            .iter()
            .map(|message| message.message_id.as_str())
            .collect::<Vec<_>>(),
        Vec::<&str>::new(),
        "the covered /compact command must not re-enter later model context"
    );
}

#[tokio::test]
async fn compact_str_reads_active_summary_checkpoint() {
    let workspace = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(workspace.path());
    let chat_id = "session-1";
    store
        .save_chat_header(&ChatSessionHeader::new(ChatMetadata {
            chat_id: chat_id.to_string(),
            agent_id: "agent-1".to_string(),
            title: "Evergreen".to_string(),
            created_at: "2026-06-19T00:00:00Z".to_string(),
            last_activity_at: "2026-06-19T00:00:00Z".to_string(),
            pinned: false,
            archived: false,
            preview: None,
        }))
        .await
        .unwrap();
    store
        .append_conversation_turn(
            chat_id,
            NewConversationTurn {
                turn_id: "turn-1".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-06-19T00:00:00Z".to_string(),
                    assistant_started_at: Some("2026-06-19T00:00:01Z".to_string()),
                    assistant_completed_at: Some("2026-06-19T00:00:02Z".to_string()),
                    status: "completed".to_string(),
                },
                selected_variant_id: Some("variant-1".to_string()),
                messages: vec![NewConversationMessage {
                    message_id: "user-1".to_string(),
                    role: "user".to_string(),
                    created_at: "2026-06-19T00:00:00Z".to_string(),
                    status: "completed".to_string(),
                    content: json!([{"type": "text", "text": "hello"}]),
                    provider: None,
                    tool: None,
                    metadata: serde_json::Map::new(),
                }],
            },
        )
        .await
        .unwrap();
    store
        .publish_manual_context_checkpoint(
            chat_id,
            "checkpoint-backed summary".to_string(),
            rua_session::CheckpointModelIdentity {
                provider: "manual".to_string(),
                model: "compact".to_string(),
            },
        )
        .await
        .unwrap();

    let conversation_session = Arc::new(
        rua_agent::command_adapters::WorkspaceConversationSessionSeam::new(
            workspace.path().to_path_buf(),
        ),
    );
    let ctx = make_ctx_with_session(conversation_session);

    let cmd = rua_agent::command::CompactStrCommand;
    let outcome = cmd.execute(&ctx, "").await;
    let text = outcome_text(&outcome);

    assert!(text.contains("checkpoint-backed summary"));
}

#[tokio::test]
async fn compact_no_compactor_shows_disabled() {
    let session =
        Arc::new(MockSession::new().with_messages(vec![make_message(Role::User, "hello")]));
    let ctx = make_ctx_with_session(session);

    let cmd = rua_agent::command::CompactCommand;
    let outcome = cmd.execute(&ctx, "").await;
    assert!(outcome_text(&outcome).contains("Context Manager Disabled"));
}

#[tokio::test]
async fn new_with_messages() {
    let session =
        Arc::new(MockSession::new().with_messages(vec![make_message(Role::User, "hello")]));
    let compactor = Arc::new(MockCompactor::returning("s"));
    let mut ctx = make_ctx_with_session(session.clone());
    ctx.context_compactor = Some(compactor);

    let cmd = rua_agent::command::NewCommand;
    let outcome = cmd.execute(&ctx, "").await;
    let text = outcome_text(&outcome);
    assert!(text.contains("New Conversation Started"));
    assert!(session
        .clear_messages_called
        .load(std::sync::atomic::Ordering::SeqCst));
}

#[tokio::test]
async fn new_empty_messages() {
    let session = Arc::new(MockSession::new());
    let mut ctx = make_ctx_with_session(session);
    ctx.context_compactor = Some(Arc::new(MockCompactor::returning("s")));

    let cmd = rua_agent::command::NewCommand;
    let outcome = cmd.execute(&ctx, "").await;
    assert!(outcome_text(&outcome).contains("No messages to summarize"));
}

#[tokio::test]
async fn clear_clears_session_and_carries_metadata() {
    let session = Arc::new(
        MockSession::new()
            .with_messages(vec![make_message(Role::User, "hello")])
            .with_summary("old summary"),
    );
    let ctx = make_ctx_with_session(session.clone());

    let cmd = rua_agent::command::ClearCommand;
    let outcome = cmd.execute(&ctx, "").await;
    let text = outcome_text(&outcome);
    let meta = outcome_metadata(&outcome);

    assert!(text.contains("History Cleared"));
    assert_eq!(meta["clear_history"], true);
    assert_eq!(meta["clear_plan"], true);
    assert!(session
        .clear_messages_called
        .load(std::sync::atomic::Ordering::SeqCst));
}

#[tokio::test]
async fn history_empty() {
    let session = Arc::new(MockSession::new());
    let ctx = make_ctx_with_session(session);

    let cmd = rua_agent::command::HistoryCommand;
    let outcome = cmd.execute(&ctx, "").await;
    assert!(outcome_text(&outcome).contains("No messages"));
}

#[tokio::test]
async fn history_shows_messages() {
    let session = Arc::new(MockSession::new().with_messages(vec![
        make_message(Role::User, "hello"),
        make_message(Role::Assistant, "hi there"),
    ]));
    let ctx = make_ctx_with_session(session);

    let cmd = rua_agent::command::HistoryCommand;
    let outcome = cmd.execute(&ctx, "").await;
    let text = outcome_text(&outcome);
    assert!(text.contains("2 messages"));
    assert!(text.contains("hello"));
}

#[tokio::test]
async fn compact_str_no_summary() {
    let session = Arc::new(MockSession::new());
    let ctx = make_ctx_with_session(session);

    let cmd = rua_agent::command::CompactStrCommand;
    let outcome = cmd.execute(&ctx, "").await;
    assert!(outcome_text(&outcome).contains("No Compressed Summary"));
}

#[tokio::test]
async fn compact_str_shows_summary() {
    let session = Arc::new(MockSession::new().with_summary("test summary"));
    let ctx = make_ctx_with_session(session);

    let cmd = rua_agent::command::CompactStrCommand;
    let outcome = cmd.execute(&ctx, "").await;
    assert!(outcome_text(&outcome).contains("test summary"));
}

#[tokio::test]
async fn message_shows_content() {
    let session = Arc::new(MockSession::new().with_messages(vec![
        make_message(Role::User, "first message"),
        make_message(Role::Assistant, "second message"),
    ]));
    let ctx = make_ctx_with_session(session);

    let cmd = rua_agent::command::MessageCommand;
    let outcome = cmd.execute(&ctx, "2").await;
    let text = outcome_text(&outcome);
    assert!(text.contains("Message 2/2"));
    assert!(text.contains("second message"));
}

#[tokio::test]
async fn message_out_of_range() {
    let session =
        Arc::new(MockSession::new().with_messages(vec![make_message(Role::User, "hello")]));
    let ctx = make_ctx_with_session(session);

    let cmd = rua_agent::command::MessageCommand;
    let outcome = cmd.execute(&ctx, "5").await;
    assert!(outcome_text(&outcome).contains("Out of Range"));
}
