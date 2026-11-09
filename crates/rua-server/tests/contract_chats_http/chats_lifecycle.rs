use axum::http::{Method, StatusCode};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use futures::StreamExt;
use rua_provider::config_loader::{DefaultModel, ProviderConfig, ProviderModelEntry};
use rua_security::{ApprovalRequestInput, ApprovalTrigger, ToolCallSnapshot};
use rua_server::services::chat_index::{self, ChatIndexEntry};
use rua_server::services::chat_metadata_store::{
    persist_acp_binding, update_authority_placement, AcpBindingMetadata,
};
use rua_server::services::console_chat::{StoredMessage, TURN_ABORTED_MARKER_TEXT};
use rua_server::services::conversation_authority::{
    ConversationActiveTurnStart, ConversationDriverKind, ConversationInputPayload,
    ConversationTurnBinding,
};
use rua_server::services::conversation_authority_placement::ConversationAuthorityPlacement;
use rua_session::{
    CheckpointModelIdentity, CheckpointSequenceRange, CheckpointTurnRange, NewConversationMessage,
    NewConversationTurn, RuntimeNote, RuntimeNotePayload, SummaryCheckpointDraft,
    TaskAttemptTerminalRuntimeNote, TurnLifecycle, WorkspaceSessionStore,
};
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use super::support::{
    build_app, create_agent, delete_agent, read_json, request, seed_materialized_chat,
};

fn ids(payload: &Value) -> Vec<String> {
    payload
        .as_array()
        .expect("list response array")
        .iter()
        .map(|row| row["id"].as_str().expect("chat id").to_string())
        .collect()
}

#[tokio::test]
async fn submission_already_inside_transition_gate_finishes_before_shutdown_closes_chat() {
    let app = build_app();
    app.state.wait_platform_initialization().await.unwrap();
    let authority = app.state.conversation_authority.clone();
    let address = rua_core::ChatAddress::agent("gate-race", "chat");
    let key = rua_server::streaming::StreamKey::new(
        "console",
        address.clone(),
        rua_server::streaming::Priority::Normal,
    );
    let entered = std::sync::Arc::new(tokio::sync::Notify::new());
    let release = std::sync::Arc::new(tokio::sync::Notify::new());
    let entered_callback = entered.clone();
    let release_callback = release.clone();
    let accepting = authority.clone();
    let streams = app.state.streams.clone();
    let submit_address = address.clone();
    let submit = tokio::spawn(async move {
        accepting.accept_submission(&streams, &key,
            ConversationActiveTurnStart { address: submit_address, driver: ConversationDriverKind::Rua,
                authority_device_id: "local".into(), authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(1,0,[0;8]), driver_detail: None },
            ConversationInputPayload { message:"first".into(), model_override:None,input:None,messages:None,extra:serde_json::Map::new(),attachments:Vec::new(),enqueued_at:std::time::Instant::now() },
            None, Some(Box::new(move |_, _| Box::pin(async move {
                entered_callback.notify_one(); release_callback.notified().await; Ok(())
            }))),
        ).await
    });
    entered.notified().await;
    authority.begin_draining("gate-race").await;
    let closing = authority.clone();
    let closing_address = address.clone();
    let shutdown = tokio::spawn(async move {
        closing.begin_chat_shutdown(&closing_address).await;
    });
    release.notify_one();
    assert!(
        submit.await.unwrap().is_ok(),
        "already accepted transaction completes atomically"
    );
    shutdown.await.unwrap();
    assert!(authority.active_turn(&address).await.is_some());
    authority.discard_unclaimed_inputs(&address).await;
    let queue = authority.queue_snapshot(&address).await;
    assert_eq!(
        queue.len(),
        1,
        "claimed first input remains until its terminal commit"
    );
}

#[tokio::test]
async fn shutdown_rejects_agent_and_project_submissions_before_starting_a_turn() {
    let app = build_app();
    let authority = &app.state.conversation_authority;
    for address in [
        rua_core::ChatAddress::agent("shutdown-test", "chat"),
        rua_core::ChatAddress::project("shutdown-project", "chat"),
    ] {
        authority.begin_chat_shutdown(&address).await;
        let key = rua_server::streaming::StreamKey::new(
            "console",
            address.clone(),
            rua_server::streaming::Priority::Normal,
        );
        let result = authority.accept_submission(
            app.state.streams.as_ref(),
            &key,
            ConversationActiveTurnStart {
                address: address.clone(),
                driver: ConversationDriverKind::Rua,
                authority_device_id: "device-local".to_string(),
                authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
                driver_detail: None,
            },
            ConversationInputPayload {
                message: "must not start".to_string(),
                model_override: None,
                input: None,
                messages: None,
                extra: serde_json::Map::new(),
                attachments: Vec::new(),
                enqueued_at: std::time::Instant::now(),
            },
            None,
            None,
        ).await;
        assert!(result
            .err()
            .is_some_and(|error| error.contains("shutting down")));
        assert!(authority.active_turn(&address).await.is_none());
        authority
            .resume_agent("shutdown-test", std::slice::from_ref(&address))
            .await;
        let resumed = authority.accept_submission(
            app.state.streams.as_ref(), &key,
            ConversationActiveTurnStart {
                address: address.clone(), driver: ConversationDriverKind::Rua,
                authority_device_id: "device-local".to_string(),
                authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
                driver_detail: None,
            },
            ConversationInputPayload {
                message: "resumed".to_string(), model_override: None, input: None, messages: None,
                extra: serde_json::Map::new(), attachments: Vec::new(), enqueued_at: std::time::Instant::now(),
            }, None, None,
        ).await;
        if let Err(error) = resumed {
            assert!(
                !error.contains("shutting down"),
                "explicit resume must reopen admission: {error}"
            );
        }
    }
}

async fn seed_chats_from_json(workspace: &Path, chats: &Value) {
    for chat in chats["chats"].as_array().expect("chats fixture array") {
        let id = chat["id"].as_str().expect("chat id");
        let created_at = chat_time(chat.get("created_at")).unwrap_or_else(Utc::now);
        let updated_at = chat_time(chat.get("updated_at")).unwrap_or(created_at);
        chat_index::upsert(
            workspace,
            ChatIndexEntry {
                id: id.to_string(),
                name: chat["name"].as_str().unwrap_or("New Chat").to_string(),
                user_id: chat["user_id"].as_str().unwrap_or("default").to_string(),
                channel: chat["channel"].as_str().unwrap_or("console").to_string(),
                created_at,
                updated_at,
                meta: chat.get("meta").cloned().unwrap_or_else(|| json!({})),
                status: chat["status"].as_str().unwrap_or("idle").to_string(),
                pinned: chat["pinned"].as_bool().unwrap_or(false),
                pin_order: chat["pin_order"].as_i64().unwrap_or(0),
                tag: serde_json::from_value(chat["tag"].clone()).unwrap_or_default(),
                archived: false,
            },
        )
        .await
        .expect("seed chat index");
    }
}

fn chat_time(value: Option<&Value>) -> Option<DateTime<Utc>> {
    value
        .and_then(Value::as_str)
        .and_then(|raw| DateTime::parse_from_rfc3339(raw).ok())
        .map(|dt| dt.with_timezone(&Utc))
}

async fn create_blocking_task(
    state: &rua_server::AppState,
    agent_id: &str,
    chat_id: &str,
    instruction: &str,
) -> (rua_task::TaskOwner, String) {
    let runner = Arc::new(BranchBlockingUnifiedTaskRunner {
        started: tokio::sync::Notify::new(),
    });
    state.task_runtime.register_runner(runner.clone());
    let owner = rua_task::TaskOwner::new(agent_id, chat_id);
    let task_id = state
        .task_runtime
        .create(
            rua_task::TaskSpec {
                owner: owner.clone(),
                instruction: instruction.to_string(),
                runner: rua_task::TaskRunnerSpec::Tool {
                    tool_name: "Fixture_Block".to_string(),
                    arguments: json!({}),
                },
                context_refs: Vec::new(),
                depends_on: Vec::new(),
                origin: Default::default(),
            },
            rua_task::TaskAttemptAuthorization::new(agent_id).unwrap(),
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), runner.started.notified())
        .await
        .unwrap();
    (owner, task_id)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn existing_chat_remains_readable_after_sync_is_disabled() {
    let app = build_app();
    let agent_id = "chat_after_sync_disabled";
    create_agent(&app.app, agent_id, "Chat after sync disabled").await;
    let created = seed_materialized_chat(
        &app,
        "/api/chats",
        Some(agent_id),
        "chat-after-sync-disabled",
    )
    .await;
    let chat_id = created["id"].as_str().expect("created chat id");

    let enable = request(
        &app.app,
        Method::POST,
        "/api/sync/enable",
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(enable.status(), StatusCode::OK);
    let disable = request(
        &app.app,
        Method::POST,
        "/api/sync/disable",
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(disable.status(), StatusCode::OK);

    let history = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{agent_id}/chats/{chat_id}"),
        None,
        None,
    )
    .await;
    assert_eq!(
        history.status(),
        StatusCode::OK,
        "existing Chat history must remain readable after disabling sync: {}",
        read_json(history).await
    );

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test]
async fn existing_chat_history_does_not_require_local_device_identity() {
    let app = build_app();
    let agent_id = "chat_without_device_identity";
    create_agent(&app.app, agent_id, "Chat without Device identity").await;
    let created = seed_materialized_chat(
        &app,
        "/api/chats",
        Some(agent_id),
        "Local history remains readable",
    )
    .await;
    let chat_id = created["id"].as_str().expect("created chat id");

    *app.state
        .sync_device
        .write()
        .expect("sync device write lock") = None;

    let history = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{agent_id}/chats/{chat_id}"),
        None,
        None,
    )
    .await;
    assert_eq!(
        history.status(),
        StatusCode::OK,
        "stable local history must not depend on runtime Device identity"
    );
    let history = read_json(history).await;
    assert_eq!(history["title"], "Local history remains readable");
    assert!(history.get("control").is_none());
    assert_eq!(
        history["meta"]["authority_placement"],
        created["meta"]["authority_placement"]
    );

    delete_agent(&app.app, agent_id).await;
}

async fn store_default_model(app: &super::support::TestApp, provider_id: &str, model: &str) {
    app.state
        .provider_manager
        .store_default_model(DefaultModel {
            provider_id: provider_id.to_string(),
            model: model.to_string(),
            source: None,
            extra: serde_json::Map::new(),
        })
        .await;
}

fn stored_message(id: &str, role: &str, text: &str, timestamp: &str, name: &str) -> StoredMessage {
    StoredMessage {
        id: id.to_string(),
        role: role.to_string(),
        content: vec![json!({"type": "text", "text": text})],
        name: name.to_string(),
        timestamp: timestamp.to_string(),
        device_id: String::new(),
        metadata: serde_json::Value::Null,
        extra: serde_json::Map::new(),
    }
}

fn branch_runtime_note(id: &str, output: &str, created_at: &str) -> RuntimeNote {
    RuntimeNote {
        runtime_note_id: id.to_string(),
        created_at: created_at.to_string(),
        payload: RuntimeNotePayload::TaskAttemptTerminal(TaskAttemptTerminalRuntimeNote {
            task_id: format!("task-{id}"),
            attempt_no: 1,
            terminal_status: "completed".to_string(),
            output: output.to_string(),
            runner_name: "Fixture_RuntimeNote".to_string(),
            tool_call_id: None,
            full_result_ref: None,
            artifacts: json!([]),
            device: json!({}),
        }),
    }
}

fn pending(message: &str) -> ConversationInputPayload {
    ConversationInputPayload {
        message: message.to_string(),
        model_override: None,
        input: None,
        messages: None,
        extra: serde_json::Map::new(),
        attachments: Vec::new(),
        enqueued_at: std::time::Instant::now(),
    }
}

fn provider_model(id: &str) -> ProviderModelEntry {
    ProviderModelEntry {
        id: id.to_string(),
        name: id.to_string(),
        supports_multimodal: None,
        supports_image: None,
        supports_video: None,
        probe_source: None,
        context_window: None,
        max_output_tokens: None,
        generate_kwargs: serde_json::Map::new(),
        thinking_enabled: None,
        thinking_budget: None,
        reasoning_effort: None,
        thinking_param_style: None,
        reasoning_effort_options: Vec::new(),
        thinking_budget_range: None,
        relay_reasoning: None,
        extra: serde_json::Map::new(),
    }
}

async fn seed_reasoning_provider(app: &super::support::TestApp) {
    let mut qwen = provider_model("qwen3.7-plus");
    qwen.thinking_enabled = Some(true);
    qwen.thinking_param_style = Some("budget".into());
    qwen.thinking_budget = Some(8192);
    qwen.thinking_budget_range = Some([1, 81920]);

    let mut glm = provider_model("glm-5.2");
    glm.thinking_enabled = Some(true);
    glm.thinking_param_style = Some("effort".into());
    glm.reasoning_effort = Some("medium".into());
    glm.reasoning_effort_options = vec!["low".into(), "medium".into(), "high".into()];

    app.state
        .provider_cells
        .cell("dashscope")
        .write(ProviderConfig {
            id: "dashscope".into(),
            name: "DashScope".into(),
            base_url: "https://dashscope.aliyuncs.com/compatible-mode/v1".into(),
            api_key: "sk-test".into(),
            api_keys: Vec::new(),
            api_key_prefixes: Vec::new(),
            headers: serde_json::Map::new(),
            custom_headers: serde_json::Map::new(),
            chat_model: Some("DashScopeChatModel".into()),
            models: vec![qwen, glm],
            extra_models: Vec::new(),
            timeout: None,
            generate_kwargs: serde_json::Map::new(),
            support_model_discovery: false,
            support_connection_check: false,
            is_local: false,
            api_key_prefix: String::new(),
            is_custom: false,
            is_plugin: false,
            freeze_url: false,
            require_api_key: false,
            provider_group: String::new(),
            provider_group_name: String::new(),
            provider_variant: String::new(),
            base_url_options: Vec::new(),
            thinking_param_style: Some("budget".into()),
            reasoning_effort_options: Vec::new(),
            thinking_budget_range: Some([1, 81920]),
            extra: serde_json::Map::new(),
        })
        .await
        .expect("seed provider config");
}

async fn append_evergreen_turn(
    app: &super::support::TestApp,
    agent_id: &str,
    chat_id: &str,
    messages: Vec<StoredMessage>,
) {
    append_evergreen_turn_with_id(
        app,
        agent_id,
        chat_id,
        &format!("{chat_id}-turn-1"),
        messages,
    )
    .await;
}

