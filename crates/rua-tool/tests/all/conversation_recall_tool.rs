use rua_contract::Tool;
use rua_core::tool::ToolContext;
use rua_session::{
    ChatMetadata, ChatSessionHeader, NewConversationMessage, NewConversationTurn, TurnLifecycle,
    WorkspaceSessionStore,
};
use serde_json::json;
use std::sync::Arc;

fn header(chat_id: &str) -> ChatSessionHeader {
    ChatSessionHeader::new(ChatMetadata {
        chat_id: chat_id.to_string(),
        agent_id: "agent-1".to_string(),
        title: chat_id.to_string(),
        created_at: "2026-06-19T00:00:00Z".to_string(),
        last_activity_at: "2026-06-19T00:00:00Z".to_string(),
        pinned: false,
        archived: false,
        preview: None,
    })
}

fn message(message_id: &str, role: &str, text: &str) -> NewConversationMessage {
    NewConversationMessage {
        message_id: message_id.to_string(),
        role: role.to_string(),
        created_at: "2026-06-19T00:00:00Z".to_string(),
        status: "completed".to_string(),
        content: json!([{"type": "text", "text": text}]),
        provider: None,
        tool: None,
        metadata: serde_json::Map::new(),
    }
}

async fn append_turn(store: &WorkspaceSessionStore, chat_id: &str, text: &str) {
    store
        .append_conversation_turn(
            chat_id,
            NewConversationTurn {
                turn_id: format!("{chat_id}-turn-1"),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-06-19T00:00:00Z".to_string(),
                    assistant_started_at: Some("2026-06-19T00:00:01Z".to_string()),
                    assistant_completed_at: Some("2026-06-19T00:00:02Z".to_string()),
                    status: "completed".to_string(),
                },
                selected_variant_id: None,
                messages: vec![
                    message(&format!("{chat_id}-user-1"), "user", text),
                    message(&format!("{chat_id}-assistant-1"), "assistant", "ack"),
                ],
            },
        )
        .await
        .unwrap();
}

fn ctx(workspace_dir: &std::path::Path, session_id: &str) -> ToolContext {
    let mut metadata = std::collections::HashMap::new();
    metadata.insert(
        "rua_workspace_dir".to_string(),
        json!(workspace_dir.display().to_string()),
    );
    ToolContext {
        agent_id: "agent-1".into(),
        session_id: session_id.into(),
        working_dir: workspace_dir.to_path_buf(),
        tool_result_store_dir: Some(workspace_dir.join("tool_call_logs")),
        metadata,
        ..Default::default()
    }
}

fn text(result: &rua_core::tool::ToolResult) -> String {
    result
        .content
        .first()
        .and_then(|part| part.as_text())
        .unwrap_or_default()
        .to_string()
}

#[tokio::test]
async fn conversation_recall_tool_searches_current_chat_without_long_term_memory() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    store.save_chat_header(&header("chat-1")).await.unwrap();
    store.save_chat_header(&header("chat-2")).await.unwrap();
    append_turn(&store, "chat-1", "needle in current chat").await;
    append_turn(&store, "chat-2", "needle in other chat").await;

    let tool = rua_tool::extended::conversation::ConversationRecallTool::new();
    let result = tool
        .call(
            json!({"query": "needle", "max_results": 10, "surrounding_messages": 0}),
            ctx(dir.path(), "chat-1"),
        )
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_str(&text(&result)).unwrap();

    assert_eq!(body["chat_id"], "chat-1");
    assert_eq!(body["hits"].as_array().unwrap().len(), 1);
    assert_eq!(body["hits"][0]["message"]["message_id"], "chat-1-user-1");
    assert!(body["hits"][0]["snippet"]
        .as_str()
        .unwrap()
        .contains("needle in current chat"));
}

