use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use chrono::{DateTime, Utc};
use http_body_util::BodyExt;
use rua_core::ChatAddress;
use rua_server::services::chat_index::{self, ChatIndexEntry};
use rua_server::{endpoints, AppState, ServerProfile};
use rua_session::{
    ChatMetadata, ChatSessionHeader, NewConversationMessage, NewConversationTurn, TurnLifecycle,
    WorkspaceSessionStore,
};
use serde_json::json;
use tempfile::TempDir;
use tower::ServiceExt;

fn build_app() -> (Router, AppState, TempDir) {
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path().join("workspaces/default");
    std::fs::create_dir_all(&workspace).expect("workspace");
    std::fs::write(
        workspace.join("agent.json"),
        serde_json::to_vec_pretty(&json!({
            "id": "default",
            "name": "Default Agent",
            "description": "",
            "workspace_dir": workspace,
            "language": "",
            "system_prompt_files": []
        }))
        .expect("agent json"),
    )
    .expect("seed agent");
    let state = AppState::from_tempdir(temp.path().to_path_buf(), ServerProfile::Test);
    let api = Router::new()
        .merge(endpoints::storage::router())
        .merge(endpoints::chats::root_router())
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            rua_server::middleware::agent_context_middleware,
        ));
    let app = Router::new().nest("/api", api).with_state(state.clone());
    (app, state, temp)
}

async fn seed_chat(
    state: &AppState,
    chat_id: &str,
    name: &str,
    updated_at: &str,
    pinned: bool,
    content: serde_json::Value,
) {
    let workspace = state.rua_root.join("workspaces/default");
    let timestamp = DateTime::parse_from_rfc3339(updated_at)
        .expect("timestamp")
        .with_timezone(&Utc);
    chat_index::upsert(
        &workspace,
        ChatIndexEntry {
            id: chat_id.to_string(),
            name: name.to_string(),
            user_id: "default".to_string(),
            channel: "console".to_string(),
            created_at: timestamp,
            updated_at: timestamp,
            meta: json!({}),
            status: "idle".to_string(),
            pinned,
            pin_order: 0,
            tag: rua_session::ChatTag::None,
            archived: false,
        },
    )
    .await
    .expect("index");
    let store = WorkspaceSessionStore::new(&workspace);
    store
        .save_chat_header(&ChatSessionHeader::new(ChatMetadata {
            chat_id: chat_id.to_string(),
            agent_id: "default".to_string(),
            title: name.to_string(),
            created_at: updated_at.to_string(),
            last_activity_at: updated_at.to_string(),
            pinned,
            archived: false,
            preview: None,
        }))
        .await
        .expect("header");
    store
        .append_conversation_turn(
            chat_id,
            NewConversationTurn {
                turn_id: format!("turn-{chat_id}"),
                lifecycle: TurnLifecycle {
                    user_submitted_at: updated_at.to_string(),
                    assistant_started_at: None,
                    assistant_completed_at: Some(updated_at.to_string()),
                    status: "completed".to_string(),
                },
                selected_variant_id: None,
                messages: vec![NewConversationMessage {
                    message_id: format!("message-{chat_id}"),
                    role: "user".to_string(),
                    created_at: updated_at.to_string(),
                    status: "completed".to_string(),
                    content,
                    provider: None,
                    tool: None,
                    metadata: serde_json::Map::new(),
                }],
            },
        )
        .await
        .expect("messages");
}

async fn json_request(
    app: &Router,
    method: Method,
    uri: &str,
    body: Option<serde_json::Value>,
) -> axum::response::Response {
    let mut builder = Request::builder().method(method).uri(uri);
    builder = builder.header("X-Agent-Id", "default");
    let body = match body {
        Some(body) => {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
            Body::from(body.to_string())
        }
        None => Body::empty(),
    };
    app.clone()
        .oneshot(builder.body(body).expect("request"))
        .await
        .expect("response")
}