async fn append_evergreen_turn_with_id(
    app: &super::support::TestApp,
    agent_id: &str,
    chat_id: &str,
    turn_id: &str,
    messages: Vec<StoredMessage>,
) {
    let workspace = app
        .state
        .registry
        .get_or_create(agent_id)
        .await
        .expect("workspace runtime")
        .workspace_dir();
    WorkspaceSessionStore::new(&workspace)
        .append_conversation_turn(
            chat_id,
            NewConversationTurn {
                turn_id: turn_id.to_string(),
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
        .expect("append evergreen turn");
}

async fn append_in_progress_evergreen_turn_with_id(
    app: &super::support::TestApp,
    agent_id: &str,
    chat_id: &str,
    turn_id: &str,
    message: StoredMessage,
) {
    let workspace = app
        .state
        .registry
        .get_or_create(agent_id)
        .await
        .expect("workspace runtime")
        .workspace_dir();
    WorkspaceSessionStore::new(&workspace)
        .append_conversation_turn(
            chat_id,
            NewConversationTurn {
                turn_id: turn_id.to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-06-24T00:00:00Z".to_string(),
                    assistant_started_at: Some("2026-06-24T00:00:01Z".to_string()),
                    assistant_completed_at: None,
                    status: "in_progress".to_string(),
                },
                selected_variant_id: None,
                messages: vec![NewConversationMessage {
                    message_id: message.id,
                    role: message.role,
                    created_at: message.timestamp,
                    status: "completed".to_string(),
                    content: Value::Array(message.content),
                    provider: None,
                    tool: None,
                    metadata: serde_json::Map::new(),
                }],
            },
        )
        .await
        .expect("append in-progress evergreen turn");
}

async fn append_test_turn(
    app: &super::support::TestApp,
    agent_id: &str,
    chat_id: &str,
    turn_id: &str,
    user_text: &str,
    assistant_text: &str,
    acp_runner: Option<&str>,
) {
    let workspace = app
        .state
        .registry
        .get_or_create(agent_id)
        .await
        .expect("workspace runtime")
        .workspace_dir();
    let metadata = acp_runner
        .map(|runner| {
            serde_json::Map::from_iter([(
                "origin".to_string(),
                json!({"kind": "acp", "runner": runner}),
            )])
        })
        .unwrap_or_default();
    let user_message_id = format!("{turn_id}-user");
    let assistant_message_id = format!("{turn_id}-assistant");
    WorkspaceSessionStore::new(&workspace)
        .append_conversation_turn(
            chat_id,
            NewConversationTurn {
                turn_id: turn_id.to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-06-19T00:00:00Z".to_string(),
                    assistant_started_at: Some("2026-06-19T00:00:01Z".to_string()),
                    assistant_completed_at: Some("2026-06-19T00:00:02Z".to_string()),
                    status: "completed".to_string(),
                },
                selected_variant_id: Some(assistant_message_id.clone()),
                messages: vec![
                    NewConversationMessage {
                        message_id: user_message_id,
                        role: "user".to_string(),
                        created_at: "2026-06-19T00:00:00Z".to_string(),
                        status: "completed".to_string(),
                        content: json!([{"type": "text", "text": user_text}]),
                        provider: None,
                        tool: None,
                        metadata: metadata.clone(),
                    },
                    NewConversationMessage {
                        message_id: assistant_message_id,
                        role: "assistant".to_string(),
                        created_at: "2026-06-19T00:00:01Z".to_string(),
                        status: "completed".to_string(),
                        content: json!([{"type": "text", "text": assistant_text}]),
                        provider: None,
                        tool: None,
                        metadata,
                    },
                ],
            },
        )
        .await
        .expect("append test turn");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn user_message_branch_points_keep_content_before_that_input() {
    let app = build_app();
    let agent_id = "integ_user_message_branch_point_01";
    create_agent(&app.app, agent_id, "User message branch point agent").await;
    let created = seed_materialized_chat(
        &app,
        "/api/chats",
        Some(agent_id),
        "User message branch points",
    )
    .await;
    let chat_id = created["id"].as_str().expect("chat id").to_string();
    append_test_turn(
        &app,
        agent_id,
        &chat_id,
        "turn-1",
        "first input",
        "first answer",
        None,
    )
    .await;
    append_test_turn(
        &app,
        agent_id,
        &chat_id,
        "turn-2",
        "second input",
        "second answer",
        None,
    )
    .await;

    let old_branch_body = request(
        &app.app,
        Method::POST,
        &format!("/api/chats/{chat_id}/branch"),
        Some(json!({
            "turn_id": "turn-1",
            "selected_variant_id": "turn-1-assistant"
        })),
        Some(agent_id),
    )
    .await;
    assert_eq!(old_branch_body.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let branched = request(
        &app.app,
        Method::POST,
        &format!("/api/chats/{chat_id}/branch"),
        Some(json!({"before_turn_id": "turn-2"})),
        Some(agent_id),
    )
    .await;
    assert_eq!(branched.status(), StatusCode::OK);
    let branch_id = read_json(branched).await["chat"]["id"]
        .as_str()
        .expect("branch id")
        .to_string();
    let branch_history = read_json(
        request(
            &app.app,
            Method::GET,
            &format!("/api/chats/{branch_id}"),
            None,
            Some(agent_id),
        )
        .await,
    )
    .await;
    assert_eq!(branch_history["messages"].as_array().unwrap().len(), 2);
    assert_eq!(
        branch_history["messages"][0]["content"][0]["text"],
        "first input"
    );

    let empty_branch = request(
        &app.app,
        Method::POST,
        &format!("/api/chats/{chat_id}/branch"),
        Some(json!({"before_turn_id": "turn-1"})),
        Some(agent_id),
    )
    .await;
    assert_eq!(empty_branch.status(), StatusCode::OK);
    let empty_branch_id = read_json(empty_branch).await["chat"]["id"]
        .as_str()
        .expect("empty branch id")
        .to_string();
    let empty_branch_history = read_json(
        request(
            &app.app,
            Method::GET,
            &format!("/api/chats/{empty_branch_id}"),
            None,
            Some(agent_id),
        )
        .await,
    )
    .await;
    assert!(empty_branch_history["messages"]
        .as_array()
        .unwrap()
        .is_empty());

    let rolled_back = request(
        &app.app,
        Method::POST,
        &format!("/api/chats/{chat_id}/rollback"),
        Some(json!({"before_turn_id": "turn-1"})),
        Some(agent_id),
    )
    .await;
    assert_eq!(rolled_back.status(), StatusCode::OK);
    let rolled_back_history = read_json(
        request(
            &app.app,
            Method::GET,
            &format!("/api/chats/{chat_id}"),
            None,
            Some(agent_id),
        )
        .await,
    )
    .await;
    assert!(rolled_back_history["messages"]
        .as_array()
        .unwrap()
        .is_empty());

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn root_chats_crud_and_agent_scoped_list_match_qwenpaw_contract() {
    let app = build_app();
    let agent_id = "integ_chats_01";
    create_agent(&app.app, agent_id, "Chats smoke agent").await;

    let created =
        seed_materialized_chat(&app, "/api/chats", Some(agent_id), "Integration smoke chat").await;
    let chat_id = created["id"].as_str().expect("chat id").to_string();
    assert_eq!(created["name"], "Integration smoke chat");
    assert!(created.get("session_id").is_none());
    assert_eq!(created["user_id"], "user-Integration smoke chat");
    assert_eq!(created["channel"], "console");

    let workspace = app
        .state
        .registry
        .get_or_create(agent_id)
        .await
        .expect("workspace runtime")
        .workspace_dir();
    let session_store = WorkspaceSessionStore::new(&workspace);
    let header = session_store
        .load_chat_header(&chat_id)
        .await
        .expect("load evergreen chat header")
        .expect("chat create should persist directory-backed session header");
    assert_eq!(header.chat_id, chat_id);
    assert_eq!(header.agent_id, agent_id);
    assert_eq!(header.metadata.title, "Integration smoke chat");
    assert_eq!(header.metadata.preview, None);

    let scoped_list = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{agent_id}/chats"),
        None,
        None,
    )
    .await;
    assert_eq!(scoped_list.status(), StatusCode::OK);
    assert!(ids(&read_json(scoped_list).await).contains(&chat_id));

    let history = request(
        &app.app,
        Method::GET,
        &format!("/api/chats/{chat_id}"),
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(history.status(), StatusCode::OK);
    let history_body = read_json(history).await;
    assert!(history_body["messages"].is_array());
    assert!(history_body.get("status").is_none());
    assert!(history_body.get("active_turn").is_none());
    assert!(history_body.get("control").is_none());
    assert!(history_body.get("run").is_none());

    let renamed = request(
        &app.app,
        Method::PUT,
        &format!("/api/chats/{chat_id}"),
        Some(json!({
            "name": "Integration smoke chat renamed",
            "collaboration_mode": "plan"
        })),
        Some(agent_id),
    )
    .await;
    assert_eq!(renamed.status(), StatusCode::OK);
    let renamed = read_json(renamed).await;
    assert_eq!(renamed["name"], "Integration smoke chat renamed");
    assert_eq!(renamed["collaboration_mode"], "plan");
    let header = session_store
        .load_chat_header(&chat_id)
        .await
        .expect("load renamed evergreen chat header")
        .expect("renamed chat header exists");
    assert_eq!(
        header.metadata.title, "Integration smoke chat renamed",
        "PUT /chats/{{id}} should keep session metadata title in sync"
    );
    assert_eq!(
        header
            .extra
            .get(rua_core::CHAT_COLLABORATION_MODE_META_KEY)
            .and_then(serde_json::Value::as_str),
        Some("plan")
    );

    let deleted = request(
        &app.app,
        Method::DELETE,
        &format!("/api/chats/{chat_id}"),
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(deleted.status(), StatusCode::OK);
    assert_eq!(read_json(deleted).await["deleted"], true);

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test]
async fn chat_model_slot_preserves_thinking_strength() {
    let app = build_app();
    seed_reasoning_provider(&app).await;
    let agent_id = "chat_model_reasoning_fields";
    create_agent(&app.app, agent_id, "Chat model reasoning fields").await;
    let created =
        seed_materialized_chat(&app, "/api/chats", Some(agent_id), "Reasoning Slot").await;
    let chat_id = created["id"].as_str().expect("chat id");

    let updated = request(
        &app.app,
        Method::PUT,
        &format!("/api/chats/{chat_id}"),
        Some(json!({
            "model": {
                "provider_id": "dashscope",
                "model": "qwen3.7-plus",
                "thinking_strength": "high",
                "thinking_param_style": "budget"
            }
        })),
        Some(agent_id),
    )
    .await;

    assert_eq!(updated.status(), StatusCode::OK);
    let body = read_json(updated).await;
    assert_eq!(body["model"]["provider_id"], "dashscope");
    assert_eq!(body["model"]["model"], "qwen3.7-plus");
    assert_eq!(body["model"]["thinking_strength"], "high");
    assert!(body["model"]["thinking_enabled"].is_null());
    assert!(body["model"]["thinking_budget"].is_null());
    assert!(body["model"]["reasoning_effort"].is_null());
    assert_eq!(body["model"]["thinking_param_style"], "budget");

    let runtime = app
        .state
        .registry
        .get_or_create(agent_id)
        .await
        .expect("agent runtime");
    let store = WorkspaceSessionStore::new(runtime.workspace_dir());
    let header = store
        .load_chat_header(chat_id)
        .await
        .expect("load chat header")
        .expect("chat header exists");
    assert_eq!(
        header.extra.get("model").cloned().map(|value| {
            let model: rua_server::endpoints::dto::ChatModelDto = serde_json::from_value(value).expect("persisted model slot");
            serde_json::to_value(model).unwrap()
        }).as_ref(),
        Some(&body["model"]),
        "chat model slot must be persisted in the evergreen session header, not only the list index"
    );

    let history = request(
        &app.app,
        Method::GET,
        &format!("/api/chats/{chat_id}"),
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(history.status(), StatusCode::OK);
    let history_body = read_json(history).await;
    assert_eq!(history_body["model"], body["model"]);

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test]
async fn chat_options_are_snapshotted_at_creation_and_owned_by_each_chat() {
    let app = build_app();
    let agent_id = "chat_options_authority";
    create_agent(&app.app, agent_id, "Chat options authority").await;
    let original_runtime = app.state.registry.get_or_create(agent_id).await.unwrap();
    let original_memory = original_runtime.memory_manager().await.unwrap();
    let original_react = original_runtime.react_runtime().await.unwrap();

    let defaults = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{agent_id}/running-config"),
        None,
        None,
    )
    .await;
    assert_eq!(defaults.status(), StatusCode::OK);
    let mut defaults = read_json(defaults).await;
    defaults["max_iters"] = json!(19);
    defaults["llm_max_concurrent"] = json!(2);
    defaults["llm_max_retries"] = json!(4);
    defaults["tool_result_compact"]["projection_truncation_batch_size"] = json!(10);

    let updated_defaults = request(
        &app.app,
        Method::PUT,
        &format!("/api/agents/{agent_id}/running-config"),
        Some(defaults),
        None,
    )
    .await;
    assert_eq!(updated_defaults.status(), StatusCode::OK);
    assert_eq!(read_json(updated_defaults).await["max_iters"], 19);

    let chat_a = seed_materialized_chat(&app, "/api/chats", Some(agent_id), "Options Chat A").await;
    let chat_b = seed_materialized_chat(&app, "/api/chats", Some(agent_id), "Options Chat B").await;
    let chat_a_id = chat_a["id"].as_str().expect("Chat A id");
    let chat_b_id = chat_b["id"].as_str().expect("Chat B id");

    let current_defaults = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{agent_id}/running-config"),
        None,
        None,
    )
    .await;
    let mut current_defaults = read_json(current_defaults).await;
    current_defaults["max_iters"] = json!(23);
    current_defaults["llm_max_concurrent"] = json!(9);
    current_defaults["llm_max_retries"] = json!(8);
    current_defaults["tool_result_compact"]["projection_truncation_batch_size"] = json!(20);
    let changed_defaults = request(
        &app.app,
        Method::PUT,
        &format!("/api/agents/{agent_id}/running-config"),
        Some(current_defaults),
        None,
    )
    .await;
    assert_eq!(changed_defaults.status(), StatusCode::OK);
    assert_eq!(read_json(changed_defaults).await["max_iters"], 23);

    let chat_c = seed_materialized_chat(&app, "/api/chats", Some(agent_id), "Options Chat C").await;
    let chat_c_id = chat_c["id"].as_str().expect("Chat C id");

    let chat_a_config = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{agent_id}/chats/{chat_a_id}/running-config"),
        None,
        None,
    )
    .await;
    assert_eq!(chat_a_config.status(), StatusCode::OK);
    let mut chat_a_config = read_json(chat_a_config).await;
    assert_eq!(chat_a_config["max_iters"], 19);
    assert_eq!(chat_a_config["llm_max_concurrent"], 2);
    assert_eq!(chat_a_config["llm_max_retries"], 4);
    assert_eq!(
        chat_a_config["tool_result_compact"]["projection_truncation_batch_size"],
        10
    );
    chat_a_config["max_iters"] = json!(7);
    chat_a_config["tool_result_compact"]["projection_truncation_batch_size"] = json!(5);
    chat_a_config["context_compact"]["active_work_plan_injection_enabled"] = json!(true);

    let updated = request(
        &app.app,
        Method::PUT,
        &format!("/api/agents/{agent_id}/chats/{chat_a_id}/running-config"),
        Some(chat_a_config),
        None,
    )
    .await;
    assert_eq!(updated.status(), StatusCode::OK);
    let updated = read_json(updated).await;
    assert_eq!(updated["max_iters"], 7);
    assert_eq!(
        updated["tool_result_compact"]["projection_truncation_batch_size"],
        5
    );
    assert_eq!(
        updated["context_compact"]["active_work_plan_injection_enabled"],
        true
    );

    let other_chat = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{agent_id}/chats/{chat_b_id}/running-config"),
        None,
        None,
    )
    .await;
    assert_eq!(other_chat.status(), StatusCode::OK);
    let other_chat = read_json(other_chat).await;
    assert_eq!(other_chat["max_iters"], 19);
    assert_eq!(
        other_chat["tool_result_compact"]["projection_truncation_batch_size"],
        10
    );

    let new_chat = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{agent_id}/chats/{chat_c_id}/running-config"),
        None,
        None,
    )
    .await;
    assert_eq!(new_chat.status(), StatusCode::OK);
    let new_chat = read_json(new_chat).await;
    assert_eq!(new_chat["max_iters"], 23);
    assert_eq!(new_chat["llm_max_concurrent"], 9);
    assert_eq!(new_chat["llm_max_retries"], 8);
    assert_eq!(
        new_chat["tool_result_compact"]["projection_truncation_batch_size"],
        20
    );

    let runtime = app
        .state
        .registry
        .get_or_create(agent_id)
        .await
        .expect("agent runtime");
    assert!(WorkspaceSessionStore::new(runtime.workspace_dir())
        .configuration_head_path(chat_a_id)
        .is_file());
    assert!(std::sync::Arc::ptr_eq(&original_runtime, &runtime));
    assert!(std::sync::Arc::ptr_eq(
        &original_memory,
        &runtime.memory_manager().await.unwrap()
    ));
    assert!(std::sync::Arc::ptr_eq(
        &original_react,
        &runtime.react_runtime().await.unwrap()
    ));
    assert_eq!(
        WorkspaceSessionStore::new(runtime.workspace_dir())
            .load_chat_running_config(chat_a_id)
            .await
            .unwrap()
            .unwrap()
            .tool_result_compact
            .projection_truncation_batch_size,
        5
    );

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test]
async fn seed_materialized_chat_without_model_uses_default_model() {
    let app = build_app();
    store_default_model(&app, "dashscope", "qwen-plus").await;
    let agent_id = "chat_create_default_model";
    create_agent(&app.app, agent_id, "Chat create active model").await;

    let created =
        seed_materialized_chat(&app, "/api/chats", Some(agent_id), "Active Model Chat").await;
    let chat_id = created["id"].as_str().expect("chat id");

    assert_eq!(created["model"]["provider_id"], "dashscope");
    assert_eq!(created["model"]["model"], "qwen-plus");
    assert_eq!(created["model"]["thinking_strength"], "auto");

    let runtime = app
        .state
        .registry
        .get_or_create(agent_id)
        .await
        .expect("agent runtime");
    let store = WorkspaceSessionStore::new(runtime.workspace_dir());
    let header = store
        .load_chat_header(chat_id)
        .await
        .expect("load chat header")
        .expect("chat header exists");
    assert_eq!(
        header.extra.get("model"),
        Some(&created["model"]),
        "default chat model must be persisted in the evergreen session header"
    );

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test]
async fn unmaterialized_chat_history_uses_default_model() {
    let app = build_app();
    store_default_model(&app, "dashscope", "qwen-plus").await;
    let agent_id = "chat_draft_default_model";
    create_agent(&app.app, agent_id, "Chat draft active model").await;
    let chat_id = uuid::Uuid::new_v4();

    let history = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{agent_id}/chats/{chat_id}"),
        None,
        None,
    )
    .await;

    assert_eq!(history.status(), StatusCode::OK);
    let body = read_json(history).await;
    assert_eq!(body["model"]["provider_id"], "dashscope");
    assert_eq!(body["model"]["model"], "qwen-plus");
    assert_eq!(body["model"]["thinking_strength"], "auto");
    assert!(body["messages"].as_array().is_some_and(Vec::is_empty));
    let local_device_id = app
        .state
        .sync_device
        .read()
        .expect("sync device lock")
        .as_ref()
        .map(|device| hex::encode(device.device_id_short()))
        .expect("test Device identity");
    assert_eq!(
        body["meta"]["authority_placement"]["device_id"],
        local_device_id
    );
    assert!(body["meta"]["authority_placement"]["authority_epoch"]
        .as_str()
        .is_some_and(|epoch| !epoch.is_empty()));
    assert!(body.get("status").is_none());
    assert!(body.get("active_turn").is_none());
    assert!(body.get("control").is_none());

    let runtime = app
        .state
        .registry
        .get_or_create(agent_id)
        .await
        .expect("runtime");
    assert!(
        rua_server::services::chat_index::get(&runtime.workspace_dir(), &chat_id.to_string())
            .await
            .expect("read index")
            .is_none(),
        "reading a draft must not materialize it"
    );

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test]
async fn materializing_draft_on_selected_device_preserves_selected_model() {
    let app = build_app();
    seed_reasoning_provider(&app).await;
    store_default_model(&app, "dashscope", "glm-5.2").await;
    let agent_id = "chat_draft_local_authority";
    create_agent(&app.app, agent_id, "Chat draft local authority").await;
    let chat_id = uuid::Uuid::new_v4();
    let local_device_id = app
        .state
        .sync_device
        .read()
        .expect("sync device lock")
        .as_ref()
        .map(|device| hex::encode(device.device_id_short()))
        .expect("test Device identity");

    let migration = request(
        &app.app,
        Method::PUT,
        &format!("/api/agents/{agent_id}/chats/{chat_id}/authority-placement"),
        Some(json!({"device_id":local_device_id})),
        None,
    )
    .await;
    assert_eq!(migration.status(), StatusCode::NOT_FOUND);
    let outcome = app
        .state
        .conversation_input_intake
        .submit_delivery(
            agent_id,
            rua_channel::core::console::ConsoleChatRequest {
                message: "first selected input".into(),
                chat_id: Some(chat_id.to_string()),
                model: None,
                is_reconnect: false,
                attachments: vec![],
                input: None,
                messages: None,
                channel_kind: None,
                extra: serde_json::Map::from_iter([
                    ("target_device_id".into(), json!(local_device_id)),
                    (
                        "chat_model_slot".into(),
                        json!({"provider_id":"dashscope","model":"qwen3.7-plus"}),
                    ),
                ]),
            },
        )
        .await;
    assert!(matches!(outcome, rua_server::services::conversation_input_intake::ConversationInputDeliveryOutcome::Delivered { .. }), "{outcome:?}");
    let runtime = app.state.registry.get(agent_id).await.unwrap();
    let header = WorkspaceSessionStore::new(runtime.workspace_dir())
        .load_chat_header(&chat_id.to_string())
        .await
        .unwrap()
        .unwrap();
    let body = json!({"meta":header.extra});
    assert_eq!(
        body["meta"]["authority_placement"]["device_id"],
        local_device_id
    );
    assert_eq!(
        body["meta"]["authority_placement"]["authority_epoch"]
            .as_str()
            .map(str::len),
        Some(40)
    );
    let history = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{agent_id}/chats/{chat_id}"),
        None,
        None,
    )
    .await;
    assert_eq!(history.status(), StatusCode::OK);
    let history_body = read_json(history).await;
    assert_eq!(history_body["model"]["provider_id"], "dashscope");
    assert_eq!(history_body["model"]["model"], "qwen3.7-plus");

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test]
async fn first_input_without_model_uses_default_after_missing_put_is_rejected() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("rua_server=debug,rua_workspace=debug,rua_agent=debug,rua_channel=debug")
        .with_test_writer()
        .try_init();
    let app = build_app();
    store_default_model(&app, "dashscope", "qwen-plus").await;
    let agent_id = "chat_lazy_create_default_model";
    let chat_id = "00000000-0000-4000-8000-000000000117";
    create_agent(&app.app, agent_id, "Chat lazy create active model").await;

    let updated = request(
        &app.app,
        Method::PUT,
        &format!("/api/chats/{chat_id}"),
        Some(json!({
            "name": "Draft title"
        })),
        Some(agent_id),
    )
    .await;

    assert_eq!(updated.status(), StatusCode::NOT_FOUND);
    let outcome = app
        .state
        .conversation_input_intake
        .submit_delivery(
            agent_id,
            rua_channel::core::console::ConsoleChatRequest {
                message: "first input".into(),
                chat_id: Some(chat_id.into()),
                model: None,
                is_reconnect: false,
                attachments: vec![],
                input: None,
                messages: None,
                extra: Default::default(),
                channel_kind: None,
            },
        )
        .await;
    assert!(matches!(outcome, rua_server::services::conversation_input_intake::ConversationInputDeliveryOutcome::Delivered { .. }), "{outcome:?}");

    let history = request(
        &app.app,
        Method::GET,
        &format!("/api/chats/{chat_id}"),
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(history.status(), StatusCode::OK);
    let history_body = read_json(history).await;
    assert_eq!(history_body["model"]["provider_id"], "dashscope");
    assert_eq!(history_body["model"]["model"], "qwen-plus");
    assert_eq!(history_body["model"]["thinking_strength"], "auto");

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test]
async fn chat_model_slot_explicit_disable_maps_to_off_strength() {
    let app = build_app();
    seed_reasoning_provider(&app).await;
    let agent_id = "chat_model_reasoning_disabled";
    create_agent(&app.app, agent_id, "Chat model reasoning disabled").await;
    let created =
        seed_materialized_chat(&app, "/api/chats", Some(agent_id), "Reasoning Disabled").await;
    let chat_id = created["id"].as_str().expect("chat id");

    let updated = request(
        &app.app,
        Method::PUT,
        &format!("/api/chats/{chat_id}"),
        Some(json!({
            "model": {
                "provider_id": "dashscope",
                "model": "qwen3.7-plus",
                "thinking_enabled": false,
                "thinking_budget": null,
                "reasoning_effort": null
            }
        })),
        Some(agent_id),
    )
    .await;

    assert_eq!(updated.status(), StatusCode::OK);
    let body = read_json(updated).await;
    assert_eq!(body["model"]["thinking_strength"], "off");
    assert!(body["model"]["thinking_enabled"].is_null());
    assert!(body["model"]["thinking_budget"].is_null());
    assert!(body["model"]["reasoning_effort"].is_null());
    assert_eq!(body["model"]["thinking_param_style"], "budget");

    let history = request(
        &app.app,
        Method::GET,
        &format!("/api/chats/{chat_id}"),
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(history.status(), StatusCode::OK);
    let history_body = read_json(history).await;
    assert_eq!(history_body["model"], body["model"]);

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test]
async fn chat_model_slot_accepts_gemini_budget_range_starting_at_zero() {
    let app = build_app();
    let template = app
        .state
        .provider_manager
        .list_templates()
        .iter()
        .find(|template| template.id == "google")
        .unwrap();
    let mut config = template.to_config();
    config.models = vec![template.preset_model_entry("gemini-2.5-flash")];
    app.state
        .provider_cells
        .cell("google")
        .write(config)
        .await
        .unwrap();
    let agent_id = "chat_gemini_budget_range";
    create_agent(&app.app, agent_id, "Gemini budget range").await;
    let path = format!("/api/agents/{agent_id}/chats");
    let created = seed_materialized_chat(&app, &path, None, "Gemini budget").await;
    let chat_id = created["id"].as_str().unwrap();
    let updated = request(
        &app.app,
        Method::PUT,
        &format!("{path}/{chat_id}"),
        Some(json!({
            "model": {"provider_id":"google","model":"gemini-2.5-flash","thinking_strength":"high"}
        })),
        None,
    )
    .await;
    assert_eq!(updated.status(), StatusCode::OK);
    let body = read_json(updated).await;
    assert_eq!(body["model"]["thinking_strength"], "high");
    assert_eq!(body["model"]["thinking_budget_range"], json!([0, 24576]));
    assert_eq!(body["model"]["thinking_param_style"], "budget");
    delete_agent(&app.app, agent_id).await;
}

#[tokio::test]
async fn chat_model_switch_resets_thinking_strength_to_auto() {
    let app = build_app();
    seed_reasoning_provider(&app).await;
    let agent_id = "chat_model_reasoning_reset";
    create_agent(&app.app, agent_id, "Chat model reasoning reset").await;
    let created =
        seed_materialized_chat(&app, "/api/chats", Some(agent_id), "Reasoning Reset").await;
    let chat_id = created["id"].as_str().expect("chat id");

    let qwen = request(
        &app.app,
        Method::PUT,
        &format!("/api/chats/{chat_id}"),
        Some(json!({
            "model": {
                "provider_id": "dashscope",
                "model": "qwen3.7-plus"
            }
        })),
        Some(agent_id),
    )
    .await;
    assert_eq!(qwen.status(), StatusCode::OK);
    let qwen_body = read_json(qwen).await;
    assert_eq!(qwen_body["model"]["thinking_strength"], "auto");
    assert!(qwen_body["model"]["thinking_enabled"].is_null());
    assert!(qwen_body["model"]["thinking_budget"].is_null());
    assert!(qwen_body["model"]["reasoning_effort"].is_null());
    assert_eq!(qwen_body["model"]["thinking_param_style"], "budget");

    let glm = request(
        &app.app,
        Method::PUT,
        &format!("/api/chats/{chat_id}"),
        Some(json!({
            "model": {
                "provider_id": "dashscope",
                "model": "glm-5.2"
            }
        })),
        Some(agent_id),
    )
    .await;
    assert_eq!(glm.status(), StatusCode::OK);
    let glm_body = read_json(glm).await;
    assert_eq!(glm_body["model"]["provider_id"], "dashscope");
    assert_eq!(glm_body["model"]["model"], "glm-5.2");
    assert_eq!(glm_body["model"]["thinking_strength"], "auto");
    assert!(glm_body["model"]["thinking_enabled"].is_null());
    assert!(glm_body["model"]["thinking_budget"].is_null());
    assert!(glm_body["model"]["reasoning_effort"].is_null());
    assert_eq!(glm_body["model"]["thinking_param_style"], "effort");

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test]
async fn chat_history_excludes_authority_runtime_state() {
    let app = build_app();
    let agent_id = "integ_chats_active_turn";
    create_agent(&app.app, agent_id, "Active turn").await;
    let chat = seed_materialized_chat(
        &app,
        &format!("/api/agents/{agent_id}/chats"),
        None,
        "Active Turn Chat",
    )
    .await;
    let chat_id = chat["id"].as_str().expect("chat id").to_string();

    let active_turn = app
        .state
        .conversation_authority
        .start_active_turn(ConversationActiveTurnStart {
            address: rua_core::ChatAddress::agent(agent_id.to_string(), chat_id.clone()),
            driver: ConversationDriverKind::Rua,
            authority_device_id: "device-local".to_string(),
            authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
            driver_detail: None,
        })
        .await;
    app.state
        .conversation_authority
        .bind_turn(ConversationTurnBinding {
            address: rua_core::ChatAddress::agent(agent_id.to_string(), chat_id.clone()),
            turn_id: active_turn.turn_id.clone(),
            user_message_id: Some("msg-active-turn".to_string()),
        })
        .await
        .expect("active turn binding");

    let history = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{agent_id}/chats/{chat_id}"),
        None,
        None,
    )
    .await;
    assert_eq!(history.status(), StatusCode::OK);
    let body = read_json(history).await;
    assert!(body.get("status").is_none());
    assert!(body.get("active_turn").is_none());
    assert!(body.get("control").is_none());
}