#[tokio::test]
async fn conversation_read_span_tool_expands_without_query() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    store.save_chat_header(&header("chat-1")).await.unwrap();
    append_turn(&store, "chat-1", "first").await;
    store
        .append_conversation_turn(
            "chat-1",
            NewConversationTurn {
                turn_id: "chat-1-turn-2".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-06-19T00:00:00Z".to_string(),
                    assistant_started_at: Some("2026-06-19T00:00:01Z".to_string()),
                    assistant_completed_at: Some("2026-06-19T00:00:02Z".to_string()),
                    status: "completed".to_string(),
                },
                selected_variant_id: None,
                messages: vec![
                    message("chat-1-user-2", "user", "needle"),
                    message("chat-1-assistant-2", "assistant", "second ack"),
                ],
            },
        )
        .await
        .unwrap();

    let tool = rua_tool::extended::conversation::ConversationReadSpanTool::new();
    let result = tool
        .call(
            json!({
                "start_sequence": 3,
                "end_sequence": 3,
                "expand_before": 1,
                "expand_after": 1,
            }),
            ctx(dir.path(), "chat-1"),
        )
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_str(&text(&result)).unwrap();

    assert_eq!(body["chat_id"], "chat-1");
    assert_eq!(body["span"]["start_sequence"], 2);
    assert_eq!(body["span"]["end_sequence"], 4);
    let ids: Vec<_> = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|message| message["message_id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        vec!["chat-1-assistant-1", "chat-1-user-2", "chat-1-assistant-2"]
    );

    let spec = rua_tool::extended::conversation::ConversationReadSpanTool::spec();
    assert!(spec.description.contains("History Replacement locator"));
    assert!(spec.description.contains("prior search is not required"));
}

#[tokio::test]
async fn history_replacement_span_can_recover_tool_result_archive_key() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    store.save_chat_header(&header("chat-1")).await.unwrap();
    let call_id = "archived-call-1";
    let recall_key = rua_tool::recall_key(call_id);
    let archive = Arc::new(rua_tool::ToolCallArchive::new(
        dir.path().join("tool_call_logs"),
    ));
    archive
        .save_call(
            call_id,
            "bash",
            &json!({"command": "printf exact-value"}),
            "exact-value-from-full-archive",
        )
        .await
        .unwrap();
    store
        .append_conversation_turn(
            "chat-1",
            NewConversationTurn {
                turn_id: "turn-1".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-06-19T00:00:00Z".to_string(),
                    assistant_started_at: Some("2026-06-19T00:00:01Z".to_string()),
                    assistant_completed_at: Some("2026-06-19T00:00:03Z".to_string()),
                    status: "completed".to_string(),
                },
                selected_variant_id: Some("variant-1".to_string()),
                messages: vec![
                    message("user-1", "user", "recover the exact value"),
                    NewConversationMessage {
                        message_id: "assistant-tool".to_string(),
                        role: "assistant".to_string(),
                        created_at: "2026-06-19T00:00:01Z".to_string(),
                        status: "completed".to_string(),
                        content: json!([{
                            "type": "tool_use",
                            "id": call_id,
                            "name": "bash",
                            "input": {"command": "printf exact-value"}
                        }]),
                        provider: None,
                        tool: None,
                        metadata: serde_json::Map::new(),
                    },
                    NewConversationMessage {
                        message_id: "tool-result".to_string(),
                        role: "tool".to_string(),
                        created_at: "2026-06-19T00:00:02Z".to_string(),
                        status: "completed".to_string(),
                        content: json!([{
                            "type": "tool_result",
                            "id": call_id,
                            "name": "bash",
                            "output": [{"type": "text", "text": "visible truncated output"}],
                            "truncation": {
                                "tool": "bash",
                                "recall_key": recall_key.clone(),
                                "total_lines": 1,
                                "total_bytes": 29
                            }
                        }]),
                        provider: None,
                        tool: None,
                        metadata: serde_json::Map::new(),
                    },
                    message("assistant-final", "assistant", "The command completed."),
                ],
            },
        )
        .await
        .unwrap();

    let read_span = rua_tool::extended::conversation::ConversationReadSpanTool::new()
        .call(
            json!({
                "start_sequence": 2,
                "end_sequence": 3
            }),
            ctx(dir.path(), "chat-1"),
        )
        .await
        .unwrap();
    let span_body: serde_json::Value = serde_json::from_str(&text(&read_span)).unwrap();
    assert_eq!(
        span_body["messages"][1]["content"][0]["truncation"]["recall_key"],
        recall_key
    );

    let recall_tool = rua_tool::extended::recall::RecallToolResultTool::new();
    let recalled = recall_tool
        .call(json!({"recall_key": recall_key}), ctx(dir.path(), "chat-1"))
        .await
        .unwrap();
    assert!(text(&recalled).contains("exact-value-from-full-archive"));
}
