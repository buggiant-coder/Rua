use std::sync::Arc;

use axum::http::{Method, StatusCode};
use rua_core::{ChatAddress, ChatScopeStore};
use rua_server::services::console_chat::CONSOLE_CHANNEL_ID;
use rua_server::services::conversation_authority::{
    ConversationActiveTurnStart, ConversationAuthority, ConversationDriverKind,
    ConversationInputPayload,
};
use rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch;
use rua_server::services::foreground_operation::ForegroundOperationRegistry;
use rua_server::services::product_hooks::{
    ChatConfigurationMaterialization, ChatHookConfiguration, CreateUserHookDefinition,
    HookHandlerView, ProductHookService,
};
use rua_server::services::scope_composition::{HookBinding, SelectionSource};
use rua_server::streaming::{ActiveStreamRegistry, Priority, StreamKey};
use serde_json::json;

use super::support::{build_app, read_json, request};

const DEFINITION_ID: &str = "rua:native/turn-terminal-audit";

#[tokio::test]
async fn configuration_candidate_is_not_adopted_until_persistence_succeeds() {
    let hooks = ProductHookService::new();
    let address = ChatAddress::agent("candidate-agent", "candidate-chat");
    hooks
        .set_agent_activation("candidate-agent", DEFINITION_ID, true)
        .await
        .unwrap();

    let candidate = hooks
        .resolve_chat_configuration(&address, "candidate-agent")
        .await
        .unwrap();

    assert!(hooks.chat_configuration(&address).await.is_none());
    hooks
        .adopt_chat_configuration(&address, candidate.clone())
        .await;
    assert_eq!(hooks.chat_configuration(&address).await, Some(candidate));
}

#[tokio::test]
async fn contributions_api_filters_by_chat_and_turn_with_provenance() {
    let test = build_app();
    let address = ChatAddress::agent("contribution-agent", "chat-a");
    test.state
        .product_hooks
        .record_contribution(
            rua_server::services::product_hooks::ProductHookContributionView {
                chat_address: address,
                turn_id: "turn-a".to_string(),
                tool_call_id: "tool-a".to_string(),
                event: "tool.after".to_string(),
                definition_revision: "agent:contribution-agent/observer@v1".to_string(),
                source: "[\"agent\"]".to_string(),
                resolved_order: 2,
                kind: "feedback".to_string(),
                content: json!({"rating": "good"}),
                diagnostic: None,
            },
        )
        .await;

    let response = request(
        &test.app,
        Method::GET,
        "/api/agents/contribution-agent/hooks/contributions?chat_id=chat-a&turn_id=turn-a",
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = read_json(response).await;
    assert_eq!(body.as_array().unwrap().len(), 1);
    assert_eq!(
        body[0]["definition_revision"],
        "agent:contribution-agent/observer@v1"
    );
    assert_eq!(body[0]["resolved_order"], 2);
    assert_eq!(body[0]["kind"], "feedback");
}

#[tokio::test]
async fn queued_claim_uses_current_epoch_while_active_turn_keeps_frozen_epoch() {
    let test = build_app();
    let agent_id = "claim-epoch-agent";
    assert_eq!(
        request(
            &test.app,
            Method::POST,
            "/api/agents",
            Some(json!({"id": agent_id, "name": "Epoch Agent"}))
        )
        .await
        .status(),
        StatusCode::CREATED
    );
    request(
        &test.app,
        Method::PUT,
        &format!("/api/agents/{agent_id}/hooks/defaults"),
        Some(json!({"definition_id": DEFINITION_ID, "enabled": true})),
    )
    .await;
    let chat = super::support::seed_materialized_chat(&test, agent_id, "Claim epochs").await;
    let chat_id = chat["id"].as_str().unwrap();
    let address = ChatAddress::agent(agent_id, chat_id);
    let old_configuration = test
        .state
        .product_hooks
        .chat_configuration(&address)
        .await
        .unwrap();
    let first_turn = test
        .state
        .conversation_authority
        .start_active_turn(ConversationActiveTurnStart {
            address: address.clone(),
            driver: ConversationDriverKind::Rua,
            authority_device_id: "device-test".to_string(),
            authority_epoch: ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
            driver_detail: None,
        })
        .await;
    let mut queued = ConversationInputPayload {
        message: "queued".to_string(),
        model_override: None,
        input: None,
        messages: None,
        extra: serde_json::Map::new(),
        attachments: Vec::new(),
        enqueued_at: std::time::Instant::now(),
    };
    queued.extra.insert(
        "hook_configuration".to_string(),
        serde_json::to_value(old_configuration).unwrap(),
    );
    test.state
        .conversation_authority
        .enqueue_input(&address, queued, None)
        .await
        .unwrap();

    request(
        &test.app,
        Method::PUT,
        &format!("/api/agents/{agent_id}/hooks/defaults"),
        Some(json!({"definition_id": DEFINITION_ID, "enabled": false})),
    )
    .await;
    let configuration_path = format!("/api/agents/{agent_id}/chats/{chat_id}/configuration");
    let preview = read_json(request(&test.app, Method::GET, &configuration_path, None).await).await;
    let updated = request(&test.app, Method::PUT, &configuration_path, Some(json!({
        "expected_epoch": preview["current"]["epoch"], "candidate_epoch": preview["candidate"]["epoch"],
    }))).await;
    assert_eq!(updated.status(), StatusCode::OK);

    let key = StreamKey::new(CONSOLE_CHANNEL_ID, address.clone(), Priority::Normal);
    let (_producer_id, _cancel_rx) = test.state.streams.register_active_keyed(&key).await;
    let handoff = test
        .state
        .conversation_authority
        .complete_active_turn_and_drain_queue(
            test.state.streams.as_ref(),
            &key,
            &address,
            &first_turn.turn_id,
            "completed",
            Some("2026-08-24T00:00:00Z".to_string()),
            None,
            true,
            |_| async {},
        )
        .await
        .expect("complete first Turn");
    let next_turn = test
        .state
        .conversation_authority
        .active_turn(&address)
        .await
        .expect("queued input claimed");
    assert!(handoff.next_input.is_some());
    test.state
        .conversation_authority
        .complete_active_turn_and_drain_queue(
            test.state.streams.as_ref(),
            &key,
            &address,
            &next_turn.turn_id,
            "completed",
            Some("2026-08-24T00:00:01Z".to_string()),
            None,
            false,
            |_| async {},
        )
        .await
        .expect("complete queued Turn");

    let runs = test.state.product_hooks.runs(agent_id, Some(chat_id)).await;
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].turn_id, first_turn.turn_id);
}

#[tokio::test]
async fn running_configuration_update_preserves_claimed_epoch_and_persists_new_snapshot() {
    let test = build_app();
    let agent_id = "running-epoch-agent";
    create_configuration_agent(&test.app, agent_id).await;
    let chat = super::support::seed_materialized_chat(&test, agent_id, "Running epochs").await;
    let chat_id = chat["id"].as_str().unwrap();
    let address = ChatAddress::agent(agent_id, chat_id);
    let before = test
        .state
        .product_hooks
        .chat_configuration(&address)
        .await
        .unwrap();
    test.state
        .product_hooks
        .adopt_chat_configuration(&address, before.clone())
        .await;
    let active = test
        .state
        .conversation_authority
        .start_active_turn(ConversationActiveTurnStart {
            address: address.clone(),
            driver: ConversationDriverKind::Rua,
            authority_device_id: "test-device".to_string(),
            authority_epoch: ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
            driver_detail: None,
        })
        .await;
    let scope = ChatScopeStore::resolve(&test.state.rua_root, address.clone());
    let mut running = before.running_config.clone();
    running.llm_max_concurrent = 2;
    running.llm_max_retries = 8;
    let mut events = test
        .state
        .conversation_authority
        .subscribe_control_events(&address)
        .await;
    test.state
        .conversation_input_intake
        .update_authority_chat_running_config(
            &address,
            agent_id,
            scope.root(),
            serde_json::to_value(&running).unwrap(),
        )
        .await
        .unwrap();
    let after = test
        .state
        .product_hooks
        .chat_configuration(&address)
        .await
        .unwrap();
    assert_ne!(before.epoch, after.epoch);
    assert_eq!(after.running_config, running);
    assert_eq!(
        test.state
            .product_hooks
            .frozen_turn_configuration(&address, &active.turn_id)
            .await
            .unwrap(),
        before
    );
    let store = rua_session::WorkspaceSessionStore::new(scope.root());
    assert_eq!(
        store.load_chat_running_config(chat_id).await.unwrap(),
        Some(running)
    );
    let header = store.load_chat_header(chat_id).await.unwrap().unwrap();
    let persisted: ChatHookConfiguration =
        serde_json::from_value(header.extra["hook_configuration"].clone()).unwrap();
    assert_eq!(persisted, after);
    let event: serde_json::Value = serde_json::from_str(&events.try_recv().unwrap()).unwrap();
    assert_eq!(event["reason"], "running_config_updated");
    assert_eq!(
        event["control"]["running_config"],
        serde_json::to_value(&after.running_config).unwrap()
    );
    assert!(events.try_recv().is_err());
}