#[tokio::test]
async fn chat_history_excludes_remote_and_local_runtime_state() {
    let app = build_app();
    let agent_id = "integ_chats_remote_authority_status";
    create_agent(&app.app, agent_id, "Remote authority status").await;
    let chat = seed_materialized_chat(
        &app,
        &format!("/api/agents/{agent_id}/chats"),
        None,
        "Remote Authority Status Chat",
    )
    .await;
    let chat_id = chat["id"].as_str().expect("chat id").to_string();
    let workspace_dir = app
        .state
        .registry
        .get_or_create(agent_id)
        .await
        .expect("runtime")
        .workspace_dir();
    update_authority_placement(
        Some(&app.state),
        &workspace_dir,
        &rua_core::ChatAddress::agent(agent_id, &chat_id),
        ConversationAuthorityPlacement {
            device_id: "remote-authority-device".to_string(),
            authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(2, 0, [0; 8]),
            base_manifest_hash: format!("sha256:{}", "2".repeat(64)),
        },
    )
    .await
    .expect("seed remote authority placement");

    app.state
        .conversation_authority
        .start_active_turn(ConversationActiveTurnStart {
            address: rua_core::ChatAddress::agent(agent_id.to_string(), chat_id.clone()),
            driver: ConversationDriverKind::Rua,
            authority_device_id: "stale-local-device".to_string(),
            authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
            driver_detail: None,
        })
        .await;

    let history = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{agent_id}/chats/{chat_id}"),
        None,
        None,
    )
    .await;
    assert_eq!(history.status(), StatusCode::OK);
    let body = read_json(history).await;
    assert!(body.get("status").is_none());
    assert!(body.get("active_turn").is_none());
    assert!(body.get("control").is_none());
    assert_eq!(
        body["meta"]["authority_placement"]["device_id"],
        "remote-authority-device"
    );
}

#[tokio::test]
async fn queued_input_edit_delete_routes_mutate_conversation_authority() {
    let app = build_app();
    let agent_id = "integ_chats_queue_authority_routes";
    create_agent(&app.app, agent_id, "Queue authority routes").await;
    let chat = seed_materialized_chat(
        &app,
        &format!("/api/agents/{agent_id}/chats"),
        None,
        "Queue Authority Routes Chat",
    )
    .await;
    let chat_id = chat["id"].as_str().expect("chat id").to_string();

    app.state
        .conversation_authority
        .start_active_turn(ConversationActiveTurnStart {
            address: rua_core::ChatAddress::agent(agent_id.to_string(), chat_id.clone()),
            driver: ConversationDriverKind::Rua,
            authority_device_id: "device-local".to_string(),
            authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
            driver_detail: None,
        })
        .await;
    let first = app
        .state
        .conversation_authority
        .enqueue_input(
            &rua_core::ChatAddress::agent(agent_id, &chat_id),
            pending("first queued"),
            None,
        )
        .await
        .expect("first queue entry");
    let second = app
        .state
        .conversation_authority
        .enqueue_input(
            &rua_core::ChatAddress::agent(agent_id, &chat_id),
            pending("second queued"),
            None,
        )
        .await
        .expect("second queue entry");
    let mut live = app
        .state
        .console_facade
        .connect_chat_activity(agent_id, &chat_id, "viewer-a", None)
        .await;

    let edit = request(
        &app.app,
        Method::PATCH,
        &format!(
            "/api/agents/{agent_id}/chats/{chat_id}/queue/{}",
            second.input_id
        ),
        Some(json!({"message": "second edited"})),
        None,
    )
    .await;
    assert_eq!(edit.status(), StatusCode::OK);
    let edit = read_json(edit).await;
    assert_eq!(edit["entry"]["input_id"], second.input_id);
    assert_eq!(edit["entry"]["text"], "second edited");
    let entries = edit["control"]["queue"]["entries"]
        .as_array()
        .expect("queue entries");
    assert_eq!(entries[0]["input_id"], first.input_id);
    assert_eq!(entries[1]["input_id"], second.input_id);
    assert_eq!(entries[1]["text"], "second edited");
    let live_payload = tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(event) = live.next().await {
            let payload = event.expect("live control payload");
            if payload.contains("\"type\":\"conversation_control\"") {
                let payload = serde_json::from_str::<Value>(&payload).expect("control event json");
                if payload["reason"] == "queue_edited" {
                    return payload;
                }
            }
        }
        panic!("live activity ended before queue_edited control event");
    })
    .await
    .expect("control event");
    assert_eq!(live_payload["reason"], "queue_edited");
    assert_eq!(
        live_payload["control"]["queue"]["entries"][1]["text"],
        "second edited"
    );

    let delete = request(
        &app.app,
        Method::DELETE,
        &format!(
            "/api/agents/{agent_id}/chats/{chat_id}/queue/{}",
            first.input_id
        ),
        None,
        None,
    )
    .await;
    assert_eq!(delete.status(), StatusCode::OK);
    let delete = read_json(delete).await;
    assert_eq!(delete["deleted_input_id"], first.input_id);
    let entries = delete["control"]["queue"]["entries"]
        .as_array()
        .expect("queue entries");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["input_id"], second.input_id);
    assert_eq!(entries[0]["text"], "second edited");

    let missing = request(
        &app.app,
        Method::PATCH,
        &format!(
            "/api/agents/{agent_id}/chats/{chat_id}/queue/{}",
            first.input_id
        ),
        Some(json!({"message": "too late"})),
        None,
    )
    .await;
    assert_eq!(missing.status(), StatusCode::CONFLICT);
    assert_eq!(
        read_json(missing).await["detail"],
        "queued input is not editable"
    );
}

