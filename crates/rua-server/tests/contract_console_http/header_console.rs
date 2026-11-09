use axum::http::{Method, StatusCode};
use rua_core::ChatAddress;
use rua_provider::config_loader::{DefaultModel, ProviderConfig};
use rua_security::{ApprovalRequestInput, ApprovalTrigger, ToolCallSnapshot};
use rua_session::WorkspaceSessionStore;
use rua_sync::evergreen_chat_adapter::collect_evergreen_chat_file_snapshots;
use serde_json::json;

use super::support::{build_app, read_json, request, request_json};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn root_console_routes_are_removed() {
    let test = build_app();
    for (method, path) in [
        (Method::POST, "/console/chat/submit"),
        (Method::POST, "/console/chat/live"),
        (Method::POST, "/console/chat/stop?chat_id=missing-chat"),
        (Method::GET, "/console/push-messages"),
        (Method::GET, "/console/recent_messages"),
    ] {
        let response = request_json(
            &test.app,
            method,
            path,
            json!({"chat_id": "missing-chat", "message": "hello", "viewer_id": "viewer"}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_scoped_stop_requires_explicit_turn_and_authority_epoch() {
    let test = build_app();
    let path = "/agents/default/chats/00000000-0000-4000-8000-000000000431/console/stop";
    for body in [json!({}), json!({"turn_id": "observed-turn"})] {
        let response = request_json(&test.app, Method::POST, path, body).await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_scoped_push_messages_returns_rust_console_shape() {
    let test = build_app();

    let resp = request(
        &test.app,
        Method::GET,
        "/agents/default/console/push-messages",
    )
    .await;

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        read_json(resp).await,
        json!({"messages": [], "pending_approvals": []})
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn header_chat_submit_materializes_default_model_with_auto_thinking() {
    let test = build_app();
    let chat_id = uuid::Uuid::new_v4().to_string();
    test.state
        .provider_manager
        .store_default_model(DefaultModel {
            provider_id: "dashscope".into(),
            model: "qwen3.7-plus".into(),
            source: None,
            extra: Default::default(),
        })
        .await;

    let resp = request_json(
        &test.app,
        Method::POST,
        &format!("/agents/default/chats/{chat_id}/console/submit"),
        json!({
            "chat_id": chat_id,
            "input": [{
                "role": "user",
                "content": "hello"
            }]
        }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let _body = read_json(resp).await;

    let runtime = test
        .state
        .registry
        .get_or_create("default")
        .await
        .expect("runtime");
    let header = WorkspaceSessionStore::new(runtime.workspace_dir())
        .load_chat_header(&chat_id)
        .await
        .expect("load header")
        .expect("header exists");
    assert_eq!(
        header.extra.get("model"),
        Some(&json!({
            "provider_id": "dashscope",
            "model": "qwen3.7-plus",
            "thinking_strength": "auto",
            "thinking_enabled": null,
            "thinking_budget": null,
            "reasoning_effort": null,
            "thinking_param_style": "budget",
            "thinking_budget_range": [1, 81920],
            "reasoning_effort_options": []
        }))
    );
    assert!(header.extra["authority_placement"].is_object());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_scoped_first_submit_materializes_fixed_selection_and_policy() {
    let test = build_app();
    let chat_id = uuid::Uuid::new_v4().to_string();

    let response = request_json(
        &test.app,
        Method::POST,
        &format!("/agents/default/chats/{chat_id}/console/submit"),
        json!({
            "chat_id": chat_id,
            "input": [{
                "role": "user",
                "content": "materialize through the shared Chat module"
            }]
        }),
    )
    .await;
    let status = response.status();
    let body = read_json(response).await;
    assert_eq!(status, StatusCode::OK, "response: {body}");

    let runtime = test
        .state
        .registry
        .get_or_create("default")
        .await
        .expect("runtime");
    let store = WorkspaceSessionStore::new(runtime.workspace_dir());
    let header = store
        .load_chat_header(&chat_id)
        .await
        .expect("load header")
        .expect("header exists");
    assert_eq!(
        header.extra.get("selected_agent_id"),
        Some(&json!("default")),
        "Session File extra: {:?}",
        header.extra
    );
    assert_eq!(
        header.extra["chat_policy"],
        json!({
            "agent_assignment": "fixed",
            "resource": "none",
            "authority_placement": "selectable"
        })
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn header_chat_submit_materializes_structured_selected_model() {
    let test = build_app();
    let chat_id = uuid::Uuid::new_v4().to_string();
    test.state
        .provider_cells
        .cell("fixture")
        .write(
            serde_json::from_value::<ProviderConfig>(json!({
                "id": "fixture",
                "name": "Fixture",
                "base_url": "https://example.invalid/v1",
                "api_key": "test-key",
                "chat_model": "OpenAIChatModel",
                "models": [{
                    "id": "selected-model",
                    "name": "Selected Model",
                    "thinking_param_style": "budget",
                    "thinking_budget_range": [1, 8192]
                }]
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    test.state
        .provider_manager
        .reload_provider("fixture")
        .await
        .unwrap();

    let resp = request_json(
        &test.app,
        Method::POST,
        &format!("/agents/default/chats/{chat_id}/console/submit"),
        json!({
            "chat_id": chat_id,
            "input": [{
                "role": "user",
                "content": "hello selected model"
            }],
            "model": {
                "provider_id": "fixture",
                "model": "selected-model",
                "thinking_strength": "high"
            }
        }),
    )
    .await;
    let status = resp.status();
    let body = read_json(resp).await;
    assert_eq!(status, StatusCode::OK, "response: {body}");

    let runtime = test
        .state
        .registry
        .get_or_create("default")
        .await
        .expect("runtime");
    let workspace_dir = runtime.workspace_dir();
    let header = WorkspaceSessionStore::new(&workspace_dir)
        .load_chat_header(&chat_id)
        .await
        .expect("load header")
        .expect("header exists");
    assert_eq!(header.extra["model"]["provider_id"], json!("fixture"));
    assert_eq!(header.extra["model"]["model"], json!("selected-model"));
    assert_eq!(header.extra["model"]["thinking_strength"], json!("high"));

    let session_path = workspace_dir
        .join("sessions")
        .join(&chat_id)
        .join("session.json");
    let persisted: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&session_path).expect("read Evergreen session file"))
            .expect("parse Evergreen session file");
    assert_eq!(persisted["model"], header.extra["model"]);

    let snapshots = collect_evergreen_chat_file_snapshots(&workspace_dir, &chat_id, "device-test");
    let session_snapshot = snapshots
        .iter()
        .find(|snapshot| snapshot.relative == "session")
        .expect("Evergreen session snapshot");
    assert_eq!(session_snapshot.payload["model"], header.extra["model"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_chat_model_update_changes_session_without_rewriting_chat_index() {
    let test = build_app();
    let chat_id = uuid::Uuid::new_v4().to_string();
    test.state
        .provider_cells
        .cell("fixture")
        .write(
            serde_json::from_value::<ProviderConfig>(json!({
                "id": "fixture",
                "name": "Fixture",
                "base_url": "https://example.invalid/v1",
                "api_key": "test-key",
                "chat_model": "OpenAIChatModel",
                "models": [{
                    "id": "selected-model",
                    "name": "Selected Model",
                    "thinking_param_style": "budget",
                    "thinking_budget_range": [1, 8192]
                }]
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    test.state
        .provider_manager
        .reload_provider("fixture")
        .await
        .unwrap();
    let response = request_json(
        &test.app,
        Method::POST,
        &format!("/agents/default/chats/{chat_id}/console/submit"),
        json!({
            "chat_id": chat_id,
            "input": [{"role": "user", "content": "create model chat"}],
            "model": {
                "provider_id": "fixture",
                "model": "selected-model",
                "thinking_strength": "low"
            }
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let runtime = test
        .state
        .registry
        .get_or_create("default")
        .await
        .expect("runtime");
    let workspace = runtime.workspace_dir();
    let index_before = rua_session::chat_index::get(&workspace, &chat_id)
        .await
        .expect("read Chat Index")
        .expect("Chat Index row");

    test.state
        .conversation_input_intake
        .update_authority_chat_model(
            "default",
            &chat_id,
            json!({
                "provider_id": "fixture",
                "model": "selected-model",
                "thinking_strength": "high"
            }),
        )
        .await
        .expect("update Chat model");

    let index_after = rua_session::chat_index::get(&workspace, &chat_id)
        .await
        .expect("read unchanged Chat Index")
        .expect("Chat Index row");
    assert_eq!(index_after, index_before);
    let header = WorkspaceSessionStore::new(&workspace)
        .load_chat_header(&chat_id)
        .await
        .expect("load Session File")
        .expect("Session File");
    assert_eq!(header.extra["model"]["thinking_strength"], "high");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_chat_history_reads_current_configuration_without_chat_index_row() {
    let test = build_app();
    let chat_id = uuid::Uuid::new_v4().to_string();
    let response = request_json(
        &test.app,
        Method::POST,
        &format!("/agents/default/chats/{chat_id}/console/submit"),
        json!({
            "chat_id": chat_id,
            "input": [{"role": "user", "content": "Session authority history"}]
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let runtime = test
        .state
        .registry
        .get_or_create("default")
        .await
        .expect("runtime");
    let workspace = runtime.workspace_dir();
    let address = ChatAddress::agent("default", &chat_id);
    let service = rua_chat::ChatService::for_resolved_scope(&workspace, address.clone());
    service
        .update_listing(
            &address,
            rua_chat::ChatListUpdate {
                title: Some("Session-owned title".to_string()),
                pinned: None,
                tag: None,
                archived: None,
            },
        )
        .await
        .expect("update Session title");
    service
        .update_setting(
            &address,
            rua_chat::ChatSetting::Model(json!({
                "provider_id": "fixture",
                "model": "session-model",
                "thinking_strength": "medium"
            })),
        )
        .await
        .expect("update Session model");
    rua_session::chat_index::delete(&workspace, &chat_id)
        .await
        .expect("remove repairable Chat Index projection");

    let history = request(
        &test.app,
        Method::GET,
        &format!("/api/agents/default/chats/{chat_id}"),
    )
    .await;
    assert_eq!(history.status(), StatusCode::OK);
    let history = read_json(history).await;
    assert_eq!(history["title"], "Session-owned title");
    assert_eq!(history["model"]["model"], "session-model");
    assert_eq!(history["meta"]["selected_agent_id"], "default");
    assert_eq!(history["meta"]["chat_policy"]["agent_assignment"], "fixed");
    assert!(history["meta"]["authority_placement"].is_object());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn header_chat_submit_remains_available_after_sync_is_disabled() {
    let test = build_app();
    test.state
        .provider_manager
        .store_default_model(DefaultModel {
            provider_id: "dashscope".into(),
            model: "qwen3.7-plus".into(),
            source: None,
            extra: Default::default(),
        })
        .await;
    let identity_before = test
        .state
        .sync_device
        .read()
        .expect("sync device lock")
        .as_ref()
        .map(|device| hex::encode(device.device_id_short()))
        .expect("initial Device identity");

    let enable = request(&test.app, Method::POST, "/api/sync/enable").await;
    assert_eq!(enable.status(), StatusCode::OK);
    let disable = request(&test.app, Method::POST, "/api/sync/disable").await;
    assert_eq!(disable.status(), StatusCode::OK);

    let identity_after = test
        .state
        .sync_device
        .read()
        .expect("sync device lock")
        .as_ref()
        .map(|device| hex::encode(device.device_id_short()));
    assert_eq!(
        identity_after.as_deref(),
        Some(identity_before.as_str()),
        "disabling sync must preserve the Device identity used by local Chat authority"
    );

    let chat_id = uuid::Uuid::new_v4().to_string();
    let response = request_json(
        &test.app,
        Method::POST,
        &format!("/agents/default/chats/{chat_id}/console/submit"),
        json!({
            "chat_id": chat_id,
            "input": [{
                "role": "user",
                "content": "hello after disabling sync"
            }]
        }),
    )
    .await;
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "local Chat submit must remain available after disabling sync: {}",
        read_json(response).await
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn header_push_messages_includes_pending_approvals_for_selected_agent() {
    let test = build_app();
    let approval = test
        .state
        .security
        .approval_authority()
        .create_request(ApprovalRequestInput {
            agent_id: "default".into(),
            owner: rua_security::ApprovalOwner::chat(rua_core::ChatAddress::agent(
                "default", "compat",
            )),
            turn_id: "compat".into(),
            tool_call_id: "call-1".into(),
            tool_name: "bash".into(),
            trigger: ApprovalTrigger::approval_mode("Bash requires approval"),
            tool_call: ToolCallSnapshot {
                display: "bash".into(),
                args: json!({"command": "git status"}),
            },
        })
        .await;

    let resp = request(
        &test.app,
        Method::GET,
        "/agents/default/console/push-messages",
    )
    .await;

    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["messages"], json!([]));
    let pending = body["pending_approvals"]
        .as_array()
        .expect("pending approvals array");
    assert_eq!(pending.len(), 1, "body was {body:?}");
    assert_eq!(pending[0]["request_id"], approval.id);
    assert_eq!(pending[0]["tool_name"], "bash");
    assert!(pending[0]["created_at"].as_i64().is_some());
    assert!(pending[0]["expiry_seconds"].as_u64().is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn header_chat_interject_route_is_removed() {
    let test = build_app();
    let chat_id = "b00579f7-612b-4ace-bbc5-c4f0d75d9732";

    let resp = request_json(
        &test.app,
        Method::POST,
        "/console/chat/interject",
        json!({
            "chat_id": chat_id,
            "message": "queued interjection"
        }),
    )
    .await;

    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn header_chat_queue_route_is_removed() {
    let test = build_app();
    let chat_id = "b00579f7-612b-4ace-bbc5-c4f0d75d9732";

    let resp = request_json(
        &test.app,
        Method::POST,
        "/console/chat/queue",
        json!({
            "chat_id": chat_id,
            "message": "queued passive interjection"
        }),
    )
    .await;

    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_restart_routes_are_removed() {
    let test = build_app();

    for path in [
        "/console/chat/restart",
        "/agents/default/console/chat/restart",
    ] {
        let resp = request(&test.app, Method::POST, path).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND, "{path}");
    }
}
