use axum::http::{Method, StatusCode};
use rua_server::services::console_chat::StoredMessage;
use rua_session::{
    CheckpointModelIdentity, NewConversationMessage, NewConversationTurn, TurnLifecycle,
    WorkspaceSessionStore,
};
use serde_json::{json, Value};

use super::support::{
    build_app, create_agent, delete_agent, read_json, request, seed_materialized_chat,
};

fn stored_message(id: &str, role: &str, content: Vec<Value>) -> StoredMessage {
    StoredMessage {
        id: id.to_string(),
        role: role.to_string(),
        content,
        name: role.to_string(),
        timestamp: "2026-06-18T00:00:00Z".to_string(),
        device_id: String::new(),
        metadata: Value::Null,
        extra: serde_json::Map::new(),
    }
}

fn text_message(id: &str, role: &str, text: &str) -> StoredMessage {
    stored_message(id, role, vec![json!({"type": "text", "text": text})])
}

async fn persist_session(
    app: &super::support::TestApp,
    chat_id: &str,
    messages: Vec<StoredMessage>,
) {
    let workspace = app.state.rua_root.join("chats");
    WorkspaceSessionStore::new(&workspace)
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
                messages: messages
                    .into_iter()
                    .map(|message| NewConversationMessage {
                        message_id: message.id,
                        role: message.role,
                        created_at: message.timestamp,
                        status: "completed".to_string(),
                        content: Value::Array(message.content),
                        provider: None,
                        tool: None,
                        metadata: serde_json::Map::new(),
                    })
                    .collect(),
            },
        )
        .await
        .expect("append raw conversation turn");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_search_returns_console_facing_result_shape_from_text_content() {
    let app = build_app();
    let agent_id = "chat_search_contract_01";
    create_agent(&app.app, agent_id, "Chat search agent").await;

    let created = seed_materialized_chat(&app, agent_id, "Searchable Chat").await;
    let chat_id = created["id"].as_str().expect("chat id").to_string();
    persist_session(
        &app,
        &chat_id,
        vec![
            text_message("u1", "user", "Please remember the Rust-only alpha signal."),
            stored_message(
                "a1",
                "assistant",
                vec![
                    json!({"type": "image_url", "image_url": {"url": "file://ignored.png"}}),
                    json!({"type": "text", "text": "The alpha signal lives in the session search index."}),
                ],
            ),
        ],
    )
    .await;

    let resp = request(
        &app.app,
        Method::GET,
        "/api/chats/search?q=ALPHA",
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["offset"], 0);
    assert_eq!(body["total"], 2);
    assert!(body["next_offset"].is_null());
    let results = body["results"].as_array().expect("results array");
    assert_eq!(results.len(), 2);

    assert_eq!(results[0]["chat_id"], chat_id);
    assert_eq!(results[0]["chat_name"], "Searchable Chat");
    assert_eq!(results[0]["role"], "user");
    assert_eq!(results[0]["card_index"], 0);
    assert_eq!(results[0]["message_id"], "u1");
    assert_eq!(results[0]["turn_id"], format!("{chat_id}-turn-1"));
    assert_eq!(results[0]["sequence"], 1);
    assert_eq!(results[0]["span_start_sequence"], 1);
    assert_eq!(results[0]["span_end_sequence"], 1);
    assert!(results[0]["snippet"]
        .as_str()
        .unwrap_or_default()
        .contains("alpha"));

    assert_eq!(results[1]["chat_id"], chat_id);
    assert_eq!(results[1]["role"], "assistant");
    assert_eq!(results[1]["card_index"], 1);
    assert!(results[1]["snippet"]
        .as_str()
        .unwrap_or_default()
        .contains("alpha signal lives"));

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_search_ignores_summary_checkpoints_as_transcript_source() {
    let app = build_app();
    let agent_id = "chat_search_checkpoint_01";
    create_agent(&app.app, agent_id, "Checkpoint search agent").await;

    let created = seed_materialized_chat(&app, agent_id, "Checkpoint Search").await;
    let chat_id = created["id"].as_str().expect("chat id").to_string();
    persist_session(
        &app,
        &chat_id,
        vec![text_message(
            "u1",
            "user",
            "raw transcript deliberately omits the checkpoint-only token",
        )],
    )
    .await;

    let workspace = app.state.rua_root.join("chats");
    WorkspaceSessionStore::new(&workspace)
        .publish_manual_context_checkpoint(
            &chat_id,
            "checkpoint-only-token-zebra lives only in compressed context".to_string(),
            CheckpointModelIdentity {
                provider: "test".to_string(),
                model: "summary".to_string(),
            },
        )
        .await
        .expect("publish checkpoint");

    let resp = request(
        &app.app,
        Method::GET,
        "/api/chats/search?q=checkpoint-only-token-zebra",
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["total"], 0, "search body: {body:?}");
    assert!(body["results"].as_array().expect("results").is_empty());

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_search_filters_by_selected_agent_and_respects_limit() {
    let app = build_app();
    let filter_agent = "chat_search_filter_agent";
    let header_agent = "chat_search_header_agent";
    create_agent(&app.app, filter_agent, "Filter search agent").await;
    create_agent(&app.app, header_agent, "Header search agent").await;

    let path_chat = seed_materialized_chat(&app, filter_agent, "Agent-filtered Search").await;
    let path_chat_id = path_chat["id"].as_str().expect("path chat id");
    persist_session(
        &app,
        path_chat_id,
        vec![
            text_message("u1", "user", "needle appears in filtered chat"),
            text_message("t1", "tool", "needle inside a tool result"),
            text_message("a1", "assistant", "second needle should be hidden by limit"),
        ],
    )
    .await;

    let header_chat = seed_materialized_chat(&app, header_agent, "Header Scoped Search").await;
    let header_chat_id = header_chat["id"].as_str().expect("header chat id");
    persist_session(
        &app,
        header_chat_id,
        vec![text_message(
            "u1",
            "user",
            "needle belongs only to the conflicting header agent",
        )],
    )
    .await;

    let resp = request(
        &app.app,
        Method::GET,
        &format!("/api/chats/search?agent_id={filter_agent}&q=needle&limit=1"),
        None,
        Some(header_agent),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["offset"], 0);
    assert_eq!(body["limit"], 1);
    assert_eq!(body["total"], 2);
    assert_eq!(body["next_offset"], 1);
    let results = body["results"].as_array().expect("results array");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["chat_id"], path_chat["id"]);
    assert_eq!(results[0]["chat_name"], "Agent-filtered Search");
    assert!(results[0]["snippet"]
        .as_str()
        .unwrap_or_default()
        .contains("filtered chat"));

    let second = request(
        &app.app,
        Method::GET,
        &format!("/api/chats/search?agent_id={filter_agent}&q=needle&offset=1&limit=1"),
        None,
        None,
    )
    .await;
    let second = read_json(second).await;
    assert_eq!(second["total"], 2);
    assert_eq!(second["results"][0]["message_id"], "a1");
    assert_eq!(second["results"][0]["sequence"], 3);
    assert!(second["next_offset"].is_null());

    let empty = request(
        &app.app,
        Method::GET,
        &format!("/api/chats/search?agent_id={filter_agent}&q=%20%20"),
        None,
        None,
    )
    .await;
    assert_eq!(empty.status(), StatusCode::OK);
    assert!(read_json(empty).await["results"]
        .as_array()
        .expect("empty query results")
        .is_empty());

    delete_agent(&app.app, filter_agent).await;
    delete_agent(&app.app, header_agent).await;
}

#[tokio::test]
async fn chat_list_and_search_reject_conflicting_project_filters_even_without_search_text() {
    let app = build_app();
    for path in ["/api/chats", "/api/chats/search"] {
        let response = request(
            &app.app,
            Method::GET,
            &format!("{path}?project_id=photos&without_project=true"),
            None,
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(read_json(response).await["detail"]
            .as_str()
            .unwrap()
            .contains("without_project"));
    }
}