#[tokio::test]
async fn send_now_targets_authority_queue_head_without_redelivering_input() {
    let app = build_app();
    let agent_id = "send_now_authority_agent";
    create_agent(&app.app, agent_id, "Send Now authority").await;
    let chat = seed_materialized_chat(
        &app,
        &format!("/api/agents/{agent_id}/chats"),
        None,
        "Send Now Chat",
    )
    .await;
    let chat_id = chat["id"].as_str().expect("chat id").to_string();
    let authority_epoch_value = chat["meta"]["authority_placement"]["authority_epoch"].clone();
    let authority_epoch =
        rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::parse(
            authority_epoch_value
                .as_str()
                .expect("authority Epoch string"),
        )
        .expect("valid authority Epoch");
    let key = rua_server::streaming::StreamKey::new(
        rua_server::services::console_chat::CONSOLE_CHANNEL_ID,
        rua_core::ChatAddress::agent(agent_id, &chat_id),
        rua_server::streaming::Priority::Normal,
    );
    let (_producer_id, cancel_rx) = app.state.streams.register_active_keyed(&key).await;
    app.state
        .conversation_authority
        .start_active_turn(ConversationActiveTurnStart {
            address: rua_core::ChatAddress::agent(agent_id.to_string(), chat_id.clone()),
            driver: ConversationDriverKind::Rua,
            authority_device_id: rua_server::services::device_context::current_sync_device_id(
                &app.state.sync_device,
            )
            .expect("local Device identity"),
            authority_epoch,
            driver_detail: None,
        })
        .await;
    let first = app
        .state
        .conversation_authority
        .enqueue_input(
            &rua_core::ChatAddress::agent(agent_id, &chat_id),
            pending("first queued"),
            None,
        )
        .await
        .expect("first queue entry");
    let second = app
        .state
        .conversation_authority
        .enqueue_input(
            &rua_core::ChatAddress::agent(agent_id, &chat_id),
            pending("second queued"),
            None,
        )
        .await
        .expect("second queue entry");

    let non_head = request(
        &app.app,
        Method::POST,
        &format!(
            "/api/agents/{agent_id}/chats/{chat_id}/queue/{}/send-now",
            second.input_id
        ),
        Some(json!({
            "command_id": "send-now-non-head",
            "authority_epoch": authority_epoch_value.clone()
        })),
        None,
    )
    .await;
    assert_eq!(non_head.status(), StatusCode::CONFLICT);

    let response = request(
        &app.app,
        Method::POST,
        &format!(
            "/api/agents/{agent_id}/chats/{chat_id}/queue/{}/send-now",
            first.input_id
        ),
        Some(json!({
            "command_id": "send-now-head",
            "authority_epoch": authority_epoch_value
        })),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = read_json(response).await;
    assert_eq!(body["input_id"], first.input_id);
    assert_eq!(body["applied"], true);
    assert_eq!(
        *cancel_rx.borrow(),
        rua_core::runtime::RunControl::Interject
    );
    let entries = body["control"]["queue"]["entries"]
        .as_array()
        .expect("queue entries");
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0]["input_id"], first.input_id);
    assert_eq!(entries[1]["input_id"], second.input_id);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn source_side_chat_create_rejects_authority_placement_metadata() {
    let app = build_app();
    let agent_id = "targeted_chat_create_reject";
    create_agent(&app.app, agent_id, "Targeted chat reject agent").await;

    let resp = request(
        &app.app,
        Method::POST,
        "/api/chats",
        Some(json!({
            "name": "Should not materialize source-side",
            "user_id": "user-source-side-targeted-chat",
            "channel": "console",
            "meta": {
                "authority_placement": {
                    "mode": "fixed",
                    "device_id": "0102030405060708"
                }
            }
        })),
        Some(agent_id),
    )
    .await;

    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = read_json(resp).await;
    assert_eq!(
        body["detail"],
        "authority_placement cannot be set by source-side chat creation"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_create_rejects_remote_initial_authority() {
    let app = build_app();
    let agent_id = "remote_initial_authority_reject";
    create_agent(&app.app, agent_id, "Remote initial authority reject").await;

    let response = request(
        &app.app,
        Method::POST,
        "/api/chats",
        Some(json!({
            "name": "Must start locally",
            "authority_device_id": "0102030405060708"
        })),
        Some(agent_id),
    )
    .await;

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        read_json(response).await["detail"],
        "new chats always start on the current Device; use authority-placement to switch afterward"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conversation_authority_placement_endpoint_is_explicit_and_cannot_be_cleared() {
    let app = build_app();
    let agent_id = "authority_placement_update_agent";
    create_agent(&app.app, agent_id, "Authority placement update agent").await;
    let local_device_id = app
        .state
        .sync_device
        .read()
        .expect("sync device lock")
        .as_ref()
        .map(|device| hex::encode(device.device_id_short()))
        .expect("test sync device");

    let created = seed_materialized_chat(
        &app,
        "/api/chats",
        Some(agent_id),
        "Authority placement update chat",
    )
    .await;
    let chat_id = created["id"].as_str().expect("chat id").to_string();
    let set = request(
        &app.app,
        Method::PUT,
        &format!("/api/agents/{agent_id}/chats/{chat_id}/authority-placement"),
        Some(json!({"device_id": local_device_id})),
        None,
    )
    .await;
    assert_eq!(set.status(), StatusCode::OK);
    let set_body = read_json(set).await;
    assert_eq!(
        set_body["meta"]["authority_placement"]["device_id"],
        local_device_id
    );
    assert_eq!(
        set_body["meta"]["authority_placement"]["authority_epoch"]
            .as_str()
            .map(str::len),
        Some(40)
    );
    let initial_epoch = set_body["meta"]["authority_placement"]["authority_epoch"].clone();
    assert!(
        set_body["meta"]["authority_placement"]["base_manifest_hash"]
            .as_str()
            .is_some_and(|hash| hash.starts_with("sha256:"))
    );

    let after_set = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{agent_id}/chats/{chat_id}"),
        None,
        None,
    )
    .await;
    assert_eq!(after_set.status(), StatusCode::OK);
    let after_set_body = read_json(after_set).await;
    assert_eq!(
        after_set_body["meta"]["authority_placement"]["device_id"],
        local_device_id
    );

    let clear = request(
        &app.app,
        Method::PUT,
        &format!("/api/agents/{agent_id}/chats/{chat_id}/authority-placement"),
        Some(json!({"device_id": null})),
        None,
    )
    .await;
    assert_eq!(clear.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let after_clear = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{agent_id}/chats/{chat_id}"),
        None,
        None,
    )
    .await;
    assert_eq!(after_clear.status(), StatusCode::OK);
    let after_clear_body = read_json(after_clear).await;
    assert_eq!(
        after_clear_body["meta"]["authority_placement"]["device_id"],
        local_device_id
    );
    assert_eq!(
        after_clear_body["meta"]["authority_placement"]["authority_epoch"],
        initial_epoch
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn manual_authority_switch_advances_hlc_without_requiring_current_authority() {
    let app = build_app();
    seed_reasoning_provider(&app).await;
    store_default_model(&app, "dashscope", "qwen3.7-plus").await;
    let agent_id = "authority_manual_switch_agent";
    create_agent(&app.app, agent_id, "Authority manual switch agent").await;
    let local_device_id = app
        .state
        .sync_device
        .read()
        .expect("sync device lock")
        .as_ref()
        .map(|device| hex::encode(device.device_id_short()))
        .expect("test sync device");
    let created = seed_materialized_chat(
        &app,
        "/api/chats",
        Some(agent_id),
        "Authority manual switch chat",
    )
    .await;
    let chat_id = created["id"].as_str().expect("chat id").to_string();
    let workspace_dir = app
        .state
        .registry
        .get_or_create(agent_id)
        .await
        .expect("runtime")
        .workspace_dir();
    update_authority_placement(
        Some(&app.state),
        &workspace_dir,
        &rua_core::ChatAddress::agent(agent_id, &chat_id),
        ConversationAuthorityPlacement {
            device_id: "previous-authority-device".to_string(),
            authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(7, 0, [0; 8]),
            base_manifest_hash: format!("sha256:{}", "7".repeat(64)),
        },
    )
    .await
    .expect("seed previous authority");

    let switched = request(
        &app.app,
        Method::PUT,
        &format!("/api/agents/{agent_id}/chats/{chat_id}/authority-placement"),
        Some(json!({"device_id": local_device_id})),
        None,
    )
    .await;

    assert_eq!(switched.status(), StatusCode::OK);
    let body = read_json(switched).await;
    assert_eq!(
        body["meta"]["authority_placement"]["device_id"],
        local_device_id
    );
    let switched_epoch =
        rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::parse(
            body["meta"]["authority_placement"]["authority_epoch"]
                .as_str()
                .expect("authority Epoch string"),
        )
        .expect("parse switched Epoch");
    assert!(
        switched_epoch
            > rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(
                7,
                0,
                [0; 8],
            )
    );
    assert!(body["meta"]["authority_placement"]["base_manifest_hash"]
        .as_str()
        .is_some_and(|hash| hash.starts_with("sha256:")));

    let tasks = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{agent_id}/chats/{chat_id}/tasks"),
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(tasks.status(), StatusCode::OK);
    assert!(
        read_json(tasks).await.as_array().unwrap().is_empty(),
        "a forced transfer from an unavailable old authority must not adopt its Tasks"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn manual_authority_switch_rejects_non_terminal_history_boundary() {
    let app = build_app();
    let agent_id = "authority_non_terminal_switch_agent";
    create_agent(&app.app, agent_id, "Authority non-terminal switch agent").await;
    let local_device_id = app
        .state
        .sync_device
        .read()
        .expect("sync device lock")
        .as_ref()
        .map(|device| hex::encode(device.device_id_short()))
        .expect("test sync device");
    let created = seed_materialized_chat(
        &app,
        "/api/chats",
        Some(agent_id),
        "Authority non-terminal switch chat",
    )
    .await;
    let chat_id = created["id"].as_str().expect("chat id").to_string();
    let workspace_dir = app
        .state
        .registry
        .get_or_create(agent_id)
        .await
        .expect("runtime")
        .workspace_dir();
    update_authority_placement(
        Some(&app.state),
        &workspace_dir,
        &rua_core::ChatAddress::agent(agent_id, &chat_id),
        ConversationAuthorityPlacement {
            device_id: "previous-authority-device".to_string(),
            authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(4, 0, [0; 8]),
            base_manifest_hash: format!("sha256:{}", "4".repeat(64)),
        },
    )
    .await
    .expect("seed previous authority");
    append_in_progress_evergreen_turn_with_id(
        &app,
        agent_id,
        &chat_id,
        "running-turn",
        stored_message(
            "running-user",
            "user",
            "still running",
            "2026-07-20T00:00:00Z",
            "user",
        ),
    )
    .await;

    let rejected = request(
        &app.app,
        Method::PUT,
        &format!("/api/agents/{agent_id}/chats/{chat_id}/authority-placement"),
        Some(json!({"device_id": local_device_id})),
        None,
    )
    .await;

    assert_eq!(rejected.status(), StatusCode::CONFLICT);
    assert_eq!(
        read_json(rejected).await["detail"],
        "latest conversation turn is not terminal"
    );
    let history = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{agent_id}/chats/{chat_id}"),
        None,
        None,
    )
    .await;
    let history = read_json(history).await;
    assert_eq!(
        history["meta"]["authority_placement"]["device_id"],
        "previous-authority-device"
    );
    assert_eq!(
        history["meta"]["authority_placement"]["authority_epoch"],
        "0000000000000004000000000000000000000000"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conversation_authority_placement_update_rejects_while_turn_is_active() {
    let app = build_app();
    let agent_id = "authority_placement_active_run_agent";
    create_agent(&app.app, agent_id, "Execution target active turn agent").await;
    let created = seed_materialized_chat(
        &app,
        "/api/chats",
        Some(agent_id),
        "Execution target active turn chat",
    )
    .await;
    let chat_id = created["id"].as_str().expect("chat id").to_string();
    let authority_device_id = created["meta"]["authority_placement"]["device_id"]
        .as_str()
        .expect("authority Device")
        .to_string();
    let authority_epoch =
        serde_json::from_value(created["meta"]["authority_placement"]["authority_epoch"].clone())
            .expect("authority Epoch");
    app.state
        .conversation_authority
        .start_active_turn(ConversationActiveTurnStart {
            address: rua_core::ChatAddress::agent(agent_id.to_string(), chat_id.clone()),
            driver: ConversationDriverKind::Rua,
            authority_device_id: authority_device_id.clone(),
            authority_epoch,
            driver_detail: None,
        })
        .await;

    let rejected = request(
        &app.app,
        Method::PUT,
        &format!("/api/agents/{agent_id}/chats/{chat_id}/authority-placement"),
        Some(json!({"device_id": "device-b"})),
        None,
    )
    .await;

    assert_eq!(rejected.status(), StatusCode::CONFLICT);
    assert_eq!(
        read_json(rejected).await["detail"],
        "cannot change authority placement while the chat has an active turn"
    );
    let history = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{agent_id}/chats/{chat_id}"),
        None,
        None,
    )
    .await;
    assert_eq!(history.status(), StatusCode::OK);
    let history = read_json(history).await;
    assert_eq!(
        history["meta"]["authority_placement"]["device_id"],
        authority_device_id
    );
    assert_eq!(
        history["meta"]["authority_placement"]["authority_epoch"],
        created["meta"]["authority_placement"]["authority_epoch"]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conversation_authority_placement_update_rejects_while_acp_binding_is_active() {
    let app = build_app();
    let agent_id = "authority_placement_acp_lock_agent";
    create_agent(&app.app, agent_id, "Authority placement ACP lock agent").await;
    let created = seed_materialized_chat(
        &app,
        "/api/chats",
        Some(agent_id),
        "Execution target ACP lock chat",
    )
    .await;
    let chat_id = created["id"].as_str().expect("chat id").to_string();
    let authority_device_id = created["meta"]["authority_placement"]["device_id"]
        .as_str()
        .expect("authority Device")
        .to_string();
    let workspace_dir = app
        .state
        .registry
        .get_or_create(agent_id)
        .await
        .expect("runtime")
        .workspace_dir();
    persist_acp_binding(
        Some(&app.state),
        &workspace_dir,
        &rua_core::ChatAddress::agent(agent_id, &chat_id),
        agent_id,
        AcpBindingMetadata {
            target_device_id: Some(authority_device_id),
            runner: "codex".to_string(),
            cwd: Some("/tmp".to_string()),
            runner_session_id: Some("runner-session".to_string()),
        },
    )
    .await
    .expect("persist acp binding");

    let rejected = request(
        &app.app,
        Method::PUT,
        &format!("/api/agents/{agent_id}/chats/{chat_id}/authority-placement"),
        Some(json!({"device_id": "device-b"})),
        None,
    )
    .await;

    assert_eq!(rejected.status(), StatusCode::CONFLICT);
    assert_eq!(
        read_json(rejected).await["detail"],
        "Current chat is in ACP mode. Exit ACP before changing the authority device."
    );

    let history = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{agent_id}/chats/{chat_id}"),
        None,
        None,
    )
    .await;
    assert_eq!(history.status(), StatusCode::OK);
    let body = read_json(history).await;
    assert_eq!(body["meta"]["acp_binding"]["kind"], "acp_passthrough");
    assert!(body.get("control").is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_history_read_does_not_reconcile_in_progress_turns() {
    let app = build_app();
    let agent_id = "integ_chats_readonly_in_progress_01";
    create_agent(&app.app, agent_id, "Read-only in-progress agent").await;

    let created = seed_materialized_chat(
        &app,
        "/api/chats",
        Some(agent_id),
        "Read-only in-progress chat",
    )
    .await;
    let chat_id = created["id"].as_str().expect("chat id").to_string();
    append_in_progress_evergreen_turn_with_id(
        &app,
        agent_id,
        &chat_id,
        "turn-orphan-open-1",
        stored_message(
            "orphan-open-user-1",
            "user",
            "open must not reconcile this turn",
            "2026-06-24T00:00:00Z",
            "user",
        ),
    )
    .await;

    let history = request(
        &app.app,
        Method::GET,
        &format!("/api/chats/{chat_id}"),
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(history.status(), StatusCode::OK);
    let history = read_json(history).await;
    let messages = history["messages"].as_array().expect("messages");
    assert_eq!(
        messages.len(),
        1,
        "Chat History read must not fabricate assistant or approval messages: {history:?}"
    );
    assert_eq!(messages[0]["id"], "orphan-open-user-1");
    assert!(history.get("run").is_none());
    assert!(history.get("status").is_none());
    assert!(history.get("active_turn").is_none());
    assert!(history.get("control").is_none());

    let workspace = app
        .state
        .registry
        .get_or_create(agent_id)
        .await
        .expect("workspace runtime")
        .workspace_dir();
    let store = WorkspaceSessionStore::new(&workspace);
    let turns = store
        .read_turn_range(&chat_id, 1, usize::MAX)
        .await
        .expect("read turns");
    assert_eq!(
        turns[0].lifecycle.status, "in_progress",
        "Chat History read must not write a terminal lifecycle"
    );
    let messages = store
        .read_message_range(&chat_id, 1, usize::MAX)
        .await
        .expect("read messages");
    assert!(
        messages
            .iter()
            .filter_map(RuntimeNote::from_message_log_entry)
            .all(|note| !matches!(
                note.payload,
                RuntimeNotePayload::ConversationRuntimeTerminal(ref terminal)
                    if terminal.turn_id == "turn-orphan-open-1"
            )),
        "Chat History read must not write a terminal Runtime Note"
    );
    assert!(
        app.state
            .security
            .approval_authority()
            .list_pending_for_address(&rua_core::ChatAddress::agent(agent_id, &chat_id))
            .await
            .is_empty(),
        "Chat History read must not recreate pending approval requests"
    );

    let history_again = request(
        &app.app,
        Method::GET,
        &format!("/api/chats/{chat_id}"),
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(history_again.status(), StatusCode::OK);
    let turns_after_second_open = store
        .read_turn_range(&chat_id, 1, usize::MAX)
        .await
        .expect("read turns after second open");
    assert_eq!(
        turns_after_second_open[0].lifecycle.status, "in_progress",
        "repeated Chat History reads must remain read-only"
    );

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_history_reads_evergreen_message_log_when_legacy_session_is_absent() {
    let app = build_app();
    let agent_id = "integ_chats_evergreen_history_01";
    create_agent(&app.app, agent_id, "Evergreen history agent").await;

    let created =
        seed_materialized_chat(&app, "/api/chats", Some(agent_id), "Evergreen history chat").await;
    let chat_id = created["id"].as_str().expect("chat id").to_string();
    append_evergreen_turn(
        &app,
        agent_id,
        &chat_id,
        vec![
            stored_message(
                "evergreen-user-1",
                "user",
                "open this raw evergreen user message",
                "2026-06-19T00:00:00Z",
                "user",
            ),
            stored_message(
                "evergreen-assistant-1",
                "assistant",
                "open this raw evergreen assistant message",
                "2026-06-19T00:00:01Z",
                "assistant",
            ),
        ],
    )
    .await;

    let history = request(
        &app.app,
        Method::GET,
        &format!("/api/chats/{chat_id}"),
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(history.status(), StatusCode::OK);
    let history = read_json(history).await;
    let messages = history["messages"].as_array().expect("messages");
    assert_eq!(messages.len(), 2, "history body: {history:?}");
    assert_eq!(messages[0]["id"], "evergreen-user-1");
    assert_eq!(messages[0]["role"], "user");
    assert_eq!(
        messages[0]["content"][0]["text"],
        "open this raw evergreen user message"
    );
    assert_eq!(messages[1]["id"], "evergreen-assistant-1");
    assert_eq!(messages[1]["role"], "assistant");
    assert_eq!(
        history["durable_coverage"]["latest_message_sequence"], 2,
        "durable coverage must end at the latest complete Evergreen Message"
    );
    let manifest_hash = history["durable_coverage"]["manifest_hash"]
        .as_str()
        .expect("durable coverage manifest hash");
    assert!(
        manifest_hash.starts_with("sha256:"),
        "durable coverage must expose the canonical manifest identity: {history:?}"
    );

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_history_paginates_by_complete_evergreen_turns() {
    let app = build_app();
    let agent_id = "integ_chats_evergreen_turn_page_01";
    create_agent(&app.app, agent_id, "Evergreen turn page agent").await;

    let created = seed_materialized_chat(
        &app,
        "/api/chats",
        Some(agent_id),
        "Evergreen turn page chat",
    )
    .await;
    let chat_id = created["id"].as_str().expect("chat id").to_string();

    for turn in 1..=3 {
        append_evergreen_turn_with_id(
            &app,
            agent_id,
            &chat_id,
            &format!("evergreen-page-turn-{turn}"),
            vec![
                stored_message(
                    &format!("evergreen-page-user-{turn}"),
                    "user",
                    &format!("question {turn}"),
                    "2026-06-19T00:00:00Z",
                    "user",
                ),
                stored_message(
                    &format!("evergreen-page-assistant-{turn}"),
                    "assistant",
                    &format!("answer {turn}"),
                    "2026-06-19T00:00:01Z",
                    "assistant",
                ),
            ],
        )
        .await;
    }

    let tail = request(
        &app.app,
        Method::GET,
        &format!("/api/chats/{chat_id}?turn_limit=2"),
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(tail.status(), StatusCode::OK);
    let tail = read_json(tail).await;
    assert_eq!(tail["title"], "Evergreen turn page chat");
    assert_eq!(tail["oldest_turn_ordinal"], 2);
    assert_eq!(tail["newest_turn_ordinal"], 3);
    assert_eq!(tail["has_more_before"], true);
    let tail_messages = tail["messages"].as_array().expect("tail messages");
    assert_eq!(tail_messages.len(), 4, "tail body: {tail:?}");
    assert_eq!(tail_messages[0]["id"], "evergreen-page-user-2");
    assert_eq!(tail_messages[1]["id"], "evergreen-page-assistant-2");
    assert_eq!(tail_messages[2]["id"], "evergreen-page-user-3");
    assert_eq!(tail_messages[3]["id"], "evergreen-page-assistant-3");

    let previous = request(
        &app.app,
        Method::GET,
        &format!("/api/chats/{chat_id}?turn_limit=2&before_turn_ordinal=2"),
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(previous.status(), StatusCode::OK);
    let previous = read_json(previous).await;
    assert_eq!(previous["oldest_turn_ordinal"], 1);
    assert_eq!(previous["newest_turn_ordinal"], 1);
    assert_eq!(previous["has_more_before"], false);
    let previous_messages = previous["messages"].as_array().expect("previous messages");
    assert_eq!(previous_messages.len(), 2, "previous body: {previous:?}");
    assert_eq!(previous_messages[0]["id"], "evergreen-page-user-1");
    assert_eq!(previous_messages[1]["id"], "evergreen-page-assistant-1");

    let located = request(
        &app.app, Method::GET,
        &format!("/api/agents/{agent_id}/chats/{chat_id}?turn_limit=1&around_turn_id=evergreen-page-turn-1"),
        None, None,
    ).await;
    assert_eq!(located.status(), StatusCode::OK);
    let located = read_json(located).await;
    assert_eq!(located["oldest_turn_ordinal"], 1);
    assert_eq!(located["newest_turn_ordinal"], 1);
    assert_eq!(located["messages"].as_array().unwrap().len(), 2);
    let invalid = request(
        &app.app, Method::GET,
        &format!("/api/agents/{agent_id}/chats/{chat_id}?before_turn_ordinal=2&around_turn_id=evergreen-page-turn-1"),
        None, None,
    ).await;
    assert_eq!(invalid.status(), StatusCode::UNPROCESSABLE_ENTITY);

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_history_projects_pending_ask_user_as_answer_request_not_approval() {
    let app = build_app();
    let agent_id = "integ_chats_pending_ask_user_01";
    create_agent(&app.app, agent_id, "Pending ask_user history agent").await;

    let created =
        seed_materialized_chat(&app, "/api/chats", Some(agent_id), "Pending ask_user chat").await;
    let chat_id = created["id"].as_str().expect("chat id").to_string();
    let workspace = app
        .state
        .registry
        .get_or_create(agent_id)
        .await
        .expect("workspace runtime")
        .workspace_dir();
    let store = WorkspaceSessionStore::new(&workspace);
    let mut header = store
        .load_chat_header(&chat_id)
        .await
        .expect("load evergreen chat header")
        .expect("chat header");
    header.context_snapshot.pending_action = Some(json!({
        "action_type": "ask_user",
        "request_id": "ask-1",
        "prompt": "Which option?",
        "questions": [
            {
                "question": "Which option?",
                "options": ["A", "B"],
                "multi_select": false
            }
        ],
        "status": "waiting"
    }));
    store
        .save_chat_header(&header)
        .await
        .expect("save pending ask_user header");

    let history = request(
        &app.app,
        Method::GET,
        &format!("/api/chats/{chat_id}"),
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(history.status(), StatusCode::OK);
    let history = read_json(history).await;
    let messages = history["messages"].as_array().expect("messages");
    let pending = messages
        .iter()
        .find(|message| message["id"] == "ask-1")
        .expect("pending ask_user projection");
    assert_eq!(pending["type"], "ask_user_request");
    assert_eq!(pending["content"][0]["type"], "ask_user_request");
    assert_eq!(pending["content"][0]["data"]["request_id"], "ask-1");
    assert_eq!(
        pending["content"][0]["data"]["questions"][0]["question"],
        "Which option?"
    );
    assert_ne!(pending["type"], "tool_approval_request");

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_history_does_not_project_tool_approval_cards_from_header_or_authority() {
    let app = build_app();
    let agent_id = "integ_chats_pending_tool_approval_01";
    create_agent(&app.app, agent_id, "Pending tool approval history agent").await;

    let created = seed_materialized_chat(
        &app,
        "/api/chats",
        Some(agent_id),
        "Pending tool approval chat",
    )
    .await;
    let chat_id = created["id"].as_str().expect("chat id").to_string();
    let workspace = app
        .state
        .registry
        .get_or_create(agent_id)
        .await
        .expect("workspace runtime")
        .workspace_dir();
    let store = WorkspaceSessionStore::new(&workspace);
    let mut header = store
        .load_chat_header(&chat_id)
        .await
        .expect("load evergreen chat header")
        .expect("chat header");
    header.context_snapshot.pending_action = Some(json!({
        "action_type": "tool_approval",
        "request_id": "stale-header-approval",
        "call_id": "stale-call",
        "name": "bash",
        "prompt": "stale header prompt",
        "status": "waiting"
    }));
    store
        .save_chat_header(&header)
        .await
        .expect("save stale tool approval header");

    let first = app
        .state
        .security
        .approval_authority()
        .create_request(ApprovalRequestInput {
            owner: rua_security::ApprovalOwner::chat(rua_core::ChatAddress::agent(
                agent_id, &chat_id,
            )),
            agent_id: agent_id.into(),
            turn_id: chat_id.clone(),
            tool_call_id: "call-1".into(),
            tool_name: "bash".into(),
            trigger: ApprovalTrigger::approval_mode("Bash requires confirmation"),
            tool_call: ToolCallSnapshot {
                display: "bash".into(),
                args: json!({"command": "du -sh target"}),
            },
        })
        .await;
    let second = app
        .state
        .security
        .approval_authority()
        .create_request(ApprovalRequestInput {
            owner: rua_security::ApprovalOwner::chat(rua_core::ChatAddress::agent(
                agent_id, &chat_id,
            )),
            agent_id: agent_id.into(),
            turn_id: chat_id.clone(),
            tool_call_id: "call-2".into(),
            tool_name: "read".into(),
            trigger: ApprovalTrigger::approval_mode("Read requires confirmation"),
            tool_call: ToolCallSnapshot {
                display: "read".into(),
                args: json!({"path": "/tmp/a"}),
            },
        })
        .await;

    let history = request(
        &app.app,
        Method::GET,
        &format!("/api/chats/{chat_id}"),
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(history.status(), StatusCode::OK);
    let history = read_json(history).await;
    let messages = history["messages"].as_array().expect("messages");
    assert!(
        messages
            .iter()
            .all(|message| message["type"] != "tool_approval_request"),
        "chat history must not synthesize tool approval cards; approval UI reads the authority snapshot"
    );
    assert!(
        messages.iter().all(|message| message["id"] != first.id
            && message["id"] != second.id
            && message["id"] != "stale-header-approval"),
        "neither authority pending requests nor stale header pending_action should appear as chat messages"
    );

    let snapshot = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{agent_id}/approvals/snapshot?chat_id={chat_id}"),
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(snapshot.status(), StatusCode::OK);
    let snapshot = read_json(snapshot).await;
    let pending_ids: Vec<_> = snapshot["pending"]
        .as_array()
        .expect("pending approvals")
        .iter()
        .map(|approval| approval["id"].clone())
        .collect();
    assert_eq!(
        pending_ids,
        vec![json!(first.id), json!(second.id)],
        "approval queue state stays available through ApprovalAuthority snapshot"
    );

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_history_exposes_evergreen_turn_and_selected_variant_metadata() {
    let app = build_app();
    let agent_id = "integ_chats_evergreen_variant_01";
    create_agent(&app.app, agent_id, "Evergreen variant history agent").await;

    let created =
        seed_materialized_chat(&app, "/api/chats", Some(agent_id), "Evergreen variant chat").await;
    let chat_id = created["id"].as_str().expect("chat id").to_string();
    let workspace = app
        .state
        .registry
        .get_or_create(agent_id)
        .await
        .expect("workspace runtime")
        .workspace_dir();
    WorkspaceSessionStore::new(&workspace)
        .append_conversation_turn(
            &chat_id,
            NewConversationTurn {
                turn_id: "evergreen-turn-variant-1".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-06-19T00:00:00Z".to_string(),
                    assistant_started_at: Some("2026-06-19T00:00:01Z".to_string()),
                    assistant_completed_at: Some("2026-06-19T00:00:02Z".to_string()),
                    status: "completed".to_string(),
                },
                selected_variant_id: Some("evergreen-assistant-variant-1".to_string()),
                messages: vec![
                    NewConversationMessage {
                        message_id: "evergreen-user-variant-1".to_string(),
                        role: "user".to_string(),
                        created_at: "2026-06-19T00:00:00Z".to_string(),
                        status: "completed".to_string(),
                        content: json!([{"type": "text", "text": "question"}]),
                        provider: None,
                        tool: None,
                        metadata: serde_json::Map::new(),
                    },
                    NewConversationMessage {
                        message_id: "evergreen-assistant-variant-1".to_string(),
                        role: "assistant".to_string(),
                        created_at: "2026-06-19T00:00:01Z".to_string(),
                        status: "completed".to_string(),
                        content: json!([{"type": "text", "text": "selected answer"}]),
                        provider: None,
                        tool: None,
                        metadata: serde_json::Map::new(),
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
    let history = read_json(history).await;
    let assistant = history["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .find(|message| message["role"] == "assistant")
        .expect("assistant message");
    assert_eq!(assistant["turn_id"], "evergreen-turn-variant-1");
    assert_eq!(assistant["variant_id"], "evergreen-assistant-variant-1");
    assert_eq!(
        assistant["selected_variant_id"],
        "evergreen-assistant-variant-1"
    );
    assert_eq!(
        assistant["turn_status"], "completed",
        "a terminal Turn's records carry the terminal lifecycle status"
    );
    let user = history["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .find(|message| message["role"] == "user")
        .expect("user message");
    assert_eq!(user["turn_status"], "completed");

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_history_exposes_in_progress_turn_status_for_mid_turn_records() {
    let app = build_app();
    let agent_id = "integ_chats_in_progress_turn_status_01";
    create_agent(&app.app, agent_id, "In-progress turn status agent").await;

    let created = seed_materialized_chat(
        &app,
        "/api/chats",
        Some(agent_id),
        "In-progress turn status chat",
    )
    .await;
    let chat_id = created["id"].as_str().expect("chat id").to_string();
    let workspace = app
        .state
        .registry
        .get_or_create(agent_id)
        .await
        .expect("workspace runtime")
        .workspace_dir();

    WorkspaceSessionStore::new(&workspace)
        .append_conversation_turn(
            &chat_id,
            NewConversationTurn {
                turn_id: "in-progress-turn-1".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-06-19T00:00:00Z".to_string(),
                    assistant_started_at: Some("2026-06-19T00:00:01Z".to_string()),
                    assistant_completed_at: None,
                    status: "in_progress".to_string(),
                },
                selected_variant_id: None,
                messages: vec![
                    NewConversationMessage {
                        message_id: "in-progress-user-1".to_string(),
                        role: "user".to_string(),
                        created_at: "2026-06-19T00:00:00Z".to_string(),
                        status: "completed".to_string(),
                        content: json!([{"type": "text", "text": "use the tool"}]),
                        provider: None,
                        tool: None,
                        metadata: serde_json::Map::new(),
                    },
                    NewConversationMessage {
                        message_id: "in-progress-tool-use-1".to_string(),
                        role: "assistant".to_string(),
                        created_at: "2026-06-19T00:00:01Z".to_string(),
                        status: "completed".to_string(),
                        content: json!([{
                            "type": "tool_use",
                            "id": "call-1",
                            "name": "read",
                            "input": {"path": "README.md"}
                        }]),
                        provider: None,
                        tool: None,
                        metadata: serde_json::Map::new(),
                    },
                ],
            },
        )
        .await
        .expect("append in-progress turn");

    let history = request(
        &app.app,
        Method::GET,
        &format!("/api/chats/{chat_id}"),
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(history.status(), StatusCode::OK);
    let history = read_json(history).await;
    let assistant = history["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .find(|message| message["role"] == "assistant")
        .expect("assistant tool_use message");
    assert_eq!(assistant["turn_id"], "in-progress-turn-1");
    assert_eq!(
        assistant["turn_status"], "in_progress",
        "a mid-Turn completed record must still report the Turn as in_progress"
    );

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn acp_history_preview_persists_as_independent_ui_only_turn() {
    let app = build_app();
    let agent_id = "integ_chats_acp_history_preview_01";
    create_agent(&app.app, agent_id, "ACP history preview agent").await;

    let created = seed_materialized_chat(
        &app,
        "/api/chats",
        Some(agent_id),
        "ACP history preview chat",
    )
    .await;
    let chat_id = created["id"].as_str().expect("chat id").to_string();
    append_evergreen_turn(
        &app,
        agent_id,
        &chat_id,
        vec![
            stored_message(
                "user-before-preview",
                "user",
                "before ACP",
                "2026-06-19T00:00:00Z",
                "user",
            ),
            stored_message(
                "assistant-before-preview",
                "assistant",
                "previous assistant answer",
                "2026-06-19T00:00:01Z",
                "assistant",
            ),
        ],
    )
    .await;

    let response = request(
        &app.app,
        Method::POST,
        &format!("/api/agents/{agent_id}/chats/{chat_id}/acp-history-preview"),
        Some(json!({
            "preview_id": 7,
            "text": "previous ACP transcript",
            "runner": "codex"
        })),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let payload = read_json(response).await;
    assert_eq!(payload["created"], json!(true));
    assert_eq!(payload["turn_id"], json!("acp-history-preview-7"));

    let duplicate = request(
        &app.app,
        Method::POST,
        &format!("/api/agents/{agent_id}/chats/{chat_id}/acp-history-preview"),
        Some(json!({
            "preview_id": 7,
            "text": "previous ACP transcript",
            "runner": "codex"
        })),
        None,
    )
    .await;
    assert_eq!(duplicate.status(), StatusCode::OK);
    let duplicate_payload = read_json(duplicate).await;
    assert_eq!(duplicate_payload["created"], json!(false));

    let history_response = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{agent_id}/chats/{chat_id}"),
        None,
        None,
    )
    .await;
    assert_eq!(history_response.status(), StatusCode::OK);
    let history = read_json(history_response).await;
    let messages = history["messages"].as_array().expect("messages");
    assert_eq!(
        messages
            .iter()
            .filter(|message| message["type"] == json!("acp_history_preview"))
            .count(),
        1
    );
    let preview = messages
        .iter()
        .find(|message| message["type"] == json!("acp_history_preview"))
        .expect("preview message");
    assert_eq!(preview["role"], json!("ui"));
    assert_eq!(preview["turn_id"], json!("acp-history-preview-7"));
    assert_eq!(preview["metadata"]["ui_only"], json!(true));
    assert_eq!(preview["metadata"]["model_visible"], json!(false));
    assert_eq!(preview["metadata"]["origin"]["kind"], json!("acp"));
    assert_eq!(preview["metadata"]["origin"]["runner"], json!("codex"));
    assert_eq!(
        preview["content"][0]["text"],
        json!("previous ACP transcript")
    );
    assert!(
        messages
            .iter()
            .any(|message| message["id"] == json!("assistant-before-preview")),
        "existing assistant output must remain a separate history message: {messages:?}"
    );

    let workspace = app
        .state
        .registry
        .get_or_create(agent_id)
        .await
        .expect("workspace runtime")
        .workspace_dir();
    let turns = WorkspaceSessionStore::new(&workspace)
        .read_turn_range(&chat_id, 1, usize::MAX)
        .await
        .expect("turns");
    let preview_turn = turns
        .iter()
        .find(|turn| turn.turn_id == "acp-history-preview-7")
        .expect("preview turn");
    assert_eq!(preview_turn.metadata.get("ui_only"), Some(&json!(true)));
    assert_eq!(
        preview_turn.metadata.get("model_visible"),
        Some(&json!(false))
    );

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_history_projects_evergreen_response_variants_in_place() {
    let app = build_app();
    let agent_id = "integ_chats_evergreen_variants_projection_01";
    create_agent(&app.app, agent_id, "Evergreen variant projection agent").await;

    let created = seed_materialized_chat(
        &app,
        "/api/chats",
        Some(agent_id),
        "Evergreen variant projection chat",
    )
    .await;
    let chat_id = created["id"].as_str().expect("chat id").to_string();
    let workspace = app
        .state
        .registry
        .get_or_create(agent_id)
        .await
        .expect("workspace runtime")
        .workspace_dir();
    let store = WorkspaceSessionStore::new(&workspace);
    store
        .append_conversation_turn(
            &chat_id,
            NewConversationTurn {
                turn_id: "evergreen-turn-variant-projection-1".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-06-19T00:00:00Z".to_string(),
                    assistant_started_at: Some("2026-06-19T00:00:01Z".to_string()),
                    assistant_completed_at: Some("2026-06-19T00:00:02Z".to_string()),
                    status: "completed".to_string(),
                },
                selected_variant_id: Some("evergreen-assistant-first".to_string()),
                messages: vec![
                    NewConversationMessage {
                        message_id: "evergreen-user-variant-projection".to_string(),
                        role: "user".to_string(),
                        created_at: "2026-06-19T00:00:00Z".to_string(),
                        status: "completed".to_string(),
                        content: json!([{"type": "text", "text": "question"}]),
                        provider: None,
                        tool: None,
                        metadata: serde_json::Map::new(),
                    },
                    NewConversationMessage {
                        message_id: "evergreen-assistant-first".to_string(),
                        role: "assistant".to_string(),
                        created_at: "2026-06-19T00:00:01Z".to_string(),
                        status: "completed".to_string(),
                        content: json!([{"type": "text", "text": "first answer"}]),
                        provider: None,
                        tool: None,
                        metadata: serde_json::Map::new(),
                    },
                ],
            },
        )
        .await
        .expect("append evergreen turn");
    store
        .append_messages_to_turn_as_new_variant(
            &chat_id,
            "evergreen-turn-variant-projection-1",
            TurnLifecycle {
                user_submitted_at: "2026-06-19T00:00:00Z".to_string(),
                assistant_started_at: Some("2026-06-19T00:00:01Z".to_string()),
                assistant_completed_at: Some("2026-06-19T00:00:03Z".to_string()),
                status: "completed".to_string(),
            },
            Some("evergreen-assistant-second".to_string()),
            vec![NewConversationMessage {
                message_id: "evergreen-assistant-second".to_string(),
                role: "assistant".to_string(),
                created_at: "2026-06-19T00:00:03Z".to_string(),
                status: "completed".to_string(),
                content: json!([{"type": "text", "text": "second answer"}]),
                provider: None,
                tool: None,
                metadata: serde_json::Map::new(),
            }],
        )
        .await
        .expect("append evergreen variant");

    let history = request(
        &app.app,
        Method::GET,
        &format!("/api/chats/{chat_id}"),
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(history.status(), StatusCode::OK);
    let history = read_json(history).await;
    let messages = history["messages"].as_array().expect("messages");
    assert_eq!(messages.len(), 2, "history body: {history:?}");
    assert_eq!(messages[0]["id"], "evergreen-user-variant-projection");
    let assistant = &messages[1];
    assert_eq!(assistant["id"], "evergreen-assistant-second");
    assert_eq!(assistant["content"][0]["text"], "second answer");
    assert_eq!(
        assistant["selected_variant_id"],
        "evergreen-assistant-second"
    );
    let variants = assistant["assistant_response_variants"]
        .as_array()
        .expect("variants");
    assert_eq!(variants.len(), 2, "assistant body: {assistant:?}");
    assert_eq!(variants[0]["variant_id"], "evergreen-assistant-first");
    assert_eq!(
        variants[0]["output"][0]["content"][0]["text"],
        "first answer"
    );
    assert_eq!(
        variants[0]["output"][0]["started_at"],
        "2026-06-19T00:00:01Z"
    );
    assert_eq!(
        variants[0]["output"][0]["finished_at"],
        "2026-06-19T00:00:01Z"
    );
    assert_eq!(variants[1]["variant_id"], "evergreen-assistant-second");
    assert_eq!(
        variants[1]["output"][0]["content"][0]["text"],
        "second answer"
    );
    assert_eq!(
        variants[1]["output"][0]["started_at"],
        "2026-06-19T00:00:03Z"
    );
    assert_eq!(
        variants[1]["output"][0]["finished_at"],
        "2026-06-19T00:00:03Z"
    );

    let selected = request(
        &app.app,
        Method::PUT,
        &format!("/api/agents/{agent_id}/chats/{chat_id}"),
        Some(json!({
            "turn_id": "evergreen-turn-variant-projection-1",
            "selected_variant_id": "evergreen-assistant-first"
        })),
        None,
    )
    .await;
    assert_eq!(selected.status(), StatusCode::OK);

    let history = request(
        &app.app,
        Method::GET,
        &format!("/api/chats/{chat_id}"),
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(history.status(), StatusCode::OK);
    let history = read_json(history).await;
    let messages = history["messages"].as_array().expect("messages");
    assert_eq!(messages.len(), 2, "history body: {history:?}");
    assert_eq!(messages[1]["id"], "evergreen-assistant-first");
    assert_eq!(messages[1]["content"][0]["text"], "first answer");
    assert_eq!(
        messages[1]["assistant_response_variants"]
            .as_array()
            .expect("variants")
            .len(),
        2
    );

    let turns = store
        .read_turn_range(&chat_id, 1, usize::MAX)
        .await
        .expect("read turns");
    assert_eq!(turns[0].assistant_response_sequences, vec![2]);
    assert_eq!(turns[0].assistant_response_variants.len(), 2);
    let raw = store
        .read_message_range(&chat_id, 1, 3)
        .await
        .expect("read raw messages");
    assert_eq!(raw[1].message_id, "evergreen-assistant-first");
    assert_eq!(raw[2].message_id, "evergreen-assistant-second");

    assert!(!store
        .set_turn_selected_variant(
            &chat_id,
            "evergreen-turn-variant-projection-1",
            "not-a-real-variant".to_string(),
        )
        .await
        .expect("reject invalid variant"));

    let rejected = request(
        &app.app,
        Method::PUT,
        &format!("/api/agents/{agent_id}/chats/{chat_id}"),
        Some(json!({
            "turn_id": "evergreen-turn-variant-projection-1",
            "selected_variant_id": "not-a-real-variant"
        })),
        None,
    )
    .await;
    assert_eq!(rejected.status(), StatusCode::UNPROCESSABLE_ENTITY);

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_history_keeps_single_react_run_segments_in_one_response_variant() {
    let app = build_app();
    let agent_id = "integ_chats_evergreen_react_segments_01";
    create_agent(&app.app, agent_id, "Evergreen ReAct segment agent").await;

    let created = seed_materialized_chat(
        &app,
        "/api/chats",
        Some(agent_id),
        "Evergreen ReAct segment chat",
    )
    .await;
    let chat_id = created["id"].as_str().expect("chat id").to_string();
    let workspace = app
        .state
        .registry
        .get_or_create(agent_id)
        .await
        .expect("workspace runtime")
        .workspace_dir();
    let store = WorkspaceSessionStore::new(&workspace);
    store
        .append_conversation_turn(
            &chat_id,
            NewConversationTurn {
                turn_id: "evergreen-turn-react-segments-1".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-06-19T00:00:00Z".to_string(),
                    assistant_started_at: Some("2026-06-19T00:00:01Z".to_string()),
                    assistant_completed_at: None,
                    status: "in_progress".to_string(),
                },
                selected_variant_id: Some("react-response-1".to_string()),
                messages: vec![
                    NewConversationMessage {
                        message_id: "evergreen-user-react-segments".to_string(),
                        role: "user".to_string(),
                        created_at: "2026-06-19T00:00:00Z".to_string(),
                        status: "completed".to_string(),
                        content: json!([{"type": "text", "text": "inspect project"}]),
                        provider: None,
                        tool: None,
                        metadata: serde_json::Map::new(),
                    },
                    NewConversationMessage {
                        message_id: "evergreen-assistant-react-thinking".to_string(),
                        role: "assistant".to_string(),
                        created_at: "2026-06-19T00:00:01Z".to_string(),
                        status: "completed".to_string(),
                        content: json!([{"type": "thinking", "thinking": "thinking step"}]),
                        provider: None,
                        tool: None,
                        metadata: serde_json::Map::new(),
                    },
                ],
            },
        )
        .await
        .expect("append initial react segment");
    store
        .append_messages_to_turn(
            &chat_id,
            "evergreen-turn-react-segments-1",
            TurnLifecycle {
                user_submitted_at: "2026-06-19T00:00:00Z".to_string(),
                assistant_started_at: Some("2026-06-19T00:00:01Z".to_string()),
                assistant_completed_at: Some("2026-06-19T00:00:03Z".to_string()),
                status: "completed".to_string(),
            },
            Some("ignored-new-assistant-id".to_string()),
            vec![
                NewConversationMessage {
                    message_id: "evergreen-tool-result-react".to_string(),
                    role: "system".to_string(),
                    created_at: "2026-06-19T00:00:02Z".to_string(),
                    status: "completed".to_string(),
                    content: json!([{"type": "tool_result", "id": "call-1", "name": "bash", "output": "ok"}]),
                    provider: None,
                    tool: None,
                    metadata: serde_json::Map::new(),
                },
                NewConversationMessage {
                    message_id: "evergreen-assistant-react-final".to_string(),
                    role: "assistant".to_string(),
                    created_at: "2026-06-19T00:00:03Z".to_string(),
                    status: "completed".to_string(),
                    content: json!([{"type": "text", "text": "final answer"}]),
                    provider: None,
                    tool: None,
                    metadata: serde_json::Map::new(),
                },
            ],
        )
        .await
        .expect("append continuation react segments");

    let turns = store
        .read_turn_range(&chat_id, 1, usize::MAX)
        .await
        .expect("read turns");
    assert_eq!(turns[0].assistant_response_variants.len(), 1);
    assert_eq!(
        turns[0].assistant_response_variants[0].assistant_message_sequences,
        vec![2, 4]
    );
    assert_eq!(turns[0].assistant_response_variants[0].first_sequence, 2);
    assert_eq!(turns[0].assistant_response_variants[0].last_sequence, 4);

    let history = request(
        &app.app,
        Method::GET,
        &format!("/api/chats/{chat_id}"),
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(history.status(), StatusCode::OK);
    let history = read_json(history).await;
    let messages = history["messages"].as_array().expect("messages");
    assert_eq!(messages.len(), 4, "history body: {history:?}");
    let variant_carriers = messages
        .iter()
        .filter(|message| message.get("assistant_response_variants").is_some())
        .count();
    assert_eq!(
        variant_carriers, 0,
        "single-variant responses should not duplicate their selected output into assistant_response_variants: {history:?}"
    );
    assert!(history.to_string().contains("thinking step"));
    assert!(history.to_string().contains("final answer"));

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_history_preserves_fine_grained_message_ids() {
    let app = build_app();
    let agent_id = "integ_chats_evergreen_split_metadata_01";
    create_agent(&app.app, agent_id, "Evergreen split metadata agent").await;

    let created = seed_materialized_chat(
        &app,
        "/api/chats",
        Some(agent_id),
        "Evergreen split metadata chat",
    )
    .await;
    let chat_id = created["id"].as_str().expect("chat id").to_string();
    let workspace = app
        .state
        .registry
        .get_or_create(agent_id)
        .await
        .expect("workspace runtime")
        .workspace_dir();
    WorkspaceSessionStore::new(&workspace)
        .append_conversation_turn(
            &chat_id,
            NewConversationTurn {
                turn_id: "evergreen-turn-split-1".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-06-19T00:00:00Z".to_string(),
                    assistant_started_at: Some("2026-06-19T00:00:01Z".to_string()),
                    assistant_completed_at: Some("2026-06-19T00:00:02Z".to_string()),
                    status: "completed".to_string(),
                },
                selected_variant_id: Some("evergreen-assistant-final".to_string()),
                messages: vec![
                    NewConversationMessage {
                        message_id: "evergreen-assistant-before-tool".to_string(),
                        role: "assistant".to_string(),
                        created_at: "2026-06-19T00:00:01Z".to_string(),
                        status: "completed".to_string(),
                        content: json!([{"type": "text", "text": "before tool"}]),
                        provider: None,
                        tool: None,
                        metadata: serde_json::Map::new(),
                    },
                    NewConversationMessage {
                        message_id: "msg-tc-call-1".to_string(),
                        role: "assistant".to_string(),
                        created_at: "2026-06-19T00:00:01Z".to_string(),
                        status: "completed".to_string(),
                        content: json!([{"type": "tool_use", "id": "call-1", "name": "Shell", "input": {"cmd": "pwd"}}]),
                        provider: None,
                        tool: None,
                        metadata: serde_json::Map::new(),
                    },
                    NewConversationMessage {
                        message_id: "msg-tr-call-1".to_string(),
                        role: "system".to_string(),
                        created_at: "2026-06-19T00:00:02Z".to_string(),
                        status: "completed".to_string(),
                        content: json!([{"type": "tool_result", "id": "call-1", "name": "Shell", "output": [{"type": "text", "text": "1 | alpha\n2 | beta\n"}]}]),
                        provider: None,
                        tool: None,
                        metadata: serde_json::Map::new(),
                    },
                    NewConversationMessage {
                        message_id: "evergreen-assistant-final".to_string(),
                        role: "assistant".to_string(),
                        created_at: "2026-06-19T00:00:02Z".to_string(),
                        status: "completed".to_string(),
                        content: json!([{"type": "text", "text": "after tool"}]),
                        provider: None,
                        tool: None,
                        metadata: serde_json::Map::new(),
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
    let history = read_json(history).await;
    let outputs = history["messages"].as_array().expect("messages");
    assert_eq!(
        outputs
            .iter()
            .map(|message| message["id"].as_str().unwrap_or_default())
            .collect::<Vec<_>>(),
        vec![
            "evergreen-assistant-before-tool",
            "msg-tc-call-1",
            "msg-tr-call-1",
            "evergreen-assistant-final",
        ]
    );
    for output in outputs {
        assert_eq!(output["turn_id"], "evergreen-turn-split-1");
        assert_eq!(output["variant_id"], "evergreen-assistant-final");
        assert_eq!(output["selected_variant_id"], "evergreen-assistant-final");
    }
    let tool_output = outputs
        .iter()
        .find(|output| output["type"] == "plugin_call_output")
        .expect("tool output");
    assert_eq!(
        tool_output["content"][0]["data"]["output"], "1 | alpha\n2 | beta\n",
        "text-only tool_result output should project as text, not JSON with escaped newlines"
    );

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn put_chat_updates_evergreen_selected_variant_without_legacy_session() {
    let app = build_app();
    let agent_id = "integ_chats_evergreen_select_variant_01";
    create_agent(&app.app, agent_id, "Evergreen selected variant agent").await;

    let created = seed_materialized_chat(
        &app,
        "/api/chats",
        Some(agent_id),
        "Evergreen selected variant chat",
    )
    .await;
    let chat_id = created["id"].as_str().expect("chat id").to_string();
    let workspace = app
        .state
        .registry
        .get_or_create(agent_id)
        .await
        .expect("workspace runtime")
        .workspace_dir();
    WorkspaceSessionStore::new(&workspace)
        .append_conversation_turn(
            &chat_id,
            NewConversationTurn {
                turn_id: "evergreen-turn-select-1".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-06-19T00:00:00Z".to_string(),
                    assistant_started_at: Some("2026-06-19T00:00:01Z".to_string()),
                    assistant_completed_at: Some("2026-06-19T00:00:02Z".to_string()),
                    status: "completed".to_string(),
                },
                selected_variant_id: Some("evergreen-assistant-select-1".to_string()),
                messages: vec![
                    NewConversationMessage {
                        message_id: "evergreen-user-select-1".to_string(),
                        role: "user".to_string(),
                        created_at: "2026-06-19T00:00:00Z".to_string(),
                        status: "completed".to_string(),
                        content: json!([{"type": "text", "text": "question"}]),
                        provider: None,
                        tool: None,
                        metadata: serde_json::Map::new(),
                    },
                    NewConversationMessage {
                        message_id: "evergreen-assistant-select-1".to_string(),
                        role: "assistant".to_string(),
                        created_at: "2026-06-19T00:00:01Z".to_string(),
                        status: "completed".to_string(),
                        content: json!([{"type": "text", "text": "selected answer"}]),
                        provider: None,
                        tool: None,
                        metadata: serde_json::Map::new(),
                    },
                ],
            },
        )
        .await
        .expect("append evergreen turn");
    WorkspaceSessionStore::new(&workspace)
        .append_messages_to_turn_as_new_variant(
            &chat_id,
            "evergreen-turn-select-1",
            TurnLifecycle {
                user_submitted_at: "2026-06-19T00:00:00Z".to_string(),
                assistant_started_at: Some("2026-06-19T00:00:01Z".to_string()),
                assistant_completed_at: Some("2026-06-19T00:00:03Z".to_string()),
                status: "completed".to_string(),
            },
            Some("evergreen-assistant-select-2".to_string()),
            vec![NewConversationMessage {
                message_id: "evergreen-assistant-select-2".to_string(),
                role: "assistant".to_string(),
                created_at: "2026-06-19T00:00:03Z".to_string(),
                status: "completed".to_string(),
                content: json!([{"type": "text", "text": "second answer"}]),
                provider: None,
                tool: None,
                metadata: serde_json::Map::new(),
            }],
        )
        .await
        .expect("append evergreen variant");

    let updated = request(
        &app.app,
        Method::PUT,
        &format!("/api/chats/{chat_id}"),
        Some(json!({
            "turn_id": "evergreen-turn-select-1",
            "selected_variant_id": "evergreen-assistant-select-1"
        })),
        Some(agent_id),
    )
    .await;
    assert_eq!(updated.status(), StatusCode::OK);

    let history = request(
        &app.app,
        Method::GET,
        &format!("/api/chats/{chat_id}"),
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(history.status(), StatusCode::OK);
    let history = read_json(history).await;
    let assistant = history["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .find(|message| message["role"] == "assistant")
        .expect("assistant message");
    assert_eq!(
        assistant["selected_variant_id"],
        "evergreen-assistant-select-1"
    );
    assert_eq!(assistant["content"][0]["text"], "selected answer");

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_list_reads_sqlite_index_with_pinning_and_pagination() {
    let app = build_app();
    let agent_id = "integ_chats_sqlite_index_01";
    create_agent(&app.app, agent_id, "Chats sqlite index agent").await;

    let workspace = app
        .state
        .registry
        .get_or_create(agent_id)
        .await
        .expect("workspace runtime")
        .workspace_dir();
    let now = Utc::now();
    let chats = json!({
        "version": 1,
        "chats": [
            {
                "id": "chat-old",
                "name": "Old",
                "user_id": "default",
                "channel": "console",
                "created_at": (now - ChronoDuration::minutes(3)).to_rfc3339(),
                "updated_at": (now - ChronoDuration::minutes(3)).to_rfc3339(),
                "meta": {},
                "status": "idle"
            },
            {
                "id": "chat-new",
                "name": "New",
                "user_id": "default",
                "channel": "console",
                "created_at": (now - ChronoDuration::minutes(2)).to_rfc3339(),
                "updated_at": (now - ChronoDuration::minutes(1)).to_rfc3339(),
                "meta": {},
                "status": "idle"
            },
            {
                "id": "chat-pinned",
                "name": "Pinned",
                "user_id": "default",
                "channel": "console",
                "created_at": now.to_rfc3339(),
                "updated_at": now.to_rfc3339(),
                "meta": {},
                "status": "idle",
                "pinned": true,
                "pin_order": 1
            }
        ]
    });
    seed_chats_from_json(&workspace, &chats).await;

    let page = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{agent_id}/chats?offset=0&limit=1"),
        None,
        None,
    )
    .await;
    assert_eq!(page.status(), StatusCode::OK);
    assert_eq!(
        page.headers()
            .get("X-Total-Count")
            .and_then(|v| v.to_str().ok()),
        Some("3")
    );
    assert_eq!(ids(&read_json(page).await), vec!["chat-pinned", "chat-new"]);

    let page_from_index = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{agent_id}/chats?offset=1&limit=1"),
        None,
        None,
    )
    .await;
    assert_eq!(page_from_index.status(), StatusCode::OK);
    assert_eq!(
        page_from_index
            .headers()
            .get("X-Total-Count")
            .and_then(|v| v.to_str().ok()),
        Some("3")
    );
    assert_eq!(
        ids(&read_json(page_from_index).await),
        vec!["chat-pinned", "chat-old"]
    );

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test]
async fn chat_list_does_not_join_conversation_authority_runtime_state() {
    let app = build_app();
    let agent_id = "integ_chats_index_only_list_01";
    create_agent(&app.app, agent_id, "Index-only chat list agent").await;
    let created = seed_materialized_chat(
        &app,
        &format!("/api/agents/{agent_id}/chats"),
        None,
        "Index only",
    )
    .await;
    let chat_id = created["id"].as_str().expect("chat id");
    app.state
        .conversation_authority
        .start_active_turn(ConversationActiveTurnStart {
            address: rua_core::ChatAddress::agent(agent_id, chat_id),
            driver: ConversationDriverKind::Rua,
            authority_device_id: "device-local".to_string(),
            authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
            driver_detail: None,
        })
        .await;

    let response = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{agent_id}/chats?offset=0&limit=10"),
        None,
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = read_json(response).await;
    let listed = body
        .as_array()
        .expect("chat list")
        .iter()
        .find(|chat| chat["id"] == chat_id)
        .expect("created chat in list");
    assert_eq!(listed["status"], "idle");
    app.state
        .conversation_authority
        .finish_active_turn(&rua_core::ChatAddress::agent(agent_id, chat_id))
        .await;
    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_history_reads_runtime_notes_from_message_log_without_legacy_session() {
    let app = build_app();
    let agent_id = "integ_chats_evergreen_runtime_notes_01";
    create_agent(&app.app, agent_id, "Evergreen runtime note agent").await;

    let created = seed_materialized_chat(
        &app,
        "/api/chats",
        Some(agent_id),
        "Evergreen runtime note chat",
    )
    .await;
    let chat_id = created["id"].as_str().expect("chat id").to_string();
    let workspace = app
        .state
        .registry
        .get_or_create(agent_id)
        .await
        .expect("workspace runtime")
        .workspace_dir();
    let store = WorkspaceSessionStore::new(&workspace);
    store
        .append_conversation_turn(
            &chat_id,
            NewConversationTurn {
                turn_id: "turn-runtime-note-1".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: Utc::now().to_rfc3339(),
                    assistant_started_at: Some(Utc::now().to_rfc3339()),
                    assistant_completed_at: Some(Utc::now().to_rfc3339()),
                    status: "completed".to_string(),
                },
                selected_variant_id: None,
                messages: vec![NewConversationMessage {
                    message_id: "runtime-note-user-1".to_string(),
                    role: "user".to_string(),
                    created_at: Utc::now().to_rfc3339(),
                    status: "completed".to_string(),
                    content: json!([{"type": "text", "text": "Run task"}]),
                    provider: None,
                    tool: None,
                    metadata: serde_json::Map::new(),
                }],
            },
        )
        .await
        .expect("append Runtime Note turn");
    let note = RuntimeNote {
        runtime_note_id: "runtime-note-evergreen-1".to_string(),
        created_at: Utc::now().to_rfc3339(),
        payload: RuntimeNotePayload::TaskAttemptTerminal(TaskAttemptTerminalRuntimeNote {
            task_id: "task-evergreen-1".to_string(),
            attempt_no: 1,
            terminal_status: "completed".to_string(),
            output: "evergreen background task completed".to_string(),
            runner_name: "bash".to_string(),
            tool_call_id: Some("tool-call-evergreen-1".to_string()),
            full_result_ref: None,
            artifacts: json!([]),
            device: json!({}),
        }),
    };
    store
        .append_runtime_note(&chat_id, "turn-runtime-note-1", note)
        .await
        .expect("persist runtime note");

    let history = request(
        &app.app,
        Method::GET,
        &format!("/api/chats/{chat_id}"),
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(history.status(), StatusCode::OK);
    let history = read_json(history).await;
    let note = history["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .find(|message| message["type"] == "runtime_note")
        .expect("Runtime Note message");
    assert_eq!(
        note["runtime_note"]["runtime_note_id"],
        "runtime-note-evergreen-1"
    );
    assert_eq!(note["runtime_note"]["payload"]["runner_name"], "bash");

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deleting_chat_cancels_non_terminal_background_tasks_for_chat_id() {
    let app = build_app();
    let agent_id = "integ_chats_task_delete_01";
    create_agent(&app.app, agent_id, "Chat task delete agent").await;

    let created =
        seed_materialized_chat(&app, "/api/chats", Some(agent_id), "Delete task cleanup").await;
    let chat_id = created["id"].as_str().expect("chat id").to_string();
    let (owner, task_id) =
        create_blocking_task(&app.state, agent_id, &chat_id, "delete cleanup task").await;

    let deleted = request(
        &app.app,
        Method::DELETE,
        &format!("/api/chats/{chat_id}"),
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(deleted.status(), StatusCode::OK);
    assert_eq!(read_json(deleted).await["deleted"], true);

    assert!(matches!(
        app.state.task_runtime.get(&owner, &task_id).await,
        Err(rua_task::TaskError::NotFound(_))
    ));

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deleting_chat_removes_local_session_files() {
    let app = build_app();
    let agent_id = "integ_chats_delete_files_01";
    create_agent(&app.app, agent_id, "Chat file delete agent").await;

    let created =
        seed_materialized_chat(&app, "/api/chats", Some(agent_id), "Delete local files").await;
    let chat_id = created["id"].as_str().expect("chat id").to_string();

    let workspace = app
        .state
        .registry
        .get_or_create(agent_id)
        .await
        .expect("workspace runtime")
        .workspace_dir();
    let store = WorkspaceSessionStore::new(&workspace);
    append_evergreen_turn(
        &app,
        agent_id,
        &chat_id,
        vec![
            stored_message("user-1", "user", "hello", "2026-06-19T00:00:00Z", "user"),
            stored_message(
                "assistant-1",
                "assistant",
                "world",
                "2026-06-19T00:00:01Z",
                "assistant",
            ),
        ],
    )
    .await;
    assert!(store.chat_dir(&chat_id).exists());

    let deleted = request(
        &app.app,
        Method::DELETE,
        &format!("/api/chats/{chat_id}"),
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(deleted.status(), StatusCode::OK);

    assert!(!store.chat_dir(&chat_id).exists());

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rollback_uses_evergreen_turn_index_without_creating_legacy_session() {
    let app = build_app();
    let agent_id = "integ_chats_evergreen_rollback_01";
    create_agent(&app.app, agent_id, "Evergreen rollback agent").await;

    let created =
        seed_materialized_chat(&app, "/api/chats", Some(agent_id), "Evergreen rollback").await;
    let chat_id = created["id"].as_str().expect("chat id").to_string();
    let workspace = app
        .state
        .registry
        .get_or_create(agent_id)
        .await
        .expect("workspace runtime")
        .workspace_dir();

    append_evergreen_turn_with_id(
        &app,
        agent_id,
        &chat_id,
        "evergreen-rollback-turn-1",
        vec![
            stored_message(
                "evergreen-rollback-user-1",
                "user",
                "keep this user",
                "2026-06-19T00:00:00Z",
                "user",
            ),
            stored_message(
                "evergreen-rollback-assistant-1",
                "assistant",
                "keep this assistant",
                "2026-06-19T00:00:01Z",
                agent_id,
            ),
        ],
    )
    .await;
    let store = WorkspaceSessionStore::new(&workspace);
    store
        .append_runtime_note(
            &chat_id,
            "evergreen-rollback-turn-1",
            branch_runtime_note(
                "rollback-note-before-boundary",
                "keep rollback note",
                "2026-06-19T00:00:02Z",
            ),
        )
        .await
        .expect("append Runtime Note before rollback boundary");
    store
        .append_messages_to_turn_as_new_variant(
            &chat_id,
            "evergreen-rollback-turn-1",
            TurnLifecycle {
                user_submitted_at: "2026-06-19T00:00:00Z".to_string(),
                assistant_started_at: Some("2026-06-19T00:00:01Z".to_string()),
                assistant_completed_at: Some("2026-06-19T00:00:03Z".to_string()),
                status: "completed".to_string(),
            },
            Some("evergreen-rollback-assistant-1b".to_string()),
            vec![NewConversationMessage {
                message_id: "evergreen-rollback-assistant-1b".to_string(),
                role: "assistant".to_string(),
                created_at: "2026-06-19T00:00:03Z".to_string(),
                status: "completed".to_string(),
                content: json!([{"type": "text", "text": "selected rollback assistant"}]),
                provider: None,
                tool: None,
                metadata: serde_json::Map::new(),
            }],
        )
        .await
        .expect("append rollback variant");
    store
        .append_runtime_note(
            &chat_id,
            "evergreen-rollback-turn-1",
            branch_runtime_note(
                "rollback-note-after-boundary",
                "discard rollback note",
                "2026-06-19T00:00:04Z",
            ),
        )
        .await
        .expect("append Runtime Note after rollback boundary");
    append_evergreen_turn_with_id(
        &app,
        agent_id,
        &chat_id,
        "evergreen-rollback-turn-2",
        vec![
            stored_message(
                "evergreen-rollback-user-2",
                "user",
                "hide this user",
                "2026-06-19T00:01:00Z",
                "user",
            ),
            stored_message(
                "evergreen-rollback-assistant-2",
                "assistant",
                "hide this assistant",
                "2026-06-19T00:01:01Z",
                agent_id,
            ),
        ],
    )
    .await;
    WorkspaceSessionStore::new(&workspace)
        .append_messages_to_turn(
            &chat_id,
            "evergreen-rollback-turn-2",
            TurnLifecycle {
                user_submitted_at: "2026-06-19T00:01:00Z".to_string(),
                assistant_started_at: Some("2026-06-19T00:01:01Z".to_string()),
                assistant_completed_at: Some("2026-06-19T00:01:02Z".to_string()),
                status: "interrupted".to_string(),
            },
            None,
            vec![NewConversationMessage {
                message_id: "evergreen-rollback-turn-aborted-2".to_string(),
                role: "system".to_string(),
                created_at: "2026-06-19T00:01:02Z".to_string(),
                status: "completed".to_string(),
                content: json!([{"type": "text", "text": TURN_ABORTED_MARKER_TEXT}]),
                provider: None,
                tool: None,
                metadata: serde_json::Map::new(),
            }],
        )
        .await
        .expect("append rollback interruption marker");

    let (task_owner, task_id) = create_blocking_task(
        &app.state,
        agent_id,
        &chat_id,
        "cancel after rollback branch point",
    )
    .await;

    let updated = request(
        &app.app,
        Method::POST,
        &format!("/api/chats/{chat_id}/rollback"),
        Some(json!({"before_turn_id": "evergreen-rollback-turn-2"})),
        Some(agent_id),
    )
    .await;
    assert_eq!(updated.status(), StatusCode::OK);
    let replay = app
        .state
        .active_stream_broadcast_hub
        .subscribe_chat(
            &rua_core::ChatAddress::agent(agent_id, &chat_id),
            &Default::default(),
        )
        .await
        .replay;
    assert!(
        replay.iter().any(|frame| {
            let rua_server::services::active_stream_broadcast::ActiveStreamBroadcastFrame::Event {
                payload,
                ..
            } = frame
            else {
                return false;
            };
            let value: serde_json::Value = serde_json::from_str(payload).unwrap();
            value["type"] == "conversation_history_mutated"
                && value["action"] == "rolled_back"
                && value["before_turn_id"] == "evergreen-rollback-turn-2"
        }),
        "rollback must be included in Live Activity replay"
    );
    assert_eq!(
        app.state
            .task_runtime
            .get(&task_owner, &task_id)
            .await
            .expect("rollback Task")
            .state,
        rua_task::TaskState::Cancelled
    );

    let store = WorkspaceSessionStore::new(&workspace);
    let turns = store
        .read_turn_range(&chat_id, 1, usize::MAX)
        .await
        .expect("read turn index");
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].turn_id, "evergreen-rollback-turn-1");
    assert_eq!(
        turns[0].selected_variant_id.as_deref(),
        Some("evergreen-rollback-assistant-1b")
    );
    assert_eq!(turns[0].assistant_response_sequences, vec![4]);

    let history = request(
        &app.app,
        Method::GET,
        &format!("/api/chats/{chat_id}"),
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(history.status(), StatusCode::OK);
    let history = read_json(history).await;
    let messages = history["messages"].as_array().expect("messages");
    assert_eq!(messages.len(), 3, "history body: {history:?}");
    assert_eq!(messages[0]["id"], "evergreen-rollback-user-1");
    assert_eq!(messages[1]["id"], "rollback-note-before-boundary");
    assert_eq!(messages[2]["id"], "evergreen-rollback-assistant-1b");
    assert_eq!(
        messages[2]["content"][0]["text"],
        "selected rollback assistant"
    );
    assert!(history.to_string().contains("keep rollback note"));
    assert!(!history.to_string().contains("discard rollback note"));
    assert!(!history.to_string().contains("keep this assistant"));
    assert!(
        !history.to_string().contains("<turn_aborted>"),
        "rollback projection must exclude interruption markers from rolled-back turns"
    );

    let raw_messages = store
        .read_message_range(&chat_id, 1, 8)
        .await
        .expect("read raw message log");
    assert_eq!(
        raw_messages.len(),
        8,
        "rollback must preserve raw Message Log entries outside the active projection"
    );

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rollback_and_branch_reject_while_acp_binding_is_active() {
    let app = build_app();
    let agent_id = "integ_chats_active_acp_branch_lock_01";
    create_agent(&app.app, agent_id, "Active ACP branch lock agent").await;
    let created =
        seed_materialized_chat(&app, "/api/chats", Some(agent_id), "Active ACP branch lock").await;
    let chat_id = created["id"].as_str().expect("chat id").to_string();
    let authority_device_id = created["meta"]["authority_placement"]["device_id"]
        .as_str()
        .expect("authority Device")
        .to_string();
    append_test_turn(
        &app,
        agent_id,
        &chat_id,
        "active-acp-lock-turn-1",
        "question",
        "answer",
        None,
    )
    .await;

    let workspace = app
        .state
        .registry
        .get_or_create(agent_id)
        .await
        .expect("workspace runtime")
        .workspace_dir();
    persist_acp_binding(
        Some(&app.state),
        &workspace,
        &rua_core::ChatAddress::agent(agent_id, &chat_id),
        agent_id,
        AcpBindingMetadata {
            target_device_id: Some(authority_device_id),
            runner: "codex".to_string(),
            cwd: Some("/tmp".to_string()),
            runner_session_id: Some("runner-session".to_string()),
        },
    )
    .await
    .expect("persist active acp binding");

    for action in ["rollback", "branch"] {
        let response = request(
            &app.app,
            Method::POST,
            &format!("/api/chats/{chat_id}/{action}"),
            Some(json!({"before_turn_id": "active-acp-lock-turn-1"})),
            Some(agent_id),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let body = read_json(response).await;
        assert!(
            body["detail"]
                .as_str()
                .is_some_and(|detail| detail.contains("ACP mode")),
            "{action} should explain the active ACP lock: {body:?}"
        );
    }

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn branch_rejects_interior_acp_segment_turn_but_accepts_segment_tail() {
    let app = build_app();
    let agent_id = "integ_chats_acp_segment_branch_01";
    create_agent(&app.app, agent_id, "ACP segment branch agent").await;
    let created = seed_materialized_chat(
        &app,
        "/api/chats",
        Some(agent_id),
        "ACP segment branch source",
    )
    .await;
    let chat_id = created["id"].as_str().expect("chat id").to_string();

    append_test_turn(
        &app,
        agent_id,
        &chat_id,
        "segment-turn-a",
        "A",
        "A answer",
        None,
    )
    .await;
    append_test_turn(
        &app,
        agent_id,
        &chat_id,
        "segment-turn-c",
        "C",
        "C ACP answer",
        Some("codex"),
    )
    .await;
    append_test_turn(
        &app,
        agent_id,
        &chat_id,
        "segment-turn-d",
        "D",
        "D ACP answer",
        Some("codex"),
    )
    .await;
    append_test_turn(
        &app,
        agent_id,
        &chat_id,
        "segment-turn-f",
        "F",
        "F answer",
        None,
    )
    .await;

    let interior = request(
        &app.app,
        Method::POST,
        &format!("/api/chats/{chat_id}/branch"),
        Some(json!({"before_turn_id": "segment-turn-d"})),
        Some(agent_id),
    )
    .await;
    assert_eq!(interior.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let interior_body = read_json(interior).await;
    assert!(
        interior_body["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("ACP segment")),
        "interior ACP branch point should explain segment rule: {interior_body:?}"
    );

    let interior_rollback = request(
        &app.app,
        Method::POST,
        &format!("/api/chats/{chat_id}/rollback"),
        Some(json!({"before_turn_id": "segment-turn-d"})),
        Some(agent_id),
    )
    .await;
    assert_eq!(interior_rollback.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let interior_rollback_body = read_json(interior_rollback).await;
    assert!(
        interior_rollback_body["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("ACP segment")),
        "interior ACP rollback point should explain segment rule: {interior_rollback_body:?}"
    );

    let tail = request(
        &app.app,
        Method::POST,
        &format!("/api/chats/{chat_id}/branch"),
        Some(json!({"before_turn_id": "segment-turn-f"})),
        Some(agent_id),
    )
    .await;
    assert_eq!(tail.status(), StatusCode::OK);
    let tail_body = read_json(tail).await;
    let branch_chat_id = tail_body["chat"]["id"].as_str().expect("branch id");
    let branch_history = request(
        &app.app,
        Method::GET,
        &format!("/api/chats/{branch_chat_id}"),
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(branch_history.status(), StatusCode::OK);
    let branch_history = read_json(branch_history).await;
    assert!(branch_history.to_string().contains("D ACP answer"));
    assert!(!branch_history.to_string().contains("F answer"));

    delete_agent(&app.app, agent_id).await;
}

struct BranchBlockingUnifiedTaskRunner {
    started: tokio::sync::Notify,
}

#[async_trait::async_trait]
impl rua_task::TaskRunner for BranchBlockingUnifiedTaskRunner {
    fn kind(&self) -> rua_task::TaskRunnerKind {
        rua_task::TaskRunnerKind::Tool
    }

    async fn run(
        &self,
        request: rua_task::TaskRunRequest,
    ) -> Result<rua_task::TaskRunOutput, rua_task::TaskRunnerError> {
        self.started.notify_one();
        request.cancel.cancelled().await;
        Ok(rua_task::TaskRunOutput::cancelled())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn branch_copies_evergreen_selected_prefix_to_new_chat_directory() {
    let app = build_app();
    let agent_id = "integ_chats_evergreen_branch_01";
    create_agent(&app.app, agent_id, "Evergreen branch agent").await;

    let created = seed_materialized_chat(
        &app,
        "/api/chats",
        Some(agent_id),
        "Evergreen branch source",
    )
    .await;
    let chat_id = created["id"].as_str().expect("chat id").to_string();
    let workspace = app
        .state
        .registry
        .get_or_create(agent_id)
        .await
        .expect("workspace runtime")
        .workspace_dir();
    let task_runner = Arc::new(BranchBlockingUnifiedTaskRunner {
        started: tokio::sync::Notify::new(),
    });
    app.state.task_runtime.register_runner(task_runner.clone());
    let source_task_owner = rua_task::TaskOwner::new(agent_id, &chat_id);
    let source_task_id = app
        .state
        .task_runtime
        .create(
            rua_task::TaskSpec {
                owner: source_task_owner.clone(),
                instruction: "remain with source Chat".to_string(),
                runner: rua_task::TaskRunnerSpec::Tool {
                    tool_name: "Fixture_BranchLive".to_string(),
                    arguments: json!({}),
                },
                context_refs: Vec::new(),
                depends_on: Vec::new(),
                origin: Default::default(),
            },
            rua_task::TaskAttemptAuthorization::new(agent_id).unwrap(),
        )
        .await
        .expect("create source live Task");
    tokio::time::timeout(Duration::from_secs(2), task_runner.started.notified())
        .await
        .expect("source Task starts");

    append_evergreen_turn_with_id(
        &app,
        agent_id,
        &chat_id,
        "evergreen-branch-turn-1",
        vec![
            stored_message(
                "evergreen-branch-user-1",
                "user",
                "copy this user",
                "2026-06-19T00:00:00Z",
                "user",
            ),
            stored_message(
                "evergreen-branch-assistant-1",
                "assistant",
                "copy this assistant",
                "2026-06-19T00:00:01Z",
                agent_id,
            ),
        ],
    )
    .await;
    WorkspaceSessionStore::new(&workspace)
        .append_messages_to_turn_as_new_variant(
            &chat_id,
            "evergreen-branch-turn-1",
            TurnLifecycle {
                user_submitted_at: "2026-06-19T00:00:00Z".to_string(),
                assistant_started_at: Some("2026-06-19T00:00:01Z".to_string()),
                assistant_completed_at: Some("2026-06-19T00:00:03Z".to_string()),
                status: "completed".to_string(),
            },
            Some("evergreen-branch-assistant-1b".to_string()),
            vec![NewConversationMessage {
                message_id: "evergreen-branch-assistant-1b".to_string(),
                role: "assistant".to_string(),
                created_at: "2026-06-19T00:00:03Z".to_string(),
                status: "completed".to_string(),
                content: json!([{"type": "text", "text": "selected branch assistant"}]),
                provider: None,
                tool: None,
                metadata: serde_json::Map::new(),
            }],
        )
        .await
        .expect("append branch variant");
    let store = WorkspaceSessionStore::new(&workspace);
    let mut source_header = store.load_chat_header(&chat_id).await.unwrap().unwrap();
    let previous_epoch = source_header.extra["hook_configuration"]["epoch"]
        .as_str()
        .unwrap()
        .to_string();
    source_header.extra["hook_configuration"]["running_config"]["max_iters"] = json!(27);
    source_header.extra["hook_configuration"]["epoch"] = json!("branch-fixture-config");
    store
        .replace_chat_configuration(
            &chat_id,
            &previous_epoch,
            source_header.extra["hook_configuration"].clone(),
        )
        .await
        .unwrap();
    store.save_chat_header(&source_header).await.unwrap();
    let checkpoint_state = store
        .checkpoint_projection_state(&chat_id, 3)
        .await
        .expect("source checkpoint state");
    store
        .publish_summary_checkpoint(
            &chat_id,
            None,
            SummaryCheckpointDraft {
                checkpoint_id: "evergreen-branch-checkpoint".to_string(),
                previous_checkpoint_id: None,
                covered_turn_range: CheckpointTurnRange {
                    start_turn_ordinal: 1,
                    end_turn_ordinal: 1,
                },
                covered_sequence_range: CheckpointSequenceRange {
                    start_sequence: 1,
                    end_sequence: 3,
                },
                source_sequence_range: CheckpointSequenceRange {
                    start_sequence: 1,
                    end_sequence: 3,
                },
                frontier: checkpoint_state.frontier,
                retained_sequence_ranges: checkpoint_state.retained_sequence_ranges,
                projection_lineage: checkpoint_state.projection_lineage,
                summary: "checkpoint inherited by compatible branch".to_string(),
                token_estimate: 12,
                model: CheckpointModelIdentity {
                    provider: "test".to_string(),
                    model: "branch-test".to_string(),
                },
                created_at: "2026-06-19T00:00:04Z".to_string(),
            },
        )
        .await
        .expect("publish source checkpoint");
    append_evergreen_turn_with_id(
        &app,
        agent_id,
        &chat_id,
        "evergreen-branch-turn-2",
        vec![
            stored_message(
                "evergreen-branch-user-2",
                "user",
                "source only user",
                "2026-06-19T00:01:00Z",
                "user",
            ),
            stored_message(
                "evergreen-branch-assistant-2",
                "assistant",
                "source only assistant",
                "2026-06-19T00:01:01Z",
                agent_id,
            ),
        ],
    )
    .await;
    WorkspaceSessionStore::new(&workspace)
        .append_messages_to_turn(
            &chat_id,
            "evergreen-branch-turn-2",
            TurnLifecycle {
                user_submitted_at: "2026-06-19T00:01:00Z".to_string(),
                assistant_started_at: Some("2026-06-19T00:01:01Z".to_string()),
                assistant_completed_at: Some("2026-06-19T00:01:02Z".to_string()),
                status: "interrupted".to_string(),
            },
            None,
            vec![NewConversationMessage {
                message_id: "evergreen-branch-turn-aborted-2".to_string(),
                role: "system".to_string(),
                created_at: "2026-06-19T00:01:02Z".to_string(),
                status: "completed".to_string(),
                content: json!([{"type": "text", "text": TURN_ABORTED_MARKER_TEXT}]),
                provider: None,
                tool: None,
                metadata: serde_json::Map::new(),
            }],
        )
        .await
        .expect("append branch interruption marker");

    let branched = request(
        &app.app,
        Method::POST,
        &format!("/api/chats/{chat_id}/branch"),
        Some(json!({"before_turn_id": "evergreen-branch-turn-2"})),
        Some(agent_id),
    )
    .await;
    assert_eq!(branched.status(), StatusCode::OK);
    let branched = read_json(branched).await;
    let branch_chat_id = branched["chat"]["id"].as_str().expect("branch id");
    assert_ne!(branch_chat_id, chat_id);
    assert!(!app
        .state
        .task_runtime
        .get(&source_task_owner, &source_task_id)
        .await
        .expect("source Task remains")
        .state
        .is_terminal());
    let branch_tasks = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{agent_id}/chats/{branch_chat_id}/tasks"),
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(branch_tasks.status(), StatusCode::OK);
    assert!(read_json(branch_tasks)
        .await
        .as_array()
        .expect("branch Task list")
        .is_empty());

    let branch_header = store
        .load_chat_header(branch_chat_id)
        .await
        .expect("load branch header")
        .expect("branch should be a directory-backed Evergreen chat");
    assert_eq!(
        store
            .load_chat_running_config(&chat_id)
            .await
            .unwrap()
            .unwrap()
            .max_iters,
        27
    );
    assert_eq!(
        store
            .load_chat_running_config(branch_chat_id)
            .await
            .unwrap()
            .unwrap()
            .max_iters,
        27
    );
    assert_eq!(
        branch_header.metadata.title,
        "Evergreen branch source (branch)"
    );
    let branch_turns = store
        .read_turn_range(branch_chat_id, 1, usize::MAX)
        .await
        .expect("read branch turns");
    assert_eq!(branch_turns.len(), 1);
    assert_eq!(branch_turns[0].turn_id, "evergreen-branch-turn-1");
    assert_eq!(
        branch_turns[0].selected_variant_id.as_deref(),
        Some("evergreen-branch-assistant-1b")
    );
    assert_eq!(branch_turns[0].assistant_response_sequences, vec![2]);
    let branch_checkpoint = store
        .load_active_summary_checkpoint(branch_chat_id)
        .await
        .expect("load branch checkpoint")
        .expect("branch should inherit a compatible checkpoint");
    assert_eq!(
        branch_checkpoint.summary,
        "checkpoint inherited by compatible branch"
    );
    assert_eq!(branch_checkpoint.frontier.end_sequence, 2);

    let branch_history = request(
        &app.app,
        Method::GET,
        &format!("/api/chats/{branch_chat_id}"),
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(branch_history.status(), StatusCode::OK);
    let branch_history = read_json(branch_history).await;
    let messages = branch_history["messages"].as_array().expect("messages");
    assert_eq!(messages.len(), 2, "branch history: {branch_history:?}");
    assert_eq!(messages[0]["id"], "evergreen-branch-user-1");
    assert_eq!(messages[1]["id"], "evergreen-branch-assistant-1b");
    assert_eq!(
        messages[1]["content"][0]["text"],
        "selected branch assistant"
    );
    assert!(!branch_history.to_string().contains("copy this assistant"));
    assert!(
        !branch_history.to_string().contains("<turn_aborted>"),
        "branch projection must not inherit interruption markers after the branch point"
    );

    let source_history = request(
        &app.app,
        Method::GET,
        &format!("/api/chats/{chat_id}"),
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(source_history.status(), StatusCode::OK);
    let source_history = read_json(source_history).await;
    assert_eq!(
        source_history["messages"]
            .as_array()
            .expect("source messages")
            .len(),
        5
    );
    assert!(
        source_history.to_string().contains("<turn_aborted>"),
        "source chat keeps its own interruption marker"
    );

    app.state
        .task_runtime
        .cancel(&source_task_owner, &source_task_id)
        .await
        .expect("clean up source live Task");
    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn branch_copies_runtime_notes_in_chronology_through_selected_response_boundary() {
    let app = build_app();
    let agent_id = "integ_chats_runtime_note_branch_01";
    create_agent(&app.app, agent_id, "Runtime Note branch agent").await;

    let created = seed_materialized_chat(
        &app,
        "/api/chats",
        Some(agent_id),
        "Runtime Note branch source",
    )
    .await;
    let chat_id = created["id"].as_str().expect("chat id").to_string();
    let workspace = app
        .state
        .registry
        .get_or_create(agent_id)
        .await
        .expect("workspace runtime")
        .workspace_dir();
    let store = WorkspaceSessionStore::new(&workspace);

    append_evergreen_turn_with_id(
        &app,
        agent_id,
        &chat_id,
        "runtime-note-branch-turn-1",
        vec![
            stored_message(
                "runtime-note-branch-user-1",
                "user",
                "first request",
                "2026-06-19T00:00:00Z",
                "user",
            ),
            stored_message(
                "runtime-note-branch-assistant-1",
                "assistant",
                "first response",
                "2026-06-19T00:00:01Z",
                agent_id,
            ),
        ],
    )
    .await;
    store
        .append_runtime_note(
            &chat_id,
            "runtime-note-branch-turn-1",
            branch_runtime_note(
                "runtime-note-prior-turn-after-response",
                "retain prior turn note",
                "2026-06-19T00:00:02Z",
            ),
        )
        .await
        .expect("append prior Turn Runtime Note");

    append_evergreen_turn_with_id(
        &app,
        agent_id,
        &chat_id,
        "runtime-note-branch-turn-2",
        vec![
            stored_message(
                "runtime-note-branch-user-2",
                "user",
                "second request",
                "2026-06-19T00:01:00Z",
                "user",
            ),
            stored_message(
                "runtime-note-branch-assistant-2",
                "assistant",
                "unselected response",
                "2026-06-19T00:01:01Z",
                agent_id,
            ),
        ],
    )
    .await;
    store
        .append_runtime_note(
            &chat_id,
            "runtime-note-branch-turn-2",
            branch_runtime_note(
                "runtime-note-branch-before-response",
                "retain branch point note",
                "2026-06-19T00:01:02Z",
            ),
        )
        .await
        .expect("append branch-point Runtime Note before selected response");
    store
        .append_messages_to_turn_as_new_variant(
            &chat_id,
            "runtime-note-branch-turn-2",
            TurnLifecycle {
                user_submitted_at: "2026-06-19T00:01:00Z".to_string(),
                assistant_started_at: Some("2026-06-19T00:01:03Z".to_string()),
                assistant_completed_at: Some("2026-06-19T00:01:04Z".to_string()),
                status: "completed".to_string(),
            },
            Some("runtime-note-branch-assistant-2b".to_string()),
            vec![NewConversationMessage {
                message_id: "runtime-note-branch-assistant-2b".to_string(),
                role: "assistant".to_string(),
                created_at: "2026-06-19T00:01:04Z".to_string(),
                status: "completed".to_string(),
                content: json!([{"type": "text", "text": "selected response"}]),
                provider: None,
                tool: None,
                metadata: serde_json::Map::new(),
            }],
        )
        .await
        .expect("append selected branch variant");
    store
        .append_runtime_note(
            &chat_id,
            "runtime-note-branch-turn-2",
            branch_runtime_note(
                "runtime-note-branch-after-response",
                "discard branch point note",
                "2026-06-19T00:01:05Z",
            ),
        )
        .await
        .expect("append branch-point Runtime Note after selected response");
    let source_turns = store
        .read_turn_range(&chat_id, 1, usize::MAX)
        .await
        .expect("read source Turn Index");
    assert_eq!(source_turns[0].last_sequence, 3);
    assert_eq!(
        store
            .read_message_range(&chat_id, 1, 3)
            .await
            .expect("read first source Turn")
            .into_iter()
            .map(|message| message.message_id)
            .collect::<Vec<_>>(),
        vec![
            "runtime-note-branch-user-1",
            "runtime-note-branch-assistant-1",
            "runtime-note-prior-turn-after-response",
        ]
    );
    append_evergreen_turn_with_id(
        &app,
        agent_id,
        &chat_id,
        "runtime-note-branch-turn-3",
        vec![
            stored_message(
                "runtime-note-branch-user-3",
                "user",
                "branch from this input",
                "2026-06-19T00:02:00Z",
                "user",
            ),
            stored_message(
                "runtime-note-branch-assistant-3",
                "assistant",
                "exclude this response",
                "2026-06-19T00:02:01Z",
                agent_id,
            ),
        ],
    )
    .await;

    let branched = request(
        &app.app,
        Method::POST,
        &format!("/api/chats/{chat_id}/branch"),
        Some(json!({"before_turn_id": "runtime-note-branch-turn-3"})),
        Some(agent_id),
    )
    .await;
    assert_eq!(branched.status(), StatusCode::OK);
    let branch_chat_id = read_json(branched).await["chat"]["id"]
        .as_str()
        .expect("branch id")
        .to_string();
    let raw_branch_message_ids = store
        .read_message_range(&branch_chat_id, 1, 20)
        .await
        .expect("read raw branch Message Log")
        .into_iter()
        .map(|message| message.message_id)
        .collect::<Vec<_>>();
    assert_eq!(
        raw_branch_message_ids,
        vec![
            "runtime-note-branch-user-1",
            "runtime-note-branch-assistant-1",
            "runtime-note-prior-turn-after-response",
            "runtime-note-branch-user-2",
            "runtime-note-branch-before-response",
            "runtime-note-branch-assistant-2b",
        ]
    );

    let branch_history = request(
        &app.app,
        Method::GET,
        &format!("/api/chats/{branch_chat_id}"),
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(branch_history.status(), StatusCode::OK);
    let branch_history = read_json(branch_history).await;
    let message_ids = branch_history["messages"]
        .as_array()
        .expect("branch messages")
        .iter()
        .map(|message| message["id"].as_str().expect("message id"))
        .collect::<Vec<_>>();
    assert_eq!(
        message_ids,
        vec![
            "runtime-note-branch-user-1",
            "runtime-note-branch-assistant-1",
            "runtime-note-prior-turn-after-response",
            "runtime-note-branch-user-2",
            "runtime-note-branch-before-response",
            "runtime-note-branch-assistant-2b",
        ]
    );
    assert!(branch_history
        .to_string()
        .contains("retain prior turn note"));
    assert!(branch_history
        .to_string()
        .contains("retain branch point note"));
    assert!(!branch_history
        .to_string()
        .contains("discard branch point note"));
    assert!(!branch_history.to_string().contains("unselected response"));

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn put_chat_accepts_partial_rename_but_rejects_read_only_fields() {
    let app = build_app();
    let agent_id = "integ_chat_update_contract_01";
    create_agent(&app.app, agent_id, "Chat update contract agent").await;

    let created = seed_materialized_chat(&app, "/api/chats", Some(agent_id), "Old Chat").await;
    let chat_id = created["id"].as_str().expect("chat id").to_string();
    let user_id = created["user_id"].as_str().expect("user id").to_string();
    let channel = created["channel"].as_str().expect("channel").to_string();

    let renamed = request(
        &app.app,
        Method::PUT,
        &format!("/api/chats/{chat_id}"),
        Some(json!({"name": "Renamed Chat"})),
        Some(agent_id),
    )
    .await;
    assert_eq!(renamed.status(), StatusCode::OK);
    let renamed_body = read_json(renamed).await;
    assert_eq!(renamed_body["name"], "Renamed Chat");
    assert!(renamed_body.get("session_id").is_none());
    assert_eq!(renamed_body["user_id"], user_id);
    assert_eq!(renamed_body["channel"], channel);

    for payload in [
        json!({"id": "other"}),
        json!({"user_id": "other-user"}),
        json!({"channel": "other-channel"}),
        json!({"created_at": "2026-01-01T00:00:00Z"}),
        json!({"updated_at": "2026-01-01T00:00:00Z"}),
        json!({"meta": {"source": "client-forged"}}),
        json!({"status": "running"}),
    ] {
        let rejected = request(
            &app.app,
            Method::PUT,
            &format!("/api/chats/{chat_id}"),
            Some(payload),
            Some(agent_id),
        )
        .await;
        assert_eq!(rejected.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert!(read_json(rejected).await["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("read-only"));
    }

    let after = request(&app.app, Method::GET, "/api/chats", None, Some(agent_id)).await;
    assert_eq!(after.status(), StatusCode::OK);
    let row = read_json(after)
        .await
        .as_array()
        .expect("chats array")
        .iter()
        .find(|row| row["id"] == chat_id)
        .expect("chat still exists")
        .clone();
    assert_eq!(row["name"], "Renamed Chat");
    assert!(row.get("session_id").is_none());

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn empty_put_touches_chat_without_overwriting_renamed_title() {
    let app = build_app();
    let agent_id = "integ_chat_touch_contract_01";
    create_agent(&app.app, agent_id, "Chat touch contract agent").await;

    let workspace = app
        .state
        .registry
        .get_or_create(agent_id)
        .await
        .expect("workspace runtime")
        .workspace_dir();
    let stale_updated_at = Utc::now() - ChronoDuration::days(1);
    seed_chats_from_json(
        &workspace,
        &json!({
            "version": 1,
            "chats": [
                {
                    "id": "touch-chat",
                    "name": "User Renamed Chat",
                    "user_id": "user-touch",
                    "channel": "console",
                    "created_at": stale_updated_at,
                    "updated_at": stale_updated_at,
                    "meta": {"source": "fixture"},
                    "status": "idle",
                    "pinned": false,
                    "pin_order": 0
                }
            ]
        }),
    )
    .await;

    let touched = request(
        &app.app,
        Method::PUT,
        "/api/chats/touch-chat",
        Some(json!({})),
        Some(agent_id),
    )
    .await;
    assert_eq!(touched.status(), StatusCode::OK);
    let touched_body = read_json(touched).await;
    assert_eq!(touched_body["name"], "User Renamed Chat");
    assert!(touched_body.get("session_id").is_none());
    assert_eq!(touched_body["user_id"], "user-touch");
    assert_eq!(touched_body["channel"], "console");
    assert_eq!(touched_body["meta"], json!({"source": "fixture"}));
    let updated_at = chrono::DateTime::parse_from_rfc3339(
        touched_body["updated_at"]
            .as_str()
            .expect("updated_at string"),
    )
    .expect("updated_at timestamp")
    .with_timezone(&Utc);
    assert!(updated_at > stale_updated_at);

    let list = request(&app.app, Method::GET, "/api/chats", None, Some(agent_id)).await;
    assert_eq!(list.status(), StatusCode::OK);
    let persisted = read_json(list)
        .await
        .as_array()
        .expect("chats array")
        .iter()
        .find(|row| row["id"] == "touch-chat")
        .expect("chat still exists")
        .clone();
    assert_eq!(persisted["name"], "User Renamed Chat");
    let persisted_updated_at = chrono::DateTime::parse_from_rfc3339(
        persisted["updated_at"]
            .as_str()
            .expect("persisted updated_at string"),
    )
    .expect("persisted updated_at timestamp")
    .with_timezone(&Utc);
    assert!(persisted_updated_at > stale_updated_at);

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_history_title_falls_back_to_index_when_header_is_placeholder_id() {
    let app = build_app();
    let agent_id = "integ_chat_history_title_fallback";
    create_agent(&app.app, agent_id, "Chat history title fallback agent").await;

    let created =
        seed_materialized_chat(&app, "/api/chats", Some(agent_id), "Synced Chat Title").await;
    let chat_id = created["id"].as_str().expect("chat id").to_string();
    let workspace = app
        .state
        .registry
        .get_or_create(agent_id)
        .await
        .expect("workspace runtime")
        .workspace_dir();
    let store = WorkspaceSessionStore::new(&workspace);
    let mut header = store
        .load_chat_header(&chat_id)
        .await
        .expect("load chat header")
        .expect("chat header");
    header.metadata.title = chat_id.clone();
    store
        .save_chat_header(&header)
        .await
        .expect("save placeholder title header");

    let history = request(
        &app.app,
        Method::GET,
        &format!("/api/chats/{chat_id}"),
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(history.status(), StatusCode::OK);
    let history_body = read_json(history).await;
    assert_eq!(history_body["title"], "Synced Chat Title");

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn root_batch_delete_returns_qwenpaw_deleted_flag() {
    let app = build_app();
    let agent_id = "integ_chats_batch_01";
    create_agent(&app.app, agent_id, "Chats batch smoke agent").await;

    let chat_a = seed_materialized_chat(&app, "/api/chats", Some(agent_id), "Batch chat 1").await;
    let chat_b = seed_materialized_chat(&app, "/api/chats", Some(agent_id), "Batch chat 2").await;
    let chat_ids = vec![
        chat_a["id"].as_str().expect("chat a id").to_string(),
        chat_b["id"].as_str().expect("chat b id").to_string(),
    ];

    let before = request(&app.app, Method::GET, "/api/chats", None, Some(agent_id)).await;
    assert_eq!(before.status(), StatusCode::OK);
    let before_ids = ids(&read_json(before).await);
    assert!(chat_ids.iter().all(|id| before_ids.contains(id)));

    let batch = request(
        &app.app,
        Method::POST,
        "/api/chats/batch-delete",
        Some(json!(chat_ids)),
        Some(agent_id),
    )
    .await;
    assert_eq!(batch.status(), StatusCode::OK);
    let body = read_json(batch).await;
    assert_eq!(body["deleted"], true);
    assert_eq!(body["success"], true);
    assert_eq!(body["deleted_count"], 2);

    let after = request(&app.app, Method::GET, "/api/chats", None, Some(agent_id)).await;
    assert_eq!(after.status(), StatusCode::OK);
    let after_ids = ids(&read_json(after).await);
    assert!(chat_ids.iter().all(|id| !after_ids.contains(id)));

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_scoped_create_list_batch_delete_match_qwenpaw_contract() {
    let app = build_app();
    let agent_id = "integ_scoped_chats_batch_01";
    create_agent(&app.app, agent_id, "Scoped chats batch agent").await;
    let base = format!("/api/agents/{agent_id}");

    let chat_a =
        seed_materialized_chat(&app, &format!("{base}/chats"), None, "Scoped batch chat 1").await;
    let chat_b =
        seed_materialized_chat(&app, &format!("{base}/chats"), None, "Scoped batch chat 2").await;
    let chat_ids = vec![
        chat_a["id"].as_str().expect("chat a id").to_string(),
        chat_b["id"].as_str().expect("chat b id").to_string(),
    ];

    let before = request(&app.app, Method::GET, &format!("{base}/chats"), None, None).await;
    assert_eq!(before.status(), StatusCode::OK);
    let before_ids = ids(&read_json(before).await);
    assert!(chat_ids.iter().all(|id| before_ids.contains(id)));

    let batch = request(
        &app.app,
        Method::POST,
        &format!("{base}/chats/batch-delete"),
        Some(json!(chat_ids)),
        None,
    )
    .await;
    assert_eq!(batch.status(), StatusCode::OK);
    let body = read_json(batch).await;
    assert_eq!(body["deleted"], true);
    assert_eq!(body["success"], true);
    assert_eq!(body["deleted_count"], 2);

    let after = request(&app.app, Method::GET, &format!("{base}/chats"), None, None).await;
    assert_eq!(after.status(), StatusCode::OK);
    let after_ids = ids(&read_json(after).await);
    assert!(chat_ids.iter().all(|id| !after_ids.contains(id)));

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_scoped_put_get_delete_repeat_delete_404() {
    let app = build_app();
    let agent_id = "integ_scoped_chat_crud_01";
    create_agent(&app.app, agent_id, "Scoped single chat agent").await;
    let base = format!("/api/agents/{agent_id}");

    let created =
        seed_materialized_chat(&app, &format!("{base}/chats"), None, "Scoped chat original").await;
    let chat_id = created["id"].as_str().expect("chat id").to_string();

    let renamed = request(
        &app.app,
        Method::PUT,
        &format!("{base}/chats/{chat_id}"),
        Some(json!({ "name": "Scoped chat renamed", "tag": "yellow" })),
        None,
    )
    .await;
    assert_eq!(renamed.status(), StatusCode::OK);
    let renamed = read_json(renamed).await;
    assert_eq!(renamed["name"], "Scoped chat renamed");
    assert_eq!(renamed["tag"], "yellow");

    let history = request(
        &app.app,
        Method::GET,
        &format!("{base}/chats/{chat_id}"),
        None,
        None,
    )
    .await;
    assert_eq!(history.status(), StatusCode::OK);
    let history_body = read_json(history).await;
    assert!(history_body["messages"].is_array());
    assert!(history_body.get("status").is_none());
    assert!(history_body.get("active_turn").is_none());
    assert!(history_body.get("control").is_none());

    let list = request(&app.app, Method::GET, &format!("{base}/chats"), None, None).await;
    assert_eq!(list.status(), StatusCode::OK);
    let chats_by_id: std::collections::HashMap<_, _> = read_json(list)
        .await
        .as_array()
        .expect("chats array")
        .iter()
        .map(|row| {
            (
                row["id"].as_str().expect("chat id").to_string(),
                (
                    row["name"].as_str().expect("chat name").to_string(),
                    row["tag"].as_str().expect("chat tag").to_string(),
                ),
            )
        })
        .collect();
    assert_eq!(
        chats_by_id.get(&chat_id),
        Some(&("Scoped chat renamed".to_string(), "yellow".to_string()))
    );

    let yellow_list = request(
        &app.app,
        Method::GET,
        &format!("{base}/chats?tag=yellow"),
        None,
        None,
    )
    .await;
    assert_eq!(yellow_list.status(), StatusCode::OK);
    assert_eq!(yellow_list.headers()["X-Total-Count"], "1");
    let yellow_rows = read_json(yellow_list).await;
    assert_eq!(yellow_rows.as_array().unwrap().len(), 1);
    assert_eq!(yellow_rows[0]["id"], chat_id);

    let red_list = request(
        &app.app,
        Method::GET,
        &format!("{base}/chats?tag=red"),
        None,
        None,
    )
    .await;
    assert_eq!(red_list.status(), StatusCode::OK);
    assert_eq!(red_list.headers()["X-Total-Count"], "0");
    assert!(read_json(red_list).await.as_array().unwrap().is_empty());

    let workspace = app
        .state
        .registry
        .get_or_create(agent_id)
        .await
        .expect("workspace runtime")
        .workspace_dir();
    let session_store = WorkspaceSessionStore::new(&workspace);
    let header = session_store
        .load_chat_header(&chat_id)
        .await
        .expect("load Chat Session File")
        .expect("Chat Session File");
    assert_eq!(header.tag, rua_session::ChatTag::Yellow);
    let index_entry = chat_index::get(&workspace, &chat_id)
        .await
        .expect("load Chat Index")
        .expect("Chat Index row");
    assert_eq!(index_entry.tag, rua_session::ChatTag::Yellow);

    let deleted = request(
        &app.app,
        Method::DELETE,
        &format!("{base}/chats/{chat_id}"),
        None,
        None,
    )
    .await;
    assert_eq!(deleted.status(), StatusCode::OK);
    assert_eq!(read_json(deleted).await["deleted"], true);

    let orphan_dir = session_store.chat_dir(&chat_id);
    std::fs::create_dir_all(orphan_dir.join("messages")).expect("create orphan chat dir");

    let deleted_again = request(
        &app.app,
        Method::DELETE,
        &format!("{base}/chats/{chat_id}"),
        None,
        None,
    )
    .await;
    assert_eq!(deleted_again.status(), StatusCode::NOT_FOUND);
    assert!(read_json(deleted_again).await.get("detail").is_some());
    assert!(
        !orphan_dir.exists(),
        "repeat delete should still clean same-named orphan storage"
    );

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pin_order_routes_return_success_and_pinned_chats_sort_first() {
    let app = build_app();
    let agent_id = "integ_pin_order_agent";
    create_agent(&app.app, agent_id, "Pin order agent").await;

    let chat_a = seed_materialized_chat(&app, "/api/chats", Some(agent_id), "Pinned A").await;
    let chat_b = seed_materialized_chat(&app, "/api/chats", Some(agent_id), "Pinned B").await;
    let chat_a_id = chat_a["id"].as_str().expect("chat a id").to_string();
    let chat_b_id = chat_b["id"].as_str().expect("chat b id").to_string();

    for (chat_id, pin_order) in [(&chat_a_id, 20), (&chat_b_id, 10)] {
        let resp = request(
            &app.app,
            Method::PUT,
            &format!("/api/chats/{chat_id}"),
            Some(json!({ "pinned": true, "pin_order": pin_order })),
            Some(agent_id),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    let root_pin = request(
        &app.app,
        Method::PUT,
        "/api/chats/pin-order",
        Some(json!([
            {"id": chat_a_id, "pin_order": 1},
            {"id": chat_b_id, "pin_order": 2}
        ])),
        Some(agent_id),
    )
    .await;
    assert_eq!(root_pin.status(), StatusCode::OK);
    assert_eq!(read_json(root_pin).await["success"], true);

    let list = request(&app.app, Method::GET, "/api/chats", None, Some(agent_id)).await;
    assert_eq!(list.status(), StatusCode::OK);
    let ordered_ids = ids(&read_json(list).await);
    assert_eq!(ordered_ids[..2], [chat_a_id.clone(), chat_b_id.clone()]);

    let scoped_pin = request(
        &app.app,
        Method::PUT,
        &format!("/api/agents/{agent_id}/chats/pin-order"),
        Some(json!([
            {"id": chat_a_id, "pin_order": 30},
            {"id": chat_b_id, "pin_order": 5}
        ])),
        None,
    )
    .await;
    assert_eq!(scoped_pin.status(), StatusCode::OK);
    assert_eq!(read_json(scoped_pin).await["success"], true);

    let scoped_list = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{agent_id}/chats"),
        None,
        None,
    )
    .await;
    assert_eq!(scoped_list.status(), StatusCode::OK);
    let scoped_ordered_ids = ids(&read_json(scoped_list).await);
    assert_eq!(
        scoped_ordered_ids[..2],
        [chat_b_id.clone(), chat_a_id.clone()]
    );

    for chat_id in [&chat_a_id, &chat_b_id] {
        let unpin = request(
            &app.app,
            Method::PUT,
            &format!("/api/chats/{chat_id}"),
            Some(json!({ "pinned": false })),
            Some(agent_id),
        )
        .await;
        assert_eq!(unpin.status(), StatusCode::OK);
    }

    let delete_a = request(
        &app.app,
        Method::DELETE,
        &format!("/api/chats/{chat_a_id}"),
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(delete_a.status(), StatusCode::OK);
    let delete_b = request(
        &app.app,
        Method::DELETE,
        &format!("/api/chats/{chat_b_id}"),
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(delete_b.status(), StatusCode::OK);
    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unpinned_chat_list_sorts_by_updated_at_not_created_at() {
    let app = build_app();
    let agent_id = "integ_chats_updated_sort";
    create_agent(&app.app, agent_id, "Chat updated sort agent").await;

    let workspace = app
        .state
        .registry
        .get_or_create(agent_id)
        .await
        .expect("workspace runtime")
        .workspace_dir();
    let now = Utc::now();
    let older_created = now - ChronoDuration::days(7);
    let newer_created = now - ChronoDuration::days(1);
    let recently_updated = now;
    let stale_update = now - ChronoDuration::days(6);

    seed_chats_from_json(
        &workspace,
        &json!({
            "version": 1,
            "chats": [
                {
                    "id": "created-newer",
                    "name": "Created newer but stale",
                    "user_id": "user-a",
                    "channel": "console",
                    "created_at": newer_created,
                    "updated_at": stale_update,
                    "status": "idle",
                    "pinned": false,
                    "pin_order": 0
                },
                {
                    "id": "updated-newer",
                    "name": "Created older but updated newer",
                    "user_id": "user-b",
                    "channel": "console",
                    "created_at": older_created,
                    "updated_at": recently_updated,
                    "status": "idle",
                    "pinned": false,
                    "pin_order": 0
                }
            ]
        }),
    )
    .await;

    let list = request(&app.app, Method::GET, "/api/chats", None, Some(agent_id)).await;
    assert_eq!(list.status(), StatusCode::OK);
    let ordered_ids = ids(&read_json(list).await);
    assert_eq!(ordered_ids, ["updated-newer", "created-newer"]);

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn metadata_get_returns_archived_chat_without_history_or_scope_materialization() {
    let test = build_app();
    let agent_id = "metadata-owner";
    let other_id = "metadata-other";
    create_agent(&test.app, agent_id, "Metadata owner").await;
    create_agent(&test.app, other_id, "Metadata other").await;
    let chat_id = "00000000-0000-4000-8000-000000000913";
    let workspace = test.state.rua_root.join("workspaces").join(agent_id);
    let other_workspace = test.state.rua_root.join("workspaces").join(other_id);
    let uri = format!("/api/agents/{agent_id}/chats/{chat_id}/metadata");

    let missing = request(&test.app, Method::GET, &uri, None, None).await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    assert!(read_json(missing).await["detail"].is_string());
    assert!(!workspace.join("chat-index.sqlite").exists());
    assert!(!workspace.join("sessions").join(chat_id).exists());

    let now = Utc::now();
    chat_index::insert(
        &workspace,
        ChatIndexEntry {
            id: chat_id.to_string(),
            name: "Archived current chat".to_string(),
            user_id: "owner".to_string(),
            channel: "console".to_string(),
            created_at: now,
            updated_at: now,
            meta: json!({"sidebar_fixture": "kept"}),
            status: "idle".to_string(),
            pinned: false,
            pin_order: 0,
            tag: rua_session::ChatTag::Red,
            archived: true,
        },
    )
    .await
    .unwrap();
    let history_path = workspace.join("sessions").join(chat_id);
    std::fs::write(&history_path, b"not a conversation directory").unwrap();
    let response = request(&test.app, Method::GET, &uri, None, Some("default")).await;
    assert_eq!(response.status(), StatusCode::OK);
    let metadata = read_json(response).await;
    assert_eq!(metadata["id"], chat_id);
    assert_eq!(metadata["name"], "Archived current chat");
    assert_eq!(metadata["archived"], true);
    assert_eq!(metadata["tag"], "red");
    assert_eq!(metadata["meta"]["sidebar_fixture"], "kept");
    assert!(metadata.get("messages").is_none());
    assert_eq!(
        std::fs::read(&history_path).unwrap(),
        b"not a conversation directory"
    );

    for owner in [other_id, "metadata-unregistered"] {
        let response = request(
            &test.app,
            Method::GET,
            &format!("/api/agents/{owner}/chats/{chat_id}/metadata"),
            None,
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert!(read_json(response).await["detail"].is_string());
    }
    assert!(!other_workspace.join("chat-index.sqlite").exists());
    assert!(!test
        .state
        .rua_root
        .join("workspaces/metadata-unregistered")
        .exists());
}
#[tokio::test]
async fn draft_creation_and_missing_update_do_not_materialize_chat() {
    let app = build_app();
    let response = request(
        &app.app,
        Method::POST,
        "/api/agents/default/chats",
        Some(json!({"name":"draft only"})),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let draft = read_json(response).await;
    let id = draft["id"].as_str().unwrap();
    let runtime = app.state.registry.get_or_create("default").await.unwrap();
    let store = WorkspaceSessionStore::new(runtime.workspace_dir());
    assert!(
        store.load_chat_header(id).await.unwrap().is_none(),
        "minting a draft address must not create a Chat"
    );
    assert!(rua_session::chat_index::get(&runtime.workspace_dir(), id)
        .await
        .unwrap()
        .is_none());
    let updated = request(
        &app.app,
        Method::PUT,
        &format!("/api/agents/default/chats/{id}"),
        Some(json!({"name":"cannot create through update"})),
        None,
    )
    .await;
    assert_eq!(updated.status(), StatusCode::NOT_FOUND);
    assert!(store.load_chat_header(id).await.unwrap().is_none());
}

#[tokio::test]
async fn idle_chat_shutdown_does_not_persist_background_continuation_stop() {
    use rua_workspace::AgentWorkAuthority;
    let app = build_app();
    let chat =
        seed_materialized_chat(&app, "/api/agents/default/chats", None, "idle-shutdown").await;
    let address = rua_core::ChatAddress::agent("default", chat["id"].as_str().unwrap());
    let work = rua_server::services::workspace_work_lifecycle::WorkspaceWorkAuthority::new(
        app.state.conversation_authority.clone(),
        app.state.streams.clone(),
        app.state.task_runtime.clone(),
        rua_workspace::RuaRoot::new(&app.state.rua_root),
        None,
    );
    work.begin_shutdown("default").await.unwrap();
    assert!(
        !app.state
            .conversation_authority
            .background_work_view(&address)
            .await
            .suppressed
    );
    work.resume("default").await.unwrap();
    assert!(
        !app.state
            .conversation_authority
            .background_work_view(&address)
            .await
            .suppressed
    );
}