#[tokio::test]
async fn invalid_running_configuration_does_not_replace_current_epoch() {
    let test = build_app();
    let agent_id = "invalid-running-agent";
    create_configuration_agent(&test.app, agent_id).await;
    let chat = super::support::seed_materialized_chat(&test, agent_id, "Invalid running").await;
    let chat_id = chat["id"].as_str().unwrap();
    let address = ChatAddress::agent(agent_id, chat_id);
    let scope = ChatScopeStore::resolve(&test.state.rua_root, address.clone());
    let before = test
        .state
        .product_hooks
        .chat_configuration(&address)
        .await
        .unwrap();
    let mut invalid = serde_json::to_value(&before.running_config).unwrap();
    invalid["llm_max_concurrent"] = json!(0);
    assert!(test
        .state
        .conversation_input_intake
        .update_authority_chat_running_config(&address, agent_id, scope.root(), invalid,)
        .await
        .is_err());
    assert_eq!(
        test.state
            .product_hooks
            .chat_configuration(&address)
            .await
            .unwrap(),
        before
    );
    let store = rua_session::WorkspaceSessionStore::new(scope.root());
    assert_eq!(
        store.load_chat_running_config(chat_id).await.unwrap(),
        Some(before.running_config)
    );
}

#[tokio::test]
async fn collaboration_mode_update_changes_only_future_configuration_epoch() {
    let test = build_app();
    let agent_id = "mode-epoch-agent";
    create_configuration_agent(&test.app, agent_id).await;
    let chat = super::support::seed_materialized_chat(&test, agent_id, "Mode epoch").await;
    let chat_id = chat["id"].as_str().unwrap();
    let address = ChatAddress::agent(agent_id, chat_id);
    let before = test
        .state
        .product_hooks
        .chat_configuration(&address)
        .await
        .unwrap();
    let active = test
        .state
        .conversation_authority
        .start_active_turn(ConversationActiveTurnStart {
            address: address.clone(),
            driver: ConversationDriverKind::Rua,
            authority_device_id: "test-device".to_string(),
            authority_epoch: ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
            driver_detail: None,
        })
        .await;
    let response = request(
        &test.app,
        Method::PUT,
        &format!("/api/agents/{agent_id}/chats/{chat_id}"),
        Some(json!({"collaboration_mode": "plan"})),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let after = test
        .state
        .product_hooks
        .chat_configuration(&address)
        .await
        .unwrap();
    assert_ne!(after.epoch, before.epoch);
    assert_eq!(after.collaboration_mode, json!("plan"));
    assert_eq!(
        test.state
            .product_hooks
            .frozen_turn_configuration(&address, &active.turn_id)
            .await
            .unwrap(),
        before
    );
}

#[tokio::test]
async fn chat_security_changes_only_on_explicit_configuration_update() {
    let test = build_app();
    let agent_id = "security-epoch-agent";
    assert_eq!(
        request(
            &test.app,
            Method::POST,
            "/api/agents",
            Some(json!({"id": agent_id, "name": "Security epoch"}))
        )
        .await
        .status(),
        StatusCode::CREATED
    );
    let mut initial = rua_config::types::security::SecurityConfig::default();
    initial.tool_guard.approval_mode = "ask_first".into();
    test.state
        .security
        .save_and_invalidate(agent_id, &initial)
        .unwrap();
    let chat = super::support::seed_materialized_chat(&test, agent_id, "Security snapshot").await;
    let chat_id = chat["id"].as_str().unwrap();
    let address = ChatAddress::agent(agent_id, chat_id);
    let before = test
        .state
        .product_hooks
        .chat_configuration(&address)
        .await
        .unwrap();
    assert_eq!(before.security_config.tool_guard.approval_mode, "ask_first");
    test.state
        .security
        .save_and_invalidate(
            agent_id,
            &rua_config::types::security::SecurityConfig::default(),
        )
        .unwrap();
    assert_eq!(
        test.state
            .product_hooks
            .chat_configuration(&address)
            .await
            .unwrap()
            .security_config,
        initial
    );
    let path = format!("/api/agents/{agent_id}/chats/{chat_id}/configuration");
    let preview_response = request(&test.app, Method::GET, &path, None).await;
    assert_eq!(preview_response.status(), StatusCode::OK);
    let preview = read_json(preview_response).await;
    assert_eq!(
        preview["current"]["security_config"]["tool_guard"]["approval_mode"],
        "ask_first"
    );
    assert_eq!(
        preview["candidate"]["security_config"]["tool_guard"]["approval_mode"],
        "hands_off"
    );
    let applied = request(&test.app, Method::PUT, &path, Some(json!({"expected_epoch": preview["current"]["epoch"], "candidate_epoch": preview["candidate"]["epoch"]}))).await;
    assert_eq!(applied.status(), StatusCode::OK);
    assert_eq!(
        test.state
            .product_hooks
            .chat_configuration(&address)
            .await
            .unwrap()
            .security_config
            .tool_guard
            .approval_mode,
        "hands_off"
    );
}

#[tokio::test]
async fn complete_configuration_preview_requires_matching_epoch_and_applies_exact_candidate() {
    let test = build_app();
    let agent_id = "preview-epoch-agent";
    let created = request(
        &test.app,
        Method::POST,
        "/api/agents",
        Some(json!({"id": agent_id, "name": "Preview Agent"})),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let chat = super::support::seed_materialized_chat(&test, agent_id, "Preview epochs").await;
    let chat_id = chat["id"].as_str().unwrap();
    let address = ChatAddress::agent(agent_id, chat_id);
    let scope = ChatScopeStore::resolve(&test.state.rua_root, address.clone());
    let intake = &test.state.conversation_input_intake;
    let preview = intake
        .chat_configuration_control(&address, agent_id, scope.root(), None)
        .await
        .unwrap();
    let same_epoch = test
        .state
        .product_hooks
        .chat_configuration(&address)
        .await
        .unwrap();
    test.state
        .product_hooks
        .adopt_chat_configuration(&address, same_epoch)
        .await;
    assert_eq!(preview["applied"], false);
    assert_ne!(preview["current"]["epoch"], preview["candidate"]["epoch"]);
    let before = test
        .state
        .product_hooks
        .chat_configuration(&address)
        .await
        .unwrap();
    assert!(intake
        .chat_configuration_control(
            &address,
            agent_id,
            scope.root(),
            Some(("stale", preview["candidate"]["epoch"].as_str().unwrap()))
        )
        .await
        .is_err());
    assert_eq!(
        test.state
            .product_hooks
            .chat_configuration(&address)
            .await
            .unwrap(),
        before
    );
    let applied = intake
        .chat_configuration_control(
            &address,
            agent_id,
            scope.root(),
            Some((
                preview["current"]["epoch"].as_str().unwrap(),
                preview["candidate"]["epoch"].as_str().unwrap(),
            )),
        )
        .await
        .unwrap();
    assert_eq!(applied["applied"], true);
    assert_eq!(applied["candidate"], preview["candidate"]);
    let persisted = rua_session::WorkspaceSessionStore::new(scope.root())
        .load_chat_header(chat_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(persisted.extra["hook_configuration"], preview["candidate"]);
    let history = request(
        &test.app,
        Method::GET,
        &format!("/api/agents/{agent_id}/chats/{chat_id}"),
        None,
    )
    .await;
    assert_eq!(history.status(), StatusCode::OK);
    assert_eq!(
        read_json(history).await["meta"]["hook_configuration"]["epoch"],
        preview["candidate"]["epoch"]
    );
}

#[tokio::test]
async fn configuration_persistence_failure_does_not_adopt_or_publish_and_can_be_retried() {
    let test = build_app();
    let chat = super::support::seed_materialized_chat(&test, "default", "Failed commit").await;
    let chat_id = chat["id"].as_str().unwrap();
    let address = ChatAddress::agent("default", chat_id);
    let scope = ChatScopeStore::resolve(&test.state.rua_root, address.clone());
    let store = rua_session::WorkspaceSessionStore::new(scope.root());
    let intake = &test.state.conversation_input_intake;
    let preview = intake
        .chat_configuration_control(&address, "default", scope.root(), None)
        .await
        .unwrap();
    let current_epoch = preview["current"]["epoch"].as_str().unwrap();
    let candidate_epoch = preview["candidate"]["epoch"].as_str().unwrap();
    let before = test
        .state
        .product_hooks
        .chat_configuration(&address)
        .await
        .unwrap();
    let mut events = test
        .state
        .conversation_authority
        .subscribe_control_events(&address)
        .await;

    // Block only the new immutable snapshot. The current configuration stays
    // readable, so failure happens during persistence, after preview validation.
    let blocked_path = store
        .configuration_snapshot_path(chat_id, candidate_epoch)
        .unwrap();
    std::fs::create_dir(&blocked_path).unwrap();
    assert!(intake
        .chat_configuration_control(
            &address,
            "default",
            scope.root(),
            Some((current_epoch, candidate_epoch))
        )
        .await
        .is_err());
    assert_eq!(
        test.state.product_hooks.chat_configuration(&address).await,
        Some(before)
    );
    assert_eq!(
        store
            .load_chat_header(chat_id)
            .await
            .unwrap()
            .unwrap()
            .extra["hook_configuration"],
        preview["current"]
    );
    assert!(matches!(
        events.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));

    std::fs::remove_dir(blocked_path).unwrap();
    let applied = intake
        .chat_configuration_control(
            &address,
            "default",
            scope.root(),
            Some((current_epoch, candidate_epoch)),
        )
        .await
        .unwrap();
    assert_eq!(applied["candidate"], preview["candidate"]);
    assert_eq!(
        store
            .load_chat_header(chat_id)
            .await
            .unwrap()
            .unwrap()
            .extra["hook_configuration"],
        preview["candidate"]
    );
    assert_eq!(
        serde_json::to_value(
            test.state
                .product_hooks
                .chat_configuration(&address)
                .await
                .unwrap()
        )
        .unwrap(),
        preview["candidate"]
    );
    let event: serde_json::Value = serde_json::from_str(&events.try_recv().unwrap()).unwrap();
    assert_eq!(event["reason"], "configuration_updated");
    assert!(matches!(
        events.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
}

#[tokio::test]
async fn configuration_preview_preserves_unset_choices_in_a_complete_snapshot() {
    let test = build_app();
    let chat = super::support::seed_materialized_chat(&test, "default", "Unset choices").await;
    let chat_id = chat["id"].as_str().unwrap();
    let address = ChatAddress::agent("default", chat_id);
    let scope = ChatScopeStore::resolve(&test.state.rua_root, address.clone());
    let store = rua_session::WorkspaceSessionStore::new(scope.root());
    let mut header = store.load_chat_header(chat_id).await.unwrap().unwrap();
    let mut configuration = header.extra["hook_configuration"].clone();
    let previous_epoch = configuration["epoch"].as_str().unwrap().to_string();
    configuration["epoch"] = json!("unset-choices");
    for key in ["model", "thinking", "harness_configuration"] {
        configuration.as_object_mut().unwrap().remove(key);
        header.extra.insert(key.into(), json!({"stale": true}));
    }
    store
        .replace_chat_configuration(chat_id, &previous_epoch, configuration)
        .await
        .unwrap();
    store.save_chat_header(&header).await.unwrap();
    let preview = test
        .state
        .conversation_input_intake
        .chat_configuration_control(&address, "default", scope.root(), None)
        .await
        .unwrap();
    for key in ["model", "thinking", "harness_configuration"] {
        assert!(preview["current"].get(key).is_none());
        assert!(preview["candidate"].get(key).is_none(), "changed {key}");
    }
}

#[tokio::test]
async fn claimed_input_uses_snapshotted_assignment_instead_of_new_sender_defaults() {
    let hooks = Arc::new(ProductHookService::new());
    let authority = ConversationAuthority::with_product_hooks(
        Arc::new(ForegroundOperationRegistry::new(true)),
        hooks.clone(),
    );
    let address = ChatAddress::agent("fixed-agent", uuid::Uuid::new_v4().to_string());
    let mut materialization = complete_materialization("fixed-agent", "test-device");
    let assignment = rua_chat::TurnAgentAssignment {
        agent_id: "fixed-agent".to_string(),
        agent_name: "Original".to_string(),
        assigned_at: "2026-01-01T00:00:00Z".to_string(),
        system_prompt: Some("Original instructions".to_string()),
        system_prompt_files: Vec::new(),
        disabled_tools: vec!["bash".to_string()],
    };
    materialization.agent_assignment = Some(assignment.clone());
    materialization.prompt_file_content = Some("Pinned file instructions".to_string());
    materialization.model = Some(
        json!({"provider_id": "fixture", "model": "captured-model", "thinking_strength": "high"}),
    );
    materialization.running_config.llm_max_concurrent = 2;
    let configuration = hooks
        .resolve_complete_chat_configuration(&address, materialization)
        .await
        .unwrap();
    hooks
        .adopt_chat_configuration(&address, configuration.clone())
        .await;
    let mut pending = pending_message("hello");
    let mut changed = assignment.clone();
    changed.system_prompt = Some("Changed default".to_string());
    changed.disabled_tools.clear();
    pending.extra.insert(
        rua_chat::TURN_AGENT_ASSIGNMENT_META_KEY.to_string(),
        serde_json::to_value(changed).unwrap(),
    );
    pending.extra.insert(
        rua_chat::TURN_MODEL_KEY.to_string(),
        json!({"provider_id": "fixture", "model": "stale-model"}),
    );
    let streams = ActiveStreamRegistry::new();
    let key = StreamKey::new(CONSOLE_CHANNEL_ID, address.clone(), Priority::Normal);
    let accepted = authority
        .accept_submission(
            &streams,
            &key,
            ConversationActiveTurnStart {
                address: address.clone(),
                driver: ConversationDriverKind::Rua,
                authority_device_id: "test-device".to_string(),
                authority_epoch: ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
                driver_detail: None,
            },
            pending,
            None,
            None,
        )
        .await
        .unwrap();
    let rua_server::services::conversation_authority::ConversationSubmissionAcceptance::Started {
        producer_id,
        ..
    } = accepted
    else {
        panic!("expected started input");
    };
    let pending = authority
        .claim_next_or_finish(&streams, &key, producer_id, &address)
        .await
        .expect("claimed input")
        .pending;
    assert_eq!(
        pending.extra[rua_chat::TURN_AGENT_ASSIGNMENT_META_KEY],
        serde_json::to_value(assignment).unwrap()
    );
    assert_eq!(
        pending.extra["hook_configuration"],
        serde_json::to_value(configuration).unwrap()
    );
    assert_eq!(
        pending.extra["hook_configuration"]["prompt_file_content"],
        "Pinned file instructions"
    );
    assert_eq!(
        pending.extra[rua_chat::TURN_MODEL_KEY]["model"],
        "captured-model"
    );
    assert_eq!(
        pending.extra[rua_chat::TURN_THINKING_KEY]["thinking_strength"],
        "high"
    );
}

#[tokio::test]
async fn deleting_agent_default_file_preserves_materialized_chat_configuration() {
    let test = build_app();
    let agent_id = "removed-default-agent";
    assert_eq!(
        request(
            &test.app,
            Method::POST,
            "/api/agents",
            Some(json!({"id": agent_id, "name": "Removed defaults"}))
        )
        .await
        .status(),
        StatusCode::CREATED
    );
    let chat =
        super::support::seed_materialized_chat(&test, agent_id, "Retained configuration").await;
    let chat_id = chat["id"].as_str().unwrap();
    let address = ChatAddress::agent(agent_id, chat_id);
    let scope = ChatScopeStore::resolve(&test.state.rua_root, address.clone());
    let store = rua_session::WorkspaceSessionStore::new(scope.root());
    let before = store
        .load_chat_running_config(chat_id)
        .await
        .unwrap()
        .unwrap();
    tokio::fs::remove_file(scope.root().join("agent.json"))
        .await
        .unwrap();
    assert_eq!(
        test.state
            .conversation_input_intake
            .authority_chat_running_config(&address, agent_id, scope.root())
            .await
            .unwrap(),
        before
    );
    let serialized = store
        .load_chat_header(chat_id)
        .await
        .unwrap()
        .unwrap()
        .extra["hook_configuration"]
        .clone();
    let restored: ChatHookConfiguration = serde_json::from_value(serialized).unwrap();
    assert_eq!(restored.running_config, before);
    assert!(restored.agent_assignment.is_some());
    restored.validate().unwrap();
    let response = request(
        &test.app,
        Method::GET,
        &format!("/api/agents/{agent_id}/hooks/chats/{chat_id}/configuration"),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        read_json(response).await,
        serde_json::to_value(restored).unwrap()
    );
}

#[tokio::test]
async fn invalid_configuration_is_rejected_before_submission_is_accepted() {
    let authority = ConversationAuthority::new();
    let address = ChatAddress::agent("invalid-config", uuid::Uuid::new_v4().to_string());
    let streams = ActiveStreamRegistry::new();
    let key = StreamKey::new(CONSOLE_CHANNEL_ID, address.clone(), Priority::Normal);
    let mut input = pending_message("must not execute");
    input
        .extra
        .insert("hook_configuration".to_string(), json!({"epoch": "broken"}));
    let result = authority
        .accept_submission(
            &streams,
            &key,
            ConversationActiveTurnStart {
                address: address.clone(),
                driver: ConversationDriverKind::Rua,
                authority_device_id: "device".to_string(),
                authority_epoch: ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
                driver_detail: None,
            },
            input,
            None,
            None,
        )
        .await;
    assert!(result.is_err());
    assert!(authority.active_turn(&address).await.is_none());
}

#[tokio::test]
async fn complete_configuration_http_preview_and_apply_preserve_unknown_fields() {
    let test = build_app();
    let agent_id = "http-preview-agent";
    assert_eq!(
        request(
            &test.app,
            Method::POST,
            "/api/agents",
            Some(json!({"id": agent_id, "name": "Preview"}))
        )
        .await
        .status(),
        StatusCode::CREATED
    );
    let chat = super::support::seed_materialized_chat(&test, agent_id, "HTTP Preview").await;
    let chat_id = chat["id"].as_str().unwrap();
    let path = format!("/api/agents/{agent_id}/chats/{chat_id}/configuration");
    let response = request(&test.app, Method::GET, &path, None).await;
    assert_eq!(response.status(), StatusCode::OK);
    let preview = read_json(response).await;
    let response = request(
        &test.app,
        Method::PUT,
        &path,
        Some(json!({
            "expected_epoch": preview["current"]["epoch"],
            "candidate_epoch": preview["candidate"]["epoch"],
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(read_json(response).await["candidate"], preview["candidate"]);
    let mut value = preview["candidate"].clone();
    value["future_section"] = json!({"enabled": true});
    let snapshot: ChatHookConfiguration = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(snapshot).unwrap(), value);
}

#[tokio::test]
async fn project_materialization_adds_project_hooks_without_agent_scope_leakage() {
    let hooks = ProductHookService::new();
    hooks
        .set_agent_activation("scope-agent", DEFINITION_ID, true)
        .await
        .unwrap();
    hooks
        .set_project_bindings(
            "scope-project",
            vec![HookBinding {
                definition_revision: "project:scope-project/project-only@v1".to_string(),
                event: "turn.terminal".to_string(),
                source: SelectionSource::Project,
                configured_order: 1,
                parameters: Default::default(),
                before: Vec::new(),
                after: Vec::new(),
                requires: Vec::new(),
            }],
        )
        .await;

    let agent = hooks
        .resolve_chat_configuration(
            &ChatAddress::agent("scope-agent", "agent-chat"),
            "scope-agent",
        )
        .await
        .unwrap();
    let project = hooks
        .resolve_chat_configuration(
            &ChatAddress::project("scope-project", "project-chat"),
            "scope-agent",
        )
        .await
        .unwrap();

    assert_eq!(
        agent.definition_revisions,
        vec![format!("{DEFINITION_ID}@v1")]
    );
    assert_eq!(
        project.definition_revisions,
        vec![
            format!("{DEFINITION_ID}@v1"),
            "project:scope-project/project-only@v1".to_string()
        ]
    );
    assert!(hooks
        .chat_configuration(&ChatAddress::project("scope-project", "project-chat"))
        .await
        .is_none());
}

#[tokio::test]
async fn invalid_composition_preserves_persisted_and_runtime_epoch() {
    let test = build_app();
    let agent_id = "atomic-agent";
    assert_eq!(
        request(
            &test.app,
            Method::POST,
            "/api/agents",
            Some(json!({"id": agent_id, "name": "Atomic Agent"}))
        )
        .await
        .status(),
        StatusCode::CREATED
    );
    let chat =
        super::support::seed_materialized_chat(&test, agent_id, "Atomic configuration").await;
    let chat_id = chat["id"].as_str().unwrap();
    let address = ChatAddress::agent(agent_id, chat_id);
    let previous = test
        .state
        .product_hooks
        .chat_configuration(&address)
        .await
        .unwrap();
    test.state
        .product_hooks
        .set_chat_bindings(
            &address,
            vec![HookBinding {
                definition_revision: "chat:atomic/invalid@v1".to_string(),
                event: "tool.before".to_string(),
                source: SelectionSource::Chat,
                configured_order: 0,
                parameters: Default::default(),
                before: Vec::new(),
                after: Vec::new(),
                requires: vec!["missing:required@v1".to_string()],
            }],
        )
        .await;

    let response = request(
        &test.app,
        Method::GET,
        &format!("/api/agents/{agent_id}/chats/{chat_id}/configuration"),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        test.state.product_hooks.chat_configuration(&address).await,
        Some(previous.clone())
    );

    let scope = ChatScopeStore::resolve(&test.state.rua_root, address);
    let header = rua_session::WorkspaceSessionStore::new(scope.root())
        .load_chat_header(chat_id)
        .await
        .unwrap()
        .unwrap();
    let persisted: ChatHookConfiguration =
        serde_json::from_value(header.extra["hook_configuration"].clone()).unwrap();
    assert_eq!(persisted, previous);
    assert!(header.extra.get("model").is_none());
}

#[tokio::test]
async fn restart_restores_persisted_configuration_when_the_turn_is_claimed() {
    let test = build_app();
    let agent_id = "restart-agent";
    create_configuration_agent(&test.app, agent_id).await;
    request(
        &test.app,
        Method::PUT,
        &format!("/api/agents/{agent_id}/hooks/defaults"),
        Some(json!({"definition_id": DEFINITION_ID, "enabled": true})),
    )
    .await;
    let chat = super::support::seed_materialized_chat(&test, agent_id, "Restart").await;
    let chat_id = chat["id"].as_str().unwrap();
    let address = ChatAddress::agent(agent_id, chat_id);
    let scope = ChatScopeStore::resolve(&test.state.rua_root, address.clone());
    let header = rua_session::WorkspaceSessionStore::new(scope.root())
        .load_chat_header(chat_id)
        .await
        .unwrap()
        .unwrap();
    let persisted: ChatHookConfiguration =
        serde_json::from_value(header.extra["hook_configuration"].clone()).unwrap();

    let restored_hooks = Arc::new(ProductHookService::new());
    let authority = ConversationAuthority::with_product_hooks(
        Arc::new(ForegroundOperationRegistry::new(false)),
        Arc::clone(&restored_hooks),
    );
    let streams = ActiveStreamRegistry::new();
    let key = StreamKey::new(CONSOLE_CHANNEL_ID, address.clone(), Priority::Normal);
    let mut pending = pending_message("after restart");
    pending.extra.insert(
        "hook_configuration".to_string(),
        serde_json::to_value(&persisted).unwrap(),
    );
    let accepted = authority
        .accept_submission(
            &streams,
            &key,
            ConversationActiveTurnStart {
                address: address.clone(),
                driver: ConversationDriverKind::Rua,
                authority_device_id: persisted.authority_device_id.clone().unwrap(),
                authority_epoch: ConversationAuthorityEpoch::from_parts(2, 0, [0; 8]),
                driver_detail: None,
            },
            pending,
            None,
            Some(Box::new(|_, _| Box::pin(async { Ok(()) }))),
        )
        .await
        .unwrap();
    assert!(matches!(
        accepted,
        rua_server::services::conversation_authority::ConversationSubmissionAcceptance::Started { .. }
    ));
    assert_eq!(
        restored_hooks.chat_configuration(&address).await,
        Some(persisted)
    );
}

#[tokio::test]
async fn structured_command_context_and_device_are_preserved_without_fallback() {
    let hooks = ProductHookService::new();
    let agent_id = "requirements-agent";
    let definition = hooks
        .create_user_definition(
            agent_id,
            CreateUserHookDefinition {
                logical_id: "offline".to_string(),
                display_name: "Offline".to_string(),
                event: "tool.before".to_string(),
                capabilities: vec!["identity".to_string()],
                handler: HookHandlerView {
                    kind: "command".to_string(),
                    executable: None,
                    program: Some("/usr/bin/python3".to_string()),
                    args: vec!["hooks/offline.py".to_string(), "--strict".to_string()],
                    available: false,
                    working_directory: Some("/agent/workspace".to_string()),
                    timeout_seconds: Some(45),
                    required_resources: vec!["gpu:0".to_string()],
                },
                owner_namespace: None,
                source: None,
            },
        )
        .await
        .unwrap();
    hooks
        .set_agent_activation(agent_id, &definition.id, true)
        .await
        .unwrap();
    let configuration = hooks
        .resolve_complete_chat_configuration(
            &ChatAddress::agent(agent_id, "requirements-chat"),
            complete_materialization(agent_id, "authority-device"),
        )
        .await
        .unwrap();
    let requirement = &configuration.execution_requirements[0];
    assert_eq!(requirement.authority_device_id, "authority-device");
    assert_eq!(requirement.program.as_deref(), Some("/usr/bin/python3"));
    assert_eq!(requirement.args, vec!["hooks/offline.py", "--strict"]);
    assert_eq!(
        requirement.working_directory.as_deref(),
        Some("/agent/workspace")
    );
    assert_eq!(requirement.required_resources, vec!["gpu:0"]);
    assert_eq!(requirement.timeout_seconds, Some(45));
    assert!(!requirement.available);

    let error = hooks
        .create_user_definition(
            agent_id,
            CreateUserHookDefinition {
                logical_id: "missing-program".to_string(),
                display_name: "Missing program".to_string(),
                event: "tool.before".to_string(),
                capabilities: vec!["identity".to_string()],
                handler: HookHandlerView {
                    kind: "command".to_string(),
                    executable: None,
                    program: None,
                    args: Vec::new(),
                    available: true,
                    working_directory: None,
                    timeout_seconds: Some(30),
                    required_resources: Vec::new(),
                },
                owner_namespace: None,
                source: None,
            },
        )
        .await
        .unwrap_err();
    assert!(error.contains("program is required"));

    let error = hooks
        .resolve_complete_chat_configuration(
            &ChatAddress::agent(agent_id, "missing-device"),
            complete_materialization(agent_id, ""),
        )
        .await
        .unwrap_err();
    assert!(error.contains("explicit Authority Device"));
}

#[tokio::test]
async fn first_project_claim_adopts_candidate_only_after_persistence_succeeds() {
    let hooks = Arc::new(ProductHookService::new());
    hooks
        .set_agent_activation("project-agent", DEFINITION_ID, true)
        .await
        .unwrap();
    let address = ChatAddress::project("project-a", "project-first");
    let candidate = hooks
        .resolve_complete_chat_configuration(
            &address,
            complete_materialization("project-agent", "resource-device"),
        )
        .await
        .unwrap();
    let authority = ConversationAuthority::with_product_hooks(
        Arc::new(ForegroundOperationRegistry::new(false)),
        Arc::clone(&hooks),
    );
    let streams = ActiveStreamRegistry::new();
    let key = StreamKey::new(CONSOLE_CHANNEL_ID, address.clone(), Priority::Normal);
    let start = || ConversationActiveTurnStart {
        address: address.clone(),
        driver: ConversationDriverKind::Rua,
        authority_device_id: "resource-device".to_string(),
        authority_epoch: ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
        driver_detail: None,
    };
    let pending = || {
        let mut pending = pending_message("first");
        pending.extra.insert(
            "hook_configuration".to_string(),
            serde_json::to_value(&candidate).unwrap(),
        );
        pending
    };

    let failed = authority
        .accept_submission(
            &streams,
            &key,
            start(),
            pending(),
            None,
            Some(Box::new(|_, _| {
                Box::pin(async { Err("persistence failed".to_string()) })
            })),
        )
        .await;
    assert!(matches!(failed, Err(error) if error == "persistence failed"));
    assert!(hooks.chat_configuration(&address).await.is_none());

    authority
        .accept_submission(
            &streams,
            &key,
            start(),
            pending(),
            None,
            Some(Box::new(|_, _| Box::pin(async { Ok(()) }))),
        )
        .await
        .unwrap();
    assert_eq!(hooks.chat_configuration(&address).await, Some(candidate));
}

#[tokio::test]
async fn new_agent_chat_materializes_one_complete_configuration_epoch() {
    let test = build_app();
    let agent_id = "complete-agent";
    create_configuration_agent(&test.app, agent_id).await;
    let activated = request(
        &test.app,
        Method::PUT,
        &format!("/api/agents/{agent_id}/hooks/defaults"),
        Some(json!({"definition_id": DEFINITION_ID, "enabled": true})),
    )
    .await;
    assert_eq!(activated.status(), StatusCode::OK);

    let created = request(
        &test.app,
        Method::POST,
        &format!("/api/agents/{agent_id}/chats"),
        Some(json!({
            "name": "Complete configuration",
            "user_id": "test",
            "channel": "console",
            "model": {
                "provider_id": "fixture",
                "model": "fixture-model",
                "thinking_strength": "high"
            },
            "collaboration_mode": "plan",
            "meta": {
                "harness_configuration": {"mode": "strict"}
            }
        })),
    )
    .await;
    assert_eq!(created.status(), StatusCode::OK);
    let mut created = read_json(created).await;
    assert!(created["meta"].get("hook_configuration").is_none());
    test.state.provider_cells.cell("fixture").write(serde_json::from_value( json!({
        "id":"fixture", "name":"Fixture", "chat_model":"OpenAIChatModel", "base_url":"http://127.0.0.1:1/v1", "api_key":"fixture",
        "models":[{"id":"fixture-model","name":"Fixture model","supports_reasoning":true,"thinking_param_style":"effort","reasoning_effort_options":["low","high"]}]
    })).unwrap()).await.unwrap();
    let id = created["id"].as_str().unwrap().to_string();
    let outcome = test.state.conversation_input_intake.submit_delivery(agent_id, rua_channel::core::console::ConsoleChatRequest {
        message:"first configuration input".into(),chat_id:Some(id.clone()),model:None,is_reconnect:false,attachments:vec![],input:None,messages:None,channel_kind:None,
        extra:serde_json::Map::from_iter([
            ("chat_model_slot".into(),json!({"provider_id":"fixture","model":"fixture-model","thinking_strength":"high"})),
            ("collaboration_mode".into(),json!("plan")),("harness_configuration".into(),json!({"mode":"strict"})),
        ]),
    }).await;
    assert!(matches!(outcome,rua_server::services::conversation_input_intake::ConversationInputDeliveryOutcome::Delivered { .. }),"{outcome:?}");
    let runtime = test.state.registry.get(agent_id).await.unwrap();
    let header = rua_session::WorkspaceSessionStore::new(runtime.workspace_dir())
        .load_chat_header(&id)
        .await
        .unwrap()
        .unwrap();
    created["meta"] = serde_json::to_value(header.extra).unwrap();
    let configuration = &created["meta"]["hook_configuration"];
    assert!(configuration["epoch"].as_str().is_some());
    assert_eq!(configuration["selected_agent_id"], agent_id);
    assert_eq!(configuration["model"]["provider_id"], "fixture");
    assert_eq!(configuration["model"]["model"], "fixture-model");
    assert_eq!(configuration["model"]["thinking_strength"], "high");
    assert_eq!(configuration["collaboration_mode"], "plan");
    assert_eq!(
        configuration["harness_configuration"],
        json!({"mode": "strict"})
    );
    assert_eq!(
        configuration["authority_device_id"],
        created["meta"]["authority_placement"]["device_id"]
    );
    assert_eq!(
        configuration["definition_revisions"],
        json!([format!("{DEFINITION_ID}@v1")])
    );
    assert_eq!(
        configuration["effective_hooks"][0]["definition_revision"],
        format!("{DEFINITION_ID}@v1")
    );
}

#[tokio::test]
async fn effective_preview_composes_overlay_order_and_warnings_deterministically() {
    let test = build_app();
    let uri = "/api/agents/preview-agent/hooks/effective-preview";
    let input = json!({
        "agent": [
            {"definition_revision": "agent:a@v1", "event": "tool.before", "source": "agent", "configured_order": 2, "parameters": {"cwd": "agent", "mode": "safe"}, "before": ["missing@v1"]},
            {"definition_revision": "shared:x@v1", "event": "tool.before", "source": "agent", "configured_order": 1, "parameters": {"cwd": "agent", "limit": 1}}
        ],
        "project": [
            {"definition_revision": "shared:x@v1", "event": "tool.before", "source": "project", "configured_order": 9, "parameters": {"cwd": "project"}}
        ],
        "chat": [
            {"definition_revision": "chat:z@v1", "event": "tool.before", "source": "chat", "configured_order": 0, "after": ["shared:x@v1"]}
        ]
    });
    let first = request(&test.app, Method::POST, uri, Some(input.clone())).await;
    assert_eq!(first.status(), StatusCode::OK);
    let first = read_json(first).await;
    assert_eq!(first["hooks"].as_array().unwrap().len(), 3);
    let shared = first["hooks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|hook| hook["definition_revision"] == "shared:x@v1")
        .unwrap();
    assert_eq!(shared["selected_by"], json!(["agent", "project"]));
    assert_eq!(shared["parameters"], json!({"cwd": "project", "limit": 1}));
    assert_eq!(shared["overridden_parameters"], json!(["cwd"]));
    assert_eq!(first["warnings"][0]["missing_target"], "missing@v1");

    let mut reordered = input;
    reordered["agent"].as_array_mut().unwrap().reverse();
    let second = request(&test.app, Method::POST, uri, Some(reordered)).await;
    assert_eq!(second.status(), StatusCode::OK);
    assert_eq!(read_json(second).await, first);
}

#[tokio::test]
async fn effective_preview_rejects_invalid_dependency_graphs() {
    let test = build_app();
    let uri = "/api/agents/preview-agent/hooks/effective-preview";
    let cases = [
        json!({"agent": [{"definition_revision": "a@v1", "event": "tool.before", "source": "agent", "requires": ["missing@v1"]}]}),
        json!({"agent": [
            {"definition_revision": "a@v1", "event": "tool.before", "source": "agent", "before": ["b@v1"]},
            {"definition_revision": "b@v1", "event": "tool.before", "source": "agent", "before": ["a@v1"]}
        ]}),
        json!({"agent": [
            {"definition_revision": "a@v1", "event": "tool.before", "source": "agent", "before": ["b@v1"]},
            {"definition_revision": "b@v1", "event": "tool.after", "source": "agent"}
        ]}),
        json!({"agent": [
            {"definition_revision": "same@v1", "event": "tool.before", "source": "agent"},
            {"definition_revision": "same@v1", "event": "tool.before", "source": "agent"}
        ]}),
    ];
    for body in cases {
        let response = request(&test.app, Method::POST, uri, Some(body)).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(read_json(response).await["detail"].as_str().is_some());
    }
}

#[tokio::test]
async fn user_definition_create_is_qualified_and_inactive() {
    let test = build_app();
    let agent_id = "registry-create";

    let created = request(
        &test.app,
        Method::POST,
        &format!("/api/agents/{agent_id}/hooks/definitions"),
        Some(json!({
            "logical_id": "notify-terminal",
            "display_name": "Notify terminal",
            "event": "turn.terminal",
            "capabilities": ["identity", "candidate_response"],
            "handler": {
                "kind": "command",
                "program": "/bin/sh",
                "args": ["hooks/notify.sh"],
                "working_directory": "/workspace",
                "timeout_seconds": 30,
                "available": true
            }
        })),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let created = read_json(created).await;
    assert_eq!(created["id"], "agent:registry-create/notify-terminal");
    assert_eq!(created["owner_namespace"], "agent:registry-create");
    assert_eq!(created["logical_id"], "notify-terminal");
    assert_eq!(created["display_name"], "Notify terminal");
    assert_eq!(created["source"], "user");
    assert_eq!(created["latest_revision"], "v1");
    assert_eq!(created["event"], "turn.terminal");
    assert_eq!(
        created["capabilities"],
        json!(["identity", "candidate_response"])
    );
    assert_eq!(created["availability"]["status"], "available");
    assert_eq!(created["validation"]["status"], "valid");
    assert_eq!(created["active"], false);

    let listed = request(
        &test.app,
        Method::GET,
        &format!("/api/agents/{agent_id}/hooks/definitions"),
        None,
    )
    .await;
    assert_eq!(listed.status(), StatusCode::OK);
    let listed = read_json(listed).await;
    assert!(listed.as_array().unwrap().iter().any(|definition| {
        definition["id"] == "agent:registry-create/notify-terminal" && definition["active"] == false
    }));
}

#[tokio::test]
async fn user_command_definition_round_trips_structured_execution_configuration() {
    let test = build_app();
    let agent_id = "structured-command";
    let owner = "agent:structured-command";

    let created = request(
        &test.app,
        Method::POST,
        &format!("/api/agents/{agent_id}/hooks/definitions"),
        Some(json!({
            "logical_id": "policy-check",
            "display_name": "Policy check",
            "event": "tool.before",
            "capabilities": ["identity"],
            "handler": {
                "kind": "command",
                "program": "/usr/bin/python3",
                "args": ["./hooks/policy_check.py", "--strict"],
                "working_directory": "/workspace/project",
                "timeout_seconds": 45,
                "available": true
            }
        })),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);

    let detail = request(
        &test.app,
        Method::GET,
        &format!("/api/agents/{agent_id}/hooks/definitions/{owner}/policy-check"),
        None,
    )
    .await;
    assert_eq!(detail.status(), StatusCode::OK);
    let detail = read_json(detail).await;
    let handler = &detail["revisions"][0]["handler"];
    assert_eq!(handler["kind"], "command");
    assert_eq!(handler["program"], "/usr/bin/python3");
    assert_eq!(
        handler["args"],
        json!(["./hooks/policy_check.py", "--strict"])
    );
    assert_eq!(handler["working_directory"], "/workspace/project");
    assert_eq!(handler["timeout_seconds"], 45);
    assert!(handler.get("executable").is_none());
    assert!(handler.get("runtime").is_none());
}

#[tokio::test]
async fn user_command_definition_rejects_legacy_runtime_and_executable_fields() {
    let test = build_app();
    let response = request(
        &test.app,
        Method::POST,
        "/api/agents/legacy-command/hooks/definitions",
        Some(json!({
            "logical_id": "legacy",
            "display_name": "Legacy",
            "event": "turn.terminal",
            "capabilities": ["identity"],
            "handler": {
                "kind": "command",
                "runtime": "/usr/bin/python3",
                "executable": "./hooks/legacy.py",
                "working_directory": "/workspace",
                "available": true
            }
        })),
    )
    .await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(read_json(response).await["detail"]
        .as_str()
        .unwrap()
        .contains("program is required"));
}

#[tokio::test]
async fn presentation_update_keeps_revision_and_command_update_creates_one() {
    let test = build_app();
    let agent_id = "registry-revise";
    let owner = "agent:registry-revise";
    let logical_id = "notify-terminal";

    let created = request(
        &test.app,
        Method::POST,
        &format!("/api/agents/{agent_id}/hooks/definitions"),
        Some(json!({
            "logical_id": logical_id,
            "display_name": "Notify terminal",
            "event": "turn.terminal",
            "capabilities": ["identity"],
            "handler": {
                "kind": "command",
                "program": "/bin/sh",
                "args": ["hooks/notify-v1.sh"],
                "working_directory": "/workspace",
                "timeout_seconds": 30,
                "available": true
            }
        })),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);

    let activated = request(
        &test.app,
        Method::PUT,
        &format!("/api/agents/{agent_id}/hooks/defaults"),
        Some(json!({
            "definition_id": format!("{owner}/{logical_id}"),
            "enabled": true
        })),
    )
    .await;
    assert_eq!(activated.status(), StatusCode::OK);

    let presentation = request(
        &test.app,
        Method::PUT,
        &format!("/api/agents/{agent_id}/hooks/definitions/{owner}/{logical_id}"),
        Some(json!({"display_name": "Terminal notifier"})),
    )
    .await;
    assert_eq!(presentation.status(), StatusCode::OK);
    let presentation = read_json(presentation).await;
    assert_eq!(presentation["display_name"], "Terminal notifier");
    assert_eq!(presentation["latest_revision"], "v1");
    assert_eq!(presentation["active"], true);

    let revised = request(
        &test.app,
        Method::PUT,
        &format!("/api/agents/{agent_id}/hooks/definitions/{owner}/{logical_id}"),
        Some(json!({"args": ["hooks/notify-v2.sh", "--verbose"]})),
    )
    .await;
    assert_eq!(revised.status(), StatusCode::OK);
    let revised = read_json(revised).await;
    assert_eq!(revised["latest_revision"], "v2");
    assert_eq!(revised["active"], false);

    let detail = request(
        &test.app,
        Method::GET,
        &format!("/api/agents/{agent_id}/hooks/definitions/{owner}/{logical_id}"),
        None,
    )
    .await;
    assert_eq!(detail.status(), StatusCode::OK);
    let detail = read_json(detail).await;
    assert_eq!(detail["revisions"].as_array().unwrap().len(), 2);
    assert_eq!(detail["revisions"][0]["revision"], "v1");
    assert_eq!(
        detail["revisions"][0]["handler"]["args"],
        json!(["hooks/notify-v1.sh"])
    );
    assert_eq!(detail["revisions"][1]["revision"], "v2");
    assert_eq!(
        detail["revisions"][1]["handler"]["args"],
        json!(["hooks/notify-v2.sh", "--verbose"])
    );
}

#[tokio::test]
async fn user_api_rejects_native_fabrication_and_invalid_capabilities() {
    let test = build_app();
    let agent_id = "registry-invalid";

    let native = request(
        &test.app,
        Method::POST,
        &format!("/api/agents/{agent_id}/hooks/definitions"),
        Some(json!({
            "owner_namespace": "rua:native",
            "source": "native",
            "logical_id": "fabricated",
            "display_name": "Fabricated native",
            "event": "turn.terminal",
            "capabilities": ["identity"],
            "handler": {"kind": "native", "executable": "native", "available": true}
        })),
    )
    .await;
    assert_eq!(native.status(), StatusCode::BAD_REQUEST);

    let invalid = request(
        &test.app,
        Method::POST,
        &format!("/api/agents/{agent_id}/hooks/definitions"),
        Some(json!({
            "logical_id": "invalid-capability",
            "display_name": "Invalid capability",
            "event": "turn.terminal",
            "capabilities": ["tool_arguments"],
            "handler": {
                "kind": "command",
                "program": "/bin/sh",
                "args": ["hooks/invalid.sh"],
                "working_directory": "/workspace",
                "timeout_seconds": 30,
                "available": true
            }
        })),
    )
    .await;
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn availability_validation_and_activation_are_independent() {
    let test = build_app();
    let agent_id = "registry-unavailable";

    let created = request(
        &test.app,
        Method::POST,
        &format!("/api/agents/{agent_id}/hooks/definitions"),
        Some(json!({
            "logical_id": "offline-command",
            "display_name": "Offline command",
            "event": "tool.failed",
            "capabilities": ["identity", "redacted_diagnostics"],
            "handler": {
                "kind": "command",
                "program": "/bin/sh",
                "args": ["hooks/offline.sh"],
                "working_directory": "/workspace",
                "timeout_seconds": 30,
                "available": false
            }
        })),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let created = read_json(created).await;
    assert_eq!(created["availability"]["status"], "unavailable");
    assert_eq!(created["validation"]["status"], "valid");
    assert_eq!(created["active"], false);

    let activated = request(
        &test.app,
        Method::PUT,
        &format!("/api/agents/{agent_id}/hooks/defaults"),
        Some(json!({
            "definition_id": "agent:registry-unavailable/offline-command",
            "enabled": true
        })),
    )
    .await;
    assert_eq!(activated.status(), StatusCode::OK);
    let activated = read_json(activated).await;
    let definition = activated
        .as_array()
        .unwrap()
        .iter()
        .find(|definition| definition["logical_id"] == "offline-command")
        .unwrap();
    assert_eq!(definition["availability"]["status"], "unavailable");
    assert_eq!(definition["validation"]["status"], "valid");
    assert_eq!(definition["active"], true);
}

#[tokio::test]
async fn product_hooks_release_acceptance_spans_registration_adoption_turn_and_activity() {
    let test = build_app();
    let agent_id = "hook-tracer";
    assert_eq!(
        request(
            &test.app,
            Method::POST,
            "/api/agents",
            Some(json!({"id": agent_id, "name": "Hook tracer"}))
        )
        .await
        .status(),
        StatusCode::CREATED
    );

    let definitions = request(
        &test.app,
        Method::GET,
        &format!("/api/agents/{agent_id}/hooks/definitions"),
        None,
    )
    .await;
    assert_eq!(definitions.status(), StatusCode::OK);
    let definitions = read_json(definitions).await;
    assert_eq!(definitions[0]["registered"], true);
    assert_eq!(definitions[0]["active"], false);

    let old_chat =
        super::support::seed_materialized_chat(&test, agent_id, "before-activation").await;
    assert_eq!(
        old_chat["meta"]["hook_configuration"]["definition_revisions"],
        json!([])
    );

    let activated = request(
        &test.app,
        Method::PUT,
        &format!("/api/agents/{agent_id}/hooks/defaults"),
        Some(json!({"definition_id": DEFINITION_ID, "enabled": true})),
    )
    .await;
    assert_eq!(activated.status(), StatusCode::OK);
    assert_eq!(read_json(activated).await[0]["active"], true);

    let old_id = old_chat["id"].as_str().unwrap();
    let old_configuration = request(
        &test.app,
        Method::GET,
        &format!("/api/agents/{agent_id}/hooks/chats/{old_id}/configuration"),
        None,
    )
    .await;
    assert_eq!(
        read_json(old_configuration).await["definition_revisions"],
        json!([])
    );

    let configuration_path = format!("/api/agents/{agent_id}/chats/{old_id}/configuration");
    let preview = read_json(request(&test.app, Method::GET, &configuration_path, None).await).await;
    let updated = request(&test.app, Method::PUT, &configuration_path, Some(json!({
        "expected_epoch": preview["current"]["epoch"], "candidate_epoch": preview["candidate"]["epoch"],
    }))).await;
    assert_eq!(updated.status(), StatusCode::OK);
    let updated = read_json(updated).await;
    assert_eq!(
        updated["candidate"]["definition_revisions"],
        json!([format!("{DEFINITION_ID}@v1")])
    );

    let new_chat =
        super::support::seed_materialized_chat(&test, agent_id, "after-activation").await;
    assert_eq!(
        new_chat["meta"]["hook_configuration"]["definition_revisions"],
        json!([format!("{DEFINITION_ID}@v1")])
    );

    let chat_id = new_chat["id"].as_str().unwrap();
    let address = ChatAddress::agent(agent_id, chat_id);
    let turn = test
        .state
        .conversation_authority
        .start_active_turn(ConversationActiveTurnStart {
            address: address.clone(),
            driver: ConversationDriverKind::Rua,
            authority_device_id: "device-test".to_string(),
            authority_epoch: ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
            driver_detail: None,
        })
        .await;

    request(
        &test.app,
        Method::PUT,
        &format!("/api/agents/{agent_id}/hooks/defaults"),
        Some(json!({"definition_id": DEFINITION_ID, "enabled": false})),
    )
    .await;

    let key = StreamKey::new(CONSOLE_CHANNEL_ID, address.clone(), Priority::Normal);
    let (_producer_id, _cancel_rx) = test.state.streams.register_active_keyed(&key).await;
    test.state
        .product_hooks
        .record_contribution(
            rua_server::services::product_hooks::ProductHookContributionView {
                chat_address: address.clone(),
                turn_id: turn.turn_id.clone(),
                tool_call_id: "tool-acceptance".to_string(),
                event: "tool.after".to_string(),
                definition_revision: format!("{DEFINITION_ID}@v1"),
                source: "[\"agent\"]".to_string(),
                resolved_order: 0,
                kind: "feedback".to_string(),
                content: json!({"accepted": true}),
                diagnostic: None,
            },
        )
        .await;
    test.state
        .conversation_authority
        .complete_active_turn_and_drain_queue(
            test.state.streams.as_ref(),
            &key,
            &address,
            &turn.turn_id,
            "completed",
            Some("2026-08-24T00:00:00Z".to_string()),
            None,
            false,
            |_| async {},
        )
        .await
        .expect("complete active Turn");

    let runs = request(
        &test.app,
        Method::GET,
        &format!("/api/agents/{agent_id}/hooks/runs?chat_id={chat_id}"),
        None,
    )
    .await;
    assert_eq!(runs.status(), StatusCode::OK);
    let runs = read_json(runs).await;
    assert_eq!(runs.as_array().unwrap().len(), 1);
    assert_eq!(runs[0]["turn_id"], turn.turn_id);
    assert_eq!(runs[0]["status"], "completed");
    assert_eq!(runs[0]["decision_summary"], "terminal observed");
    assert_eq!(runs[0]["diagnostics"], json!([]));

    let contributions = request(
        &test.app,
        Method::GET,
        &format!(
            "/api/agents/{agent_id}/hooks/contributions?chat_id={chat_id}&turn_id={}",
            turn.turn_id
        ),
        None,
    )
    .await;
    assert_eq!(contributions.status(), StatusCode::OK);
    let contributions = read_json(contributions).await;
    assert_eq!(contributions.as_array().unwrap().len(), 1);
    assert_eq!(contributions[0]["kind"], "feedback");
    assert_eq!(
        contributions[0]["definition_revision"],
        format!("{DEFINITION_ID}@v1")
    );
}

async fn create_configuration_agent(app: &axum::Router, agent_id: &str) {
    let response = request(
        app,
        Method::POST,
        "/api/agents",
        Some(json!({"id":agent_id,"name":agent_id})),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
}

fn complete_materialization(
    agent_id: &str,
    authority_device_id: &str,
) -> ChatConfigurationMaterialization {
    ChatConfigurationMaterialization {
        selected_agent_id: agent_id.to_string(),
        running_config: Default::default(),
        security_config: Default::default(),
        agent_assignment: None,
        prompt_file_content: None,
        tool_config_snapshot: Default::default(),
        capability_configuration: None,
        model: None,
        thinking: None,
        collaboration_mode: json!("default"),
        harness_configuration: None,
        authority_device_id: authority_device_id.to_string(),
    }
}

fn pending_message(message: &str) -> ConversationInputPayload {
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

#[tokio::test]
async fn automation_inventory_is_adopted_only_by_target_chat_and_future_turns() {
    use mcp_manager::types::{McpConfig, McpServerId};
    let test = build_app();
    let agent = "automation-adoption";
    assert_eq!(
        request(
            &test.app,
            Method::POST,
            "/api/agents",
            Some(json!({"id":agent,"name":"Automation"}))
        )
        .await
        .status(),
        StatusCode::CREATED
    );
    let workspace = rua_core::RuaRoot::new(test.state.rua_root.clone())
        .agent_workspace(agent)
        .unwrap();
    let agent_file = workspace.as_path().join("agent.json");
    let mut agent_config: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&agent_file).unwrap()).unwrap();
    agent_config["mcp"] = json!({"clients":{"disabled-fixture":{"name":"fixture","enabled":false,"transport":"stdio","command":"unused","auto_connect":"manual"}}});
    rua_config::atomic_write_json(&agent_file, &agent_config).unwrap();
    let chat = super::support::seed_materialized_chat(&test, agent, "Adoption").await;
    let other = super::support::seed_materialized_chat(&test, agent, "Keep old tools").await;
    let id = chat["id"].as_str().unwrap();
    let address = ChatAddress::agent(agent, id);
    let before = test
        .state
        .product_hooks
        .chat_configuration(&address)
        .await
        .unwrap();
    let active = test
        .state
        .conversation_authority
        .start_active_turn(ConversationActiveTurnStart {
            address: address.clone(),
            driver: ConversationDriverKind::Rua,
            authority_device_id: "device-test".into(),
            authority_epoch: ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
            driver_detail: None,
        })
        .await;
    let manager = test.state.mcp_manager_for(agent).await.unwrap();
    let driver = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../rua-mcp/tests/fixtures/mock_mcp_server.py");
    let mut config = McpConfig::default();
    // A protocol fixture gives the HTTP path a changing real manager inventory;
    // driver-specific normal Chat dispatch is exercised in rua-agent tests.
    for name in ["rua_browser", "rua_computer"] {
        config.clients.insert(McpServerId::new(name),serde_json::from_value(json!({"name":name,"enabled":true,"transport":"stdio","command":"python3","args":[driver],"auto_connect":"lazy_on_first_use"})).unwrap());
    }
    manager.load_config(config).await.unwrap();
    let path = format!("/api/agents/{agent}/chats/{id}/configuration");
    let preview = read_json(request(&test.app, Method::GET, &path, None).await).await;
    let capabilities = &preview["candidate"]["capability_configuration"];
    for name in ["rua_browser", "rua_computer"] {
        assert_eq!(
            capabilities["mcp_servers"][name]["disabled_tools"],
            json!([]),
            "{preview}"
        );
    }
    assert!(capabilities["tools"].is_null());
    assert_eq!(capabilities["selection_policy"]["mode"], "default_enabled");
    assert!(manager
        .list_servers()
        .await
        .unwrap()
        .iter()
        .all(|server| server.state == mcp_manager::status::McpConnectionState::NotConnected));
    assert_eq!(
        test.state
            .product_hooks
            .chat_configuration(&address)
            .await
            .unwrap(),
        before
    );
    let applied=request(&test.app,Method::PUT,&path,Some(json!({"expected_epoch":preview["current"]["epoch"],"candidate_epoch":preview["candidate"]["epoch"]}))).await;
    assert_eq!(applied.status(), StatusCode::OK);
    let current = test
        .state
        .product_hooks
        .chat_configuration(&address)
        .await
        .unwrap();
    assert_eq!(serde_json::to_value(current).unwrap(), preview["candidate"]);
    let frozen = test
        .state
        .product_hooks
        .frozen_turn_configuration(&address, &active.turn_id)
        .await
        .unwrap();
    assert_eq!(frozen.epoch, before.epoch);
    assert_eq!(frozen.security_config, before.security_config);
    assert_eq!(
        frozen.capability_configuration,
        before.capability_configuration
    );
    let other_address = ChatAddress::agent(agent, other["id"].as_str().unwrap());
    assert_eq!(
        serde_json::to_value(
            test.state
                .product_hooks
                .chat_configuration(&other_address)
                .await
                .unwrap()
        )
        .unwrap(),
        other["meta"]["hook_configuration"]
    );
    manager.shutdown_all().await.unwrap();
}
