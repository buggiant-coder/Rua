use std::sync::Arc;
use std::time::Duration;

use axum::{
    http::{Method, StatusCode},
    Router,
};
use rua_contract::Tool;
use rua_core::ChatAddress;
use rua_session::{RuntimeNote, RuntimeNotePayload, WorkspaceSessionStore};
use rua_task::{
    TaskAdoptedExecution, TaskAttemptAuthorization, TaskOwner, TaskRunOutput, TaskRunnerSpec,
    TaskSpec, TaskState,
};
use rua_tool::extended::task_tools::{TaskCreateTool, TaskRuntimeHandle};
use serde_json::{json, Value};

use super::support::{
    build_app, json as response_json, local_device_id, request, wait_for_chat_idle,
};

async fn call(app: &Router, method: Method, path: &str, body: Option<Value>) -> Value {
    let response = request(app, method, path, body).await;
    let status = response.status();
    let body = response_json(response).await;
    assert!(status.is_success(), "{path} returned {status}: {body}");
    body
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn task_retains_original_executor_and_cwd_while_result_and_approval_follow_the_chat() {
    let test = build_app();
    let state = &test.state;
    let tasks = &state.task_runtime;
    let old = state.registry.get_or_create("default").await.unwrap();
    let mut policy = state.security.config_store().load("default").unwrap();
    policy.tool_guard.approval_mode = "ask_first".into();
    state
        .security
        .save_and_invalidate("default", &policy)
        .unwrap();
    call(
        &test.app,
        Method::POST,
        "/api/agents",
        Some(json!({"id":"next","name":"Next"})),
    )
    .await;
    let external = tempfile::tempdir().unwrap();
    let device = local_device_id(state);
    let id = uuid::Uuid::new_v4().to_string();
    let chat = format!("/api/chats/{id}");
    let selection = format!("{chat}/agent-selection");
    let address = ChatAddress::new(id.as_str());
    call(
        &test.app,
        Method::POST,
        &format!("{chat}/console/submit"),
        Some(json!({
            "message":"Open standalone Chat", "agent_id":"next"
        })),
    )
    .await;
    wait_for_chat_idle(state, &id).await;
    // The original Task executor and the Chat execution directory deliberately differ.
    call(
        &test.app,
        Method::PATCH,
        &selection,
        Some(json!({"expected_agent_id":"next","agent_id":"default"})),
    )
    .await;
    call(
        &test.app,
        Method::POST,
        &format!("{chat}/console/submit"),
        Some(json!({"message":"Create a background edit"})),
    )
    .await;
    wait_for_chat_idle(state, &id).await;
    let store = WorkspaceSessionStore::new(state.chat_workspace().as_path());
    let header = store.load_chat_header(&id).await.unwrap().unwrap();
    let original: rua_session::ChatExecutionDirectory =
        serde_json::from_value(header.extra["execution_directory"].clone()).unwrap();
    assert!(original.managed);
    assert_eq!(original.device_id, device);
    let execution_dir = original.path.clone();
    assert_ne!(execution_dir, old.workspace_dir());
    let origin_turn = store
        .read_turn_range(&id, 2, 1)
        .await
        .unwrap()
        .remove(0)
        .turn_id;
    let config = state
        .product_hooks
        .chat_configuration(&address)
        .await
        .unwrap();
    assert_eq!(config.security_config.tool_guard.approval_mode, "ask_first");
    let owner = TaskOwner::from_address(address.clone());

    // Keep the real edit Task waiting on a dependency until after the switch.
    let (release, waiting) = tokio::sync::oneshot::channel::<()>();
    let dependency = tasks
        .adopt_running(
            uuid::Uuid::new_v4().to_string(),
            TaskSpec {
                owner: owner.clone(),
                instruction: "Wait for fixture".into(),
                runner: TaskRunnerSpec::Tool {
                    tool_name: "fixture".into(),
                    arguments: json!({}),
                },
                origin: Default::default(),
                depends_on: vec![],
                context_refs: vec![],
            },
            TaskAttemptAuthorization::new("default")
                .unwrap()
                .with_configuration_epoch(config.epoch.clone()),
            TaskAdoptedExecution::new(
                async {
                    let _ = waiting.await;
                    TaskRunOutput::completed("ready")
                },
                Default::default(),
            ),
        )
        .await
        .unwrap();
    let context = rua_core::tool::ToolContext {
        agent_id: "default".into(),
        session_id: id.clone(),
        working_dir: execution_dir.clone(),
        metadata: std::collections::HashMap::from([
            ("turn_id".into(), json!(origin_turn)),
            ("tool_use_id".into(), json!("original-call")),
            ("chat_configuration_epoch".into(), json!(config.epoch)),
        ]),
        task_runtime: Some(Arc::new(TaskRuntimeHandle::new(tasks.clone()))),
        ..Default::default()
    };
    let name = "old-task-output.txt";
    let created = TaskCreateTool.call(json!({"depends_on":[dependency], "runner":{
        "type":"tool", "tool_name":"edit", "arguments":{"patch":format!("*** Begin Patch\n*** Add File: {name}\n+from original Agent\n*** End Patch")}
    }}),context).await.unwrap();
    let task: Value = serde_json::from_str(&created.text_content()).unwrap();
    let task_id = task["task_id"].as_str().unwrap();
    let before = tasks.get(&owner, task_id).await.unwrap();
    let attempt = &before.attempts[0];
    assert_eq!(attempt.authorization_agent_id, "default");
    assert_eq!(
        attempt.configuration_epoch.as_deref(),
        Some(config.epoch.as_str())
    );

    let switched = call(
        &test.app,
        Method::PATCH,
        &selection,
        Some(json!({"expected_agent_id":"default","agent_id":"next"})),
    )
    .await;
    assert_eq!(switched["selection"]["agent_id"], "next");
    let changed = state
        .conversation_input_intake
        .set_execution_directory(&address, &device, Some(external.path()))
        .await
        .unwrap();
    assert_eq!(changed.path, external.path());
    assert!(!changed.managed);
    release.send(()).unwrap();
    let pending = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let snapshot = call(
                &test.app,
                Method::GET,
                &format!("{chat}/approvals/snapshot"),
                None,
            )
            .await;
            if let Some(request) = snapshot["pending"]
                .as_array()
                .unwrap()
                .iter()
                .find(|r| r["tool_call_id"] == format!("task:{task_id}:attempt:1:tool"))
            {
                break request.clone();
            }
            let status = tasks.get(&owner, task_id).await.unwrap();
            assert!(
                !status.state.is_terminal(),
                "Task exited before approval: {status:?}"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("original Task registers approval after Agent switch");
    assert_eq!(pending["agent_id"], "default");
    assert_eq!(pending["owner"]["address"]["chat_id"], id);
    assert!(!execution_dir.join(name).exists());
    wait_for_chat_idle(state, &id).await;
    let blocked = request(
        &test.app,
        Method::PATCH,
        &selection,
        Some(json!({"expected_agent_id":"next","agent_id":"default"})),
    )
    .await;
    let status = blocked.status();
    let body = response_json(blocked).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(
        body["detail"]
            .as_str()
            .unwrap()
            .contains("unresolved_approval"),
        "{body}"
    );
    let blocked_directory = state
        .conversation_input_intake
        .set_execution_directory(&address, &device, None)
        .await
        .unwrap_err();
    assert!(
        blocked_directory.contains("unresolved_approval"),
        "{blocked_directory}"
    );
    let approvals = state.security.approval_authority();
    let old_rules = approvals.list_agent_rules("default").await.len();
    let next_rules = approvals.list_agent_rules("next").await.len();
    let option = pending["reuse_options"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["scope"] == "agent")
        .expect("Agent-scoped approval option");
    let decided = call(
        &test.app,
        Method::POST,
        &format!(
            "{chat}/approvals/{}/decide",
            pending["id"].as_str().unwrap()
        ),
        Some(json!({
            "decision":"allow_agent","decided_by":"fixture",
            "matcher":option["matcher"]
        })),
    )
    .await;
    assert_eq!(decided["agent_id"], "default");
    assert_eq!(
        approvals.list_agent_rules("default").await.len(),
        old_rules + 1
    );
    assert_eq!(approvals.list_agent_rules("next").await.len(), next_rules);
    // A native channel callback reaches the selected Agent's facade, while the
    // request and its decision still belong to the Agent that issued it.
    let mut callback: rua_security::approval_authority::ApprovalRequestInput =
        serde_json::from_value(pending).unwrap();
    callback.tool_call_id = "native-callback".into();
    let callback = approvals.create_request(callback).await;
    assert!(state
        .console_facade
        .decide_chat_approval(&address, "next", Some(&callback.id), false, "native-user")
        .await
        .unwrap());
    let resolved = approvals.request(&callback.id).await.unwrap();
    assert_eq!(resolved.agent_id, "default");
    assert_eq!(resolved.decided_by.as_deref(), Some("native-user"));
    assert_eq!(
        resolved.status,
        rua_security::approval_authority::ApprovalStatus::Rejected
    );
    let record = tasks
        .await_terminal(&owner, task_id, Duration::from_secs(12))
        .await
        .unwrap();
    assert_eq!(record.state, TaskState::Completed, "{:?}", record.result);
    assert_eq!(
        record.spec.origin.turn_id.as_deref(),
        Some(origin_turn.as_str())
    );
    assert_eq!(record.attempts[0].device, attempt.device);
    assert_eq!(
        record.attempts[0].configuration_epoch,
        attempt.configuration_epoch
    );
    assert_eq!(
        record.attempts[0].authorization_agent_id,
        attempt.authorization_agent_id
    );
    assert_eq!(
        std::fs::read_to_string(execution_dir.join(name))
            .unwrap()
            .trim(),
        "from original Agent"
    );
    assert!(!old.workspace_dir().join(name).exists());
    assert!(!external.path().join(name).exists());
    let result_turn = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let messages = store.read_message_range(&id, 1, usize::MAX).await.unwrap();
            if let Some(turn_id) = messages.iter().find_map(|m| {
                match RuntimeNote::from_message_log_entry(m)?.payload {
                    RuntimeNotePayload::TaskAttemptTerminal(n)
                        if n.task_id == task_id
                            && n.attempt_no == 1
                            && n.tool_call_id.as_deref() == Some("original-call") =>
                    {
                        assert_eq!(n.execution_directory.as_deref(), Some(&original));
                        Some(m.turn_id.clone())
                    }
                    _ => None,
                }
            }) {
                break turn_id;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the completed Task result reaches the original Chat");
    let header = store.load_chat_header(&id).await.unwrap().unwrap();
    assert_eq!(header.extra["execution_directory"], json!(changed));
    let turns = store.read_turn_range(&id, 1, usize::MAX).await.unwrap();
    let result = turns.iter().find(|t| t.turn_id == result_turn).unwrap();
    assert_eq!(result.metadata["turn_agent_assignment"]["agent_id"], "next");
    wait_for_chat_idle(state, &id).await;
    let restored = state
        .conversation_input_intake
        .set_execution_directory(&address, &device, None)
        .await
        .unwrap();
    assert_eq!(restored, original);
    let history = call(
        &test.app,
        Method::GET,
        &format!("{chat}?turn_limit=20"),
        None,
    )
    .await;
    assert_eq!(history["meta"]["execution_directory"], json!(restored));
    let metadata = call(&test.app, Method::GET, &format!("{chat}/metadata"), None).await;
    assert_eq!(metadata["meta"]["execution_directory"], json!(restored));
    assert_eq!(
        std::fs::read_to_string(restored.path.join(name))
            .unwrap()
            .trim(),
        "from original Agent"
    );
    watch_arriving_during_switch_uses_the_committed_agent(state, &address, config).await;
}

async fn watch_arriving_during_switch_uses_the_committed_agent(
    state: &rua_server::AppState,
    address: &ChatAddress,
    mut next_config: rua_server::services::product_hooks::ChatHookConfiguration,
) {
    let root = state.chat_workspace();
    let change = rua_server::services::project_chat::ChatAgentSwitch::begin(
        root.as_path(),
        address,
        "next",
        "default",
        &state.conversation_authority,
        state.security.approval_authority(),
    )
    .await
    .unwrap();
    let note = rua_task::TaskWatchOutputNotification {
        extra: Default::default(),
        notification_id: "switching-watch".into(),
        owner: TaskOwner::from_address(address.clone()),
        task_id: "fixture-watch".into(),
        attempt_no: 1,
        origin_turn_id: None,
        projection: rua_task::TaskLiveOutputProjection::ModelVisible,
        records: vec![rua_task::TaskLiveOutputRecord {
            source: "stdout".into(),
            text: "progress".into(),
        }],
    };
    let intake = &state.conversation_input_intake;
    // An overlapping delivery of the same notification is rejected after the
    // first registers. This places its admission inside the switch deterministically.
    let delivery = intake.deliver_task_watch_output(note.clone());
    let duplicate = intake.deliver_task_watch_output(note);
    tokio::pin!(delivery, duplicate);
    let (early, remaining) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::select! {
            result = &mut delivery => (result, &mut duplicate),
            result = &mut duplicate => (result, &mut delivery),
        }
    })
    .await
    .expect("the second delivery detects the registered watch");
    assert!(early.unwrap_err().contains("already registered"));
    next_config.epoch = uuid::Uuid::new_v4().to_string();
    change
        .commit(&next_config, &local_device_id(state))
        .await
        .unwrap();
    state
        .product_hooks
        .adopt_chat_configuration(address, next_config)
        .await;
    drop(change);
    tokio::time::timeout(Duration::from_secs(10), remaining)
        .await
        .expect("the new executor finishes the waiting Watch Turn")
        .unwrap();

    let id = address.chat_id().as_str();
    let store = WorkspaceSessionStore::new(root.as_path());
    let messages = store.read_message_range(id, 1, usize::MAX).await.unwrap();
    let turn_id = &messages
        .iter()
        .find(|m| {
            RuntimeNote::from_message_log_entry(m)
                .is_some_and(|n| n.runtime_note_id == "switching-watch")
        })
        .expect("admitted watch has a durable Turn")
        .turn_id;
    let turns = store.read_turn_range(id, 1, usize::MAX).await.unwrap();
    let turn = turns.iter().find(|t| &t.turn_id == turn_id).unwrap();
    assert_eq!(
        turn.metadata["turn_agent_assignment"]["agent_id"],
        "default"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn managed_directories_are_isolated_and_created_only_for_accepted_chats() {
    use rua_server::services::conversation_authority::{
        ConversationActiveTurnStart, ConversationDriverKind, ConversationInputPayload,
    };
    use rua_server::services::conversation_authority_placement::parse_conversation_authority_placement;

    let test = build_app();
    let state = &test.state;
    let device = local_device_id(state);
    let store = WorkspaceSessionStore::new(state.chat_workspace().as_path());
    let empty = uuid::Uuid::new_v4().to_string();
    let rejected = state
        .conversation_input_intake
        .submit_delivery(
            "default",
            rua_channel::core::console::ConsoleChatRequest {
                message: String::new(),
                chat_id: Some(empty.clone()),
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
    assert!(matches!(rejected,rua_server::services::conversation_input_intake::ConversationInputDeliveryOutcome::Rejected{..}),"{rejected:?}");
    assert!(store.load_chat_header(&empty).await.unwrap().is_none());
    assert!(!state
        .rua_root
        .join("chat-workdirs")
        .join(&device)
        .join(&empty)
        .exists());

    let mut directories = Vec::new();
    for _ in 0..2 {
        let id = uuid::Uuid::new_v4().to_string();
        call(
            &test.app,
            Method::POST,
            &format!("/api/chats/{id}/console/submit"),
            Some(json!({"message":"Create a managed Chat","agent_id":"default"})),
        )
        .await;
        wait_for_chat_idle(state, &id).await;
        let header = store.load_chat_header(&id).await.unwrap().unwrap();
        let directory: rua_session::ChatExecutionDirectory =
            serde_json::from_value(header.extra["execution_directory"].clone()).unwrap();
        assert!(directory.managed);
        assert!(directory.path.is_dir());
        directories.push((ChatAddress::new(id), directory, header));
    }
    let (address, first, header) = &directories[0];
    let (_, second, _) = &directories[1];
    assert_ne!(first.path, second.path);
    std::fs::write(first.path.join("fixture"), "first").unwrap();
    assert!(!second.path.join("fixture").exists());
    let missing = first.path.join("missing");
    assert!(state
        .conversation_input_intake
        .set_execution_directory(address, &device, Some(&missing))
        .await
        .is_err());
    assert!(!missing.exists());
    assert!(state
        .conversation_input_intake
        .set_execution_directory(address, "another-paired-device", None)
        .await
        .is_err());

    let placement =
        parse_conversation_authority_placement(&Value::Object(header.extra.clone())).unwrap();
    let authority = &state.conversation_authority;
    authority
        .start_active_turn(ConversationActiveTurnStart {
            address: address.clone(),
            driver: ConversationDriverKind::Rua,
            driver_detail: None,
            authority_device_id: device.clone(),
            authority_epoch: placement.authority_epoch,
        })
        .await;
    authority
        .enqueue_input(
            address,
            ConversationInputPayload {
                message: "queued".into(),
                model_override: None,
                input: None,
                messages: None,
                extra: Default::default(),
                attachments: vec![],
                enqueued_at: std::time::Instant::now(),
            },
            None,
        )
        .await
        .unwrap();
    let blocked = state
        .conversation_input_intake
        .set_execution_directory(address, &device, Some(&second.path))
        .await
        .unwrap_err();
    assert!(
        blocked.contains("active_turn") && blocked.contains("queued_input"),
        "{blocked}"
    );
    authority.finish_active_turn(address).await;
    let blocked = state
        .conversation_input_intake
        .set_execution_directory(address, &device, Some(&second.path))
        .await
        .unwrap_err();
    assert!(blocked.contains("queued_input"), "{blocked}");
    let persisted = store
        .load_chat_header(address.chat_id().as_str())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(persisted.extra["execution_directory"], json!(first));
    authority.discard_unclaimed_inputs(address).await;
}
