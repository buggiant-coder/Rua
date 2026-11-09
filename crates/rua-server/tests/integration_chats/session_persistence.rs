use axum::http::{Method, StatusCode};
use serde_json::json;

use super::support::{
    accept_first_input, build_app, create_agent, create_chat, delete_agent, read_json, request,
    workspace_dir,
};
use rua_session::{NewConversationMessage, NewConversationTurn, TurnLifecycle};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_history_reads_persisted_session_messages() {
    let app = build_app();
    let agent_id = "integ_session_agent";
    create_agent(&app.app, agent_id, "Session persistence agent").await;

    let created = create_chat(
        &app.app,
        "/api/chats",
        Some(agent_id),
        "Session persistence chat",
        "session-user",
    )
    .await;
    let chat_id = created["id"].as_str().expect("chat id").to_string();
    accept_first_input(&app, agent_id, &chat_id, "session-user").await;

    let store = rua_session::WorkspaceSessionStore::new(workspace_dir(&app.data_dir, agent_id));
    store
        .append_conversation_turn(
            &chat_id,
            NewConversationTurn {
                turn_id: "turn-persisted-1".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-06-20T00:00:00Z".to_string(),
                    assistant_started_at: Some("2026-06-20T00:00:01Z".to_string()),
                    assistant_completed_at: Some("2026-06-20T00:00:02Z".to_string()),
                    status: "completed".to_string(),
                },
                selected_variant_id: None,
                messages: vec![
                    NewConversationMessage {
                        message_id: "msg-user-1".to_string(),
                        role: "user".to_string(),
                        created_at: "2026-06-20T00:00:00Z".to_string(),
                        status: "completed".to_string(),
                        content: json!("hello from persisted session"),
                        provider: None,
                        tool: None,
                        metadata: serde_json::Map::from_iter([(
                            "source".to_string(),
                            json!("integration_chats"),
                        )]),
                    },
                    NewConversationMessage {
                        message_id: "msg-assistant-1".to_string(),
                        role: "assistant".to_string(),
                        created_at: "2026-06-20T00:00:01Z".to_string(),
                        status: "completed".to_string(),
                        content: json!([
                            {"type": "thinking", "thinking": "checking persisted state"},
                            {"type": "text", "text": "persisted answer"}
                        ]),
                        provider: None,
                        tool: None,
                        metadata: serde_json::Map::from_iter([
                            ("source".to_string(), json!("integration_chats")),
                            (
                                "rua_reasoning_message_id".to_string(),
                                json!("msg-assistant-reasoning-1"),
                            ),
                            (
                                "rua_text_message_id".to_string(),
                                json!("msg-assistant-text-1"),
                            ),
                        ]),
                    },
                ],
            },
        )
        .await
        .expect("append evergreen turn");

    let history = request(
        &app.app,
        Method::GET,
        &format!("/api/chats/{chat_id}"),
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(history.status(), StatusCode::OK);
    let body = read_json(history).await;
    assert!(body.get("status").is_none());
    assert!(body.get("active_turn").is_none());
    assert!(body.get("control").is_none());
    let messages: Vec<_> = body["messages"]
        .as_array()
        .expect("messages array")
        .iter()
        .filter(|message| {
            [
                "msg-user-1",
                "msg-assistant-reasoning-1",
                "msg-assistant-text-1",
            ]
            .contains(&message["id"].as_str().unwrap_or(""))
        })
        .collect();
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0]["type"], "message");
    assert_eq!(messages[0]["role"], "user");
    assert_eq!(
        messages[0]["content"][0]["text"],
        "hello from persisted session"
    );
    assert_eq!(messages[1]["type"], "reasoning");
    assert_eq!(
        messages[1]["content"][0]["text"],
        "checking persisted state"
    );
    assert_eq!(messages[2]["type"], "message");
    assert_eq!(messages[2]["content"][0]["text"], "persisted answer");

    let deleted = request(
        &app.app,
        Method::DELETE,
        &format!("/api/chats/{chat_id}"),
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(deleted.status(), StatusCode::OK);
    delete_agent(&app.app, agent_id).await;
}
