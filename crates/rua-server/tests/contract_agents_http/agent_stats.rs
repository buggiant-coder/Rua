use axum::http::{Method, StatusCode};
use chrono::{DateTime, Utc};
use rua_server::services::chat_index;
use rua_session::{
    ChatMetadata, ChatSessionHeader, NewConversationMessage, NewConversationTurn, TurnLifecycle,
    WorkspaceSessionStore,
};
use serde_json::{json, Value};

use super::support::{
    build_app, create_agent, delete_agent, read_json, request_with_agent, seed_usage_file,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_stats_returns_qwenpaw_summary_shape_for_selected_agent() {
    let app = build_app();
    let agent_id = "integ_agent_stats_meta_01";
    create_agent(&app.app, agent_id, "Agent stats meta").await;
    seed_usage_file(
        &app,
        json!({
            "2026-06-01": {
                "openai:gpt-4": {
                    "provider_id": "openai",
                    "model_name": "gpt-4",
                    "prompt_tokens": 100,
                    "completion_tokens": 40,
                    "call_count": 2,
                    "agent_id": agent_id
                }
            },
            "2026-06-02": {
                "dashscope:qwen-plus": {
                    "provider_id": "dashscope",
                    "model_name": "qwen-plus",
                    "prompt_tokens": 25,
                    "completion_tokens": 5,
                    "call_count": 1,
                    "agent_id": agent_id
                }
            },
            "2026-06-03": {
                "openai:gpt-4o": {
                    "provider_id": "openai",
                    "model_name": "gpt-4o",
                    "prompt_tokens": 999,
                    "completion_tokens": 999,
                    "call_count": 9,
                    "agent_id": agent_id
                }
            }
        }),
    );
    seed_chat_stats(&app.data_dir, agent_id).await;

    let resp = request_with_agent(
        &app.app,
        Method::GET,
        "/api/agent-stats?start_date=2026-06-02&end_date=2026-06-01",
        None,
        agent_id,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;

    for key in [
        "total_active_sessions",
        "total_messages",
        "total_user_messages",
        "total_assistant_messages",
        "total_prompt_tokens",
        "total_completion_tokens",
        "total_llm_calls",
        "total_tool_calls",
    ] {
        assert!(
            body[key].as_u64().is_some(),
            "counter {key} should be a non-negative integer: {body:?}"
        );
    }

    assert_eq!(body["start_date"], "2026-06-01");
    assert_eq!(body["end_date"], "2026-06-02");
    assert_eq!(body["total_prompt_tokens"], 125);
    assert_eq!(body["total_completion_tokens"], 45);
    assert_eq!(body["total_llm_calls"], 3);
    assert_eq!(body["total_active_sessions"], 1);
    assert_eq!(body["total_user_messages"], 1);
    assert_eq!(body["total_assistant_messages"], 1);
    assert_eq!(body["total_messages"], 2);
    assert_eq!(body["total_tool_calls"], 1);
    assert_eq!(
        body["by_date"],
        json!([
            day_with_usage_and_chat_stats("2026-06-01", 100, 40, 2, 1, 1, 1, 1, 2, 1),
            day_with_usage("2026-06-02", 25, 5, 1),
        ])
    );
    assert_eq!(
        body["channel_stats"],
        json!([{
            "channel": "console",
            "chats": 1,
            "active_sessions": 1,
            "user_messages": 1,
            "assistant_messages": 1,
            "total_messages": 2,
            "tool_calls": 1,
        }])
    );

    delete_agent(&app.app, agent_id).await;
}

async fn seed_chat_stats(data_dir: &std::path::Path, agent_id: &str) {
    let workspace_dir = data_dir.join("workspaces").join(agent_id);
    std::fs::create_dir_all(&workspace_dir).unwrap();
    let chat_id = "stats-chat-1";
    let created_at = "2026-06-01T08:00:00Z";
    let updated_at = "2026-06-01T08:05:00Z";
    let store = WorkspaceSessionStore::new(&workspace_dir);
    store
        .save_chat_header(&ChatSessionHeader::new(ChatMetadata {
            chat_id: chat_id.to_string(),
            agent_id: agent_id.to_string(),
            title: "Stats chat".to_string(),
            created_at: created_at.to_string(),
            last_activity_at: updated_at.to_string(),
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
                turn_id: "turn-stats-1".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: created_at.to_string(),
                    assistant_started_at: Some("2026-06-01T08:00:01Z".to_string()),
                    assistant_completed_at: Some(updated_at.to_string()),
                    status: "completed".to_string(),
                },
                selected_variant_id: None,
                messages: vec![
                    NewConversationMessage {
                        message_id: "msg-user-1".to_string(),
                        role: "user".to_string(),
                        created_at: created_at.to_string(),
                        status: "completed".to_string(),
                        content: json!([{ "type": "text", "text": "hello" }]),
                        provider: None,
                        tool: None,
                        metadata: serde_json::Map::new(),
                    },
                    NewConversationMessage {
                        message_id: "msg-assistant-1".to_string(),
                        role: "assistant".to_string(),
                        created_at: "2026-06-01T08:00:02Z".to_string(),
                        status: "completed".to_string(),
                        content: json!([
                            { "type": "text", "text": "checking" },
                            {
                                "type": "tool_use",
                                "id": "tool-call-1",
                                "name": "read",
                                "arguments": { "path": "README.md" }
                            }
                        ]),
                        provider: None,
                        tool: None,
                        metadata: serde_json::Map::new(),
                    },
                ],
            },
        )
        .await
        .unwrap();
    chat_index::upsert(
        &workspace_dir,
        chat_index::ChatIndexEntry {
            id: chat_id.to_string(),
            name: "Stats chat".to_string(),
            user_id: "default".to_string(),
            channel: "console".to_string(),
            created_at: DateTime::parse_from_rfc3339(created_at)
                .unwrap()
                .with_timezone(&Utc),
            updated_at: DateTime::parse_from_rfc3339(updated_at)
                .unwrap()
                .with_timezone(&Utc),
            meta: json!({}),
            status: "active".to_string(),
            pinned: false,
            pin_order: 0,
            tag: rua_session::ChatTag::None,
            archived: false,
        },
    )
    .await
    .unwrap();
}

fn day_with_usage(date: &str, prompt_tokens: u64, completion_tokens: u64, llm_calls: u64) -> Value {
    day_with_usage_and_chat_stats(
        date,
        prompt_tokens,
        completion_tokens,
        llm_calls,
        0,
        0,
        0,
        0,
        0,
        0,
    )
}

#[allow(clippy::too_many_arguments)]
fn day_with_usage_and_chat_stats(
    date: &str,
    prompt_tokens: u64,
    completion_tokens: u64,
    llm_calls: u64,
    chats: u64,
    active_sessions: u64,
    user_messages: u64,
    assistant_messages: u64,
    total_messages: u64,
    tool_calls: u64,
) -> Value {
    json!({
        "date": date,
        "chats": chats,
        "active_sessions": active_sessions,
        "user_messages": user_messages,
        "assistant_messages": assistant_messages,
        "total_messages": total_messages,
        "prompt_tokens": prompt_tokens,
        "completion_tokens": completion_tokens,
        "llm_calls": llm_calls,
        "tool_calls": tool_calls,
    })
}