async fn read_json(response: axum::response::Response) -> serde_json::Value {
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    serde_json::from_slice(&bytes).expect("json")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn storage_chat_list_reports_breakdown_sorting_and_before_filter() {
    let (app, state, _temp) = build_app();
    let media_id = "00000000-0000-4000-8000-000000000101";
    seed_chat(
        &state,
        "chat-small",
        "Small",
        "2026-08-20T00:00:00Z",
        false,
        json!([{"type": "text", "text": "small"}]),
    )
    .await;
    seed_chat(
        &state,
        "chat-large",
        "Large",
        "2026-08-21T00:00:00Z",
        false,
        json!([{
            "type": "container",
            "nested": {
                "tool_use_id": "tool-large",
                "attachment": format!("media/{media_id}")
            }
        }]),
    )
    .await;

    let workspace = state.rua_root.join("workspaces/default");
    let store = WorkspaceSessionStore::new(&workspace);
    let task_dir = store.chat_dir("chat-large").join("tasks/task-1");
    std::fs::create_dir_all(&task_dir).expect("task dir");
    std::fs::write(task_dir.join("result.bin"), b"1234567").expect("task bytes");
    let tool_key = rua_tool::result_store::recall_key("tool-large");
    std::fs::create_dir_all(workspace.join("tool_call_logs")).expect("tool log dir");
    std::fs::write(
        workspace
            .join("tool_call_logs")
            .join(format!("{tool_key}.json")),
        b"12345",
    )
    .expect("tool json");
    std::fs::write(
        workspace
            .join("tool_call_logs")
            .join(format!("{tool_key}.output")),
        b"123456",
    )
    .expect("tool output");
    std::fs::create_dir_all(workspace.join("media")).expect("media dir");
    std::fs::write(workspace.join("media").join(media_id), b"1234").expect("media");
    std::fs::write(
        workspace
            .join("media")
            .join(format!(".{media_id}.media.json")),
        b"123",
    )
    .expect("media metadata");

    let response = json_request(
        &app,
        Method::GET,
        "/api/storage/chats?sort_by=size&sort_order=desc",
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = read_json(response).await;
    assert_eq!(
        body[0]["name"], "Large",
        "unexpected Storage response: {body}"
    );
    assert_eq!(body[0]["task_bytes"], 7);
    assert_eq!(body[0]["tool_log_bytes"], 11);
    assert_eq!(body[0]["media_bytes"], 7);
    assert_eq!(body[0]["attributed_bytes"], body[0]["total_bytes"]);
    assert_eq!(body[0]["reclaimable_bytes"], body[0]["total_bytes"]);
    assert_eq!(
        body[0]["total_bytes"].as_u64().unwrap(),
        body[0]["session_bytes"].as_u64().unwrap() + 25
    );

    let response = json_request(
        &app,
        Method::GET,
        "/api/storage/chats?before=2026-08-21T00%3A00%3A00Z",
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = read_json(response).await;
    assert_eq!(body.as_array().unwrap().len(), 1);
    assert_eq!(body[0]["name"], "Small");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn storage_preview_deduplicates_shared_media_and_reports_reclaimable_bytes() {
    let (app, state, _temp) = build_app();
    let shared_media = "00000000-0000-4000-8000-000000000151";
    for (chat_id, name) in [("chat-preview-a", "A"), ("chat-preview-b", "B")] {
        seed_chat(
            &state,
            chat_id,
            name,
            "2026-08-21T00:00:00Z",
            false,
            json!([{"media": format!("media/{shared_media}")}]),
        )
        .await;
    }
    let workspace = state.rua_root.join("workspaces/default");
    std::fs::create_dir_all(workspace.join("media")).unwrap();
    std::fs::write(workspace.join("media").join(shared_media), b"shared-media").unwrap();

    let list = json_request(&app, Method::GET, "/api/storage/chats", None).await;
    let list = read_json(list).await;
    assert_eq!(list[0]["media_bytes"], 12);
    assert_eq!(list[1]["media_bytes"], 12);
    assert!(
        list[0]["reclaimable_bytes"].as_u64().unwrap()
            < list[0]["attributed_bytes"].as_u64().unwrap()
    );

    let one = json_request(
        &app,
        Method::POST,
        "/api/storage/chats/preview",
        Some(json!({"addresses": [ChatAddress::agent("default", "chat-preview-a")]})),
    )
    .await;
    let one = read_json(one).await;
    assert_eq!(one["chat_count"], 1);
    assert_eq!(
        one["shared_media_preserved"][0]["media_ids"][0],
        shared_media
    );
    assert!(one["reclaimable_bytes"].as_u64().unwrap() < one["attributed_bytes"].as_u64().unwrap());

    let both = json_request(
        &app,
        Method::POST,
        "/api/storage/chats/preview",
        Some(json!({"addresses": [
            ChatAddress::agent("default", "chat-preview-a"),
            ChatAddress::agent("default", "chat-preview-b")
        ]})),
    )
    .await;
    let both = read_json(both).await;
    assert_eq!(both["chat_count"], 2);
    assert!(both["shared_media_preserved"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(
        both["attributed_bytes"].as_u64().unwrap() - both["reclaimable_bytes"].as_u64().unwrap(),
        12
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_chat_delete_plan_removes_session_tasks_and_tool_archives_idempotently() {
    let (_app, state, _temp) = build_app();
    seed_chat(
        &state,
        "chat-local-apply",
        "Local Apply",
        "2026-08-21T00:00:00Z",
        false,
        json!([{"tool_use_id": "tool-local-apply"}]),
    )
    .await;
    let workspace = state.rua_root.join("workspaces/default");
    let store = WorkspaceSessionStore::new(&workspace);
    let chat_dir = store.chat_dir("chat-local-apply");
    let task_dir = chat_dir.join("tasks/task-local-apply");
    std::fs::create_dir_all(&task_dir).expect("task dir");
    std::fs::write(task_dir.join("result.bin"), b"task").expect("task artifact");
    let tool_key = rua_tool::result_store::recall_key("tool-local-apply");
    let tool_dir = workspace.join("tool_call_logs");
    std::fs::create_dir_all(&tool_dir).expect("tool log dir");
    let tool_json = tool_dir.join(format!("{tool_key}.json"));
    let tool_output = tool_dir.join(format!("{tool_key}.output"));
    std::fs::write(&tool_json, b"tool-json").expect("tool json");
    std::fs::write(&tool_output, b"tool-output").expect("tool output");
    let address = ChatAddress::agent("default", "chat-local-apply");

    rua_server::services::chat_deletion::LocalChatDeletionPlan::prepare(
        &state.rua_root,
        address.clone(),
    )
    .await
    .expect("prepare local delete")
    .apply()
    .await
    .expect("apply local delete");

    assert!(!chat_dir.exists());
    assert!(!tool_json.exists());
    assert!(!tool_output.exists());
    assert!(chat_index::get(&workspace, "chat-local-apply")
        .await
        .expect("index")
        .is_none());

    rua_server::services::chat_deletion::LocalChatDeletionPlan::prepare(&state.rua_root, address)
        .await
        .expect("prepare repeated local delete")
        .apply()
        .await
        .expect("repeat local delete");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pinned_chat_is_protected_from_storage_single_and_batch_delete() {
    let (app, state, _temp) = build_app();
    seed_chat(
        &state,
        "chat-pinned",
        "Pinned",
        "2026-08-21T00:00:00Z",
        true,
        json!([{"type": "text", "text": "keep"}]),
    )
    .await;
    seed_chat(
        &state,
        "chat-deletable",
        "Deletable",
        "2026-08-21T00:00:00Z",
        false,
        json!([{"type": "text", "text": "delete"}]),
    )
    .await;
    let pinned = ChatAddress::agent("default", "chat-pinned");
    let deletable = ChatAddress::agent("default", "chat-deletable");

    let response = json_request(
        &app,
        Method::POST,
        "/api/storage/chats/delete",
        Some(json!({
            "addresses": [pinned, deletable],
            "acknowledge_data_loss": true
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let result = read_json(response).await;
    assert_eq!(result["deleted"].as_array().unwrap().len(), 1);
    assert_eq!(result["skipped"].as_array().unwrap().len(), 1);
    assert!(result["failed"].as_array().unwrap().is_empty());
    assert!(result["pending"].as_array().unwrap().is_empty());
    let ordinary = json_request(&app, Method::DELETE, "/api/chats/chat-pinned", None).await;
    assert_eq!(ordinary.status(), StatusCode::CONFLICT);
    let batch = json_request(
        &app,
        Method::POST,
        "/api/chats/batch-delete",
        Some(json!(["chat-pinned"])),
    )
    .await;
    assert_eq!(batch.status(), StatusCode::CONFLICT);
    assert!(
        chat_index::get(&state.rua_root.join("workspaces/default"), "chat-pinned")
            .await
            .expect("index")
            .is_some()
    );
    assert!(
        chat_index::get(&state.rua_root.join("workspaces/default"), "chat-deletable")
            .await
            .expect("index")
            .is_none()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn storage_delete_removes_owned_artifacts_and_preserves_shared_media() {
    let (app, state, _temp) = build_app();
    let shared_media = "00000000-0000-4000-8000-000000000201";
    let owned_media = "00000000-0000-4000-8000-000000000202";
    seed_chat(
        &state,
        "chat-delete",
        "Delete",
        "2026-08-21T00:00:00Z",
        false,
        json!([{
            "tool_use_id": "tool-delete",
            "media": [format!("media/{shared_media}"), format!("media/{owned_media}")]
        }]),
    )
    .await;
    seed_chat(
        &state,
        "chat-keep",
        "Keep",
        "2026-08-20T00:00:00Z",
        false,
        json!([{"media_ref": format!("media/{shared_media}")}]),
    )
    .await;
    let workspace = state.rua_root.join("workspaces/default");
    let media_dir = workspace.join("media");
    std::fs::create_dir_all(&media_dir).expect("media dir");
    std::fs::write(media_dir.join(shared_media), b"shared").expect("shared media");
    std::fs::write(media_dir.join(owned_media), b"owned").expect("owned media");
    let tool_key = rua_tool::result_store::recall_key("tool-delete");
    std::fs::create_dir_all(workspace.join("tool_call_logs")).expect("tool log dir");
    std::fs::write(
        workspace
            .join("tool_call_logs")
            .join(format!("{tool_key}.json")),
        b"tool-log",
    )
    .expect("tool log");

    let response = json_request(
        &app,
        Method::POST,
        "/api/storage/chats/delete",
        Some(json!({
            "addresses": [ChatAddress::agent("default", "chat-delete")],
            "acknowledge_data_loss": true
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = read_json(response).await;
    assert_eq!(body["deleted"].as_array().unwrap().len(), 1);
    assert!(body["freed_bytes"].as_u64().unwrap() > 0);
    assert_eq!(
        body["shared_media_preserved"][0]["media_ids"][0],
        shared_media
    );
    assert!(media_dir.join(shared_media).is_file());
    assert!(!media_dir.join(owned_media).exists());
    assert!(!workspace
        .join("tool_call_logs")
        .join(format!("{tool_key}.json"))
        .exists());
    assert!(chat_index::get(&workspace, "chat-delete")
        .await
        .expect("index")
        .is_none());
    assert!(chat_index::get(&workspace, "chat-keep")
        .await
        .expect("index")
        .is_some());
}

#[tokio::test]
async fn deleting_chat_without_media_does_not_read_unrelated_history() {
    let (_app, state, _temp) = build_app();
    for id in ["target", "unrelated"] {
        seed_chat(
            &state,
            id,
            id,
            "2026-11-03T00:00:00Z",
            false,
            json!([{"type": "text", "text": "plain text"}]),
        )
        .await;
    }
    let workspace = state.rua_root.join("workspaces/default");
    // An unrelated history must not be a dependency of a media-free deletion.
    std::fs::write(
        workspace.join("sessions/unrelated/session.json"),
        b"invalid",
    )
    .unwrap();
    let prepared = rua_server::services::chat_storage::prepare_chat_deletion(
        &state,
        &workspace,
        rua_core::ChatScope::agent("default"),
        "Default".into(),
        &std::collections::BTreeSet::from(["target".to_string()]),
    )
    .await
    .expect("delete preparation should only inspect the target");
    assert_eq!(prepared.snapshot.entries.len(), 1);
    assert_eq!(
        prepared.snapshot.entries[0].address.chat_id().as_str(),
        "target"
    );
}

#[tokio::test]
async fn deletion_scan_preserves_shared_media_and_batch_accounting() {
    let (_app, state, _temp) = build_app();
    let media = "00000000-0000-4000-8000-000000000551";
    for id in ["first", "second"] {
        seed_chat(
            &state,
            id,
            id,
            "2026-11-03T00:00:00Z",
            false,
            json!([{"media": format!("media/{media}")}]),
        )
        .await;
    }
    let workspace = state.rua_root.join("workspaces/default");
    std::fs::create_dir_all(workspace.join("media")).unwrap();
    std::fs::write(workspace.join("media").join(media), b"shared").unwrap();
    let full = rua_server::services::chat_storage::scan_scope(
        &workspace,
        rua_core::ChatScope::agent("default"),
        "Default".into(),
    )
    .await
    .unwrap();
    for ids in [vec!["first"], vec!["first", "second"]] {
        let deleting = ids.into_iter().map(str::to_string).collect();
        let selected = rua_server::services::chat_storage::scan_scope_for_deletion(
            &workspace,
            rua_core::ChatScope::agent("default"),
            "Default".into(),
            &deleting,
        )
        .await
        .unwrap();
        assert_eq!(selected.entries.len(), deleting.len());
        assert_eq!(
            rua_server::services::chat_storage::removable_media(&selected, &deleting),
            rua_server::services::chat_storage::removable_media(&full, &deleting)
        );
        for entry in &selected.entries {
            let original = full
                .entries
                .iter()
                .find(|other| other.address == entry.address)
                .unwrap();
            assert_eq!(entry.reclaimable_bytes, original.reclaimable_bytes);
            assert_eq!(entry.total_bytes, original.total_bytes);
        }
    }
}
