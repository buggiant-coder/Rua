use super::handoff::{Peer, PersistingCaptureRuntime};
use super::*;
use rua_security::{ApprovalReuseMatcher, PermissionRuleEffect, PermissionRuleInput};
use rua_sync::identity::NetworkSecret;
use rua_task::{
    TaskAttemptAuthorization, TaskNotificationSink, TaskOrigin, TaskOwner, TaskSpec, TaskState,
};
use serde_json::{json, Value};

async fn record_at(
    peer: &Peer,
    owner: &TaskOwner,
    id: &str,
    attempt: u32,
    state: TaskState,
) -> rua_task::TaskRecord {
    let result = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let record = peer.state.task_runtime.get(owner, id).await.unwrap();
            if record.attempt_no == attempt && record.state == state {
                break record;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    match result {
        Ok(record) => record,
        Err(error) => {
            let actual = peer.state.task_runtime.get(owner, id).await.unwrap();
            let latest = actual.attempts.last().unwrap();
            let snapshot = peer
                .store()
                .load_chat_header(owner.chat_id())
                .await
                .ok()
                .flatten()
                .and_then(|header| header.extra.get("hook_configuration").cloned());
            let authority = peer.state.security.approval_authority();
            panic!("Task {id} did not reach attempt {attempt} {state:?}: {error}; actual state={:?}, attempt={:?}, snapshot={:?}, approvals={:?}, live Chat rules={:?}",
                actual.state, latest,
                snapshot.map(|config| json!({"epoch":config["epoch"], "agent":config["selected_agent_id"], "security":config["security_config"]})),
                authority.snapshot_at(&owner.chat_address).await,
                authority.list_chat_rules(&owner.chat_address).await);
        }
    }
}

async fn note_count(peer: &Peer, chat: &str, id: &str, attempt: u32) -> usize {
    let expected = format!("task:{id}:attempt:{attempt}");
    let store = peer.store();
    let mut count = 0;
    for turn in store.read_turn_range(chat, 1, usize::MAX).await.unwrap() {
        count += store
            .read_runtime_notes_for_turn(chat, &turn.turn_id)
            .await
            .unwrap()
            .iter()
            .filter(|note| note.runtime_note_id == expected)
            .count();
    }
    count
}

async fn delivered(
    executor: &Peer,
    current: &Peer,
    owner: &TaskOwner,
    id: &str,
    attempt: u32,
    calls: usize,
) {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if executor
                .state
                .task_runtime
                .pending_terminal_deliveries(Some(owner))
                .await
                .unwrap()
                .is_empty()
                && current.captured.requests.lock().await.len() == calls
                && current
                    .state
                    .conversation_authority
                    .active_turn(&owner.chat_address)
                    .await
                    .is_none()
                && note_count(current, owner.chat_id(), id, attempt).await == 1
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the original executor must deliver exactly one durable result to the new Authority");
    current.idle(owner.chat_id()).await;
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn handed_off_chat_controls_original_remote_task_and_retries_with_current_authorization() {
    let secret = NetworkSecret::generate();
    let source = Peer::new(&secret).await;
    let executor = Peer::new(&secret).await;
    let current = Peer::new(&secret).await;
    source.pair(&[&executor, &current]);
    executor.pair(&[&source, &current]);
    current.pair(&[&source, &executor]);
    let chat = uuid::Uuid::new_v4().to_string();
    let owner = TaskOwner::new(&chat);
    let cwd = executor
        .state
        .rua_root
        .canonicalize()
        .unwrap()
        .join("original-task-cwd");
    std::fs::create_dir(&cwd).unwrap();
    let args = json!({"device_id":executor.device_id,"cwd":cwd,
        "command":"printf x >> starts; while [ ! -f finish ]; do sleep 0.05; done; printf finished"});
    // The first Agent's captured rule names this exact command and execution
    // Device. The next Agent has no such grant and uses Ask First.
    source
        .state
        .security
        .approval_authority()
        .create_agent_rule(
            "default",
            PermissionRuleInput {
                effect: PermissionRuleEffect::Allow,
                enabled: true,
                label: "Run the original Task".into(),
                matcher: ApprovalReuseMatcher::ToolCall {
                    tool_name: "bash".into(),
                    input_params: Some(args.to_string()),
                },
            },
        )
        .await
        .unwrap();
    source
        .submit(&chat, "manage the Task on the other Device")
        .await;
    executor.synchronize(&source).await;
    let origin = TaskOrigin {
        turn_id: Some(
            source.store().read_turn_range(&chat, 1, 1).await.unwrap()[0]
                .turn_id
                .clone(),
        ),
        tool_call_id: Some("original-task-call".into()),
    };
    // Keep the real Workspace Tool runner and SecurityAgentHook installed.
    let id = executor
        .state
        .task_runtime
        .create(
            TaskSpec {
                owner: owner.clone(),
                instruction: "wait in the original execution directory".into(),
                runner: rua_task::TaskRunnerSpec::Tool {
                    tool_name: "bash".into(),
                    arguments: args.clone(),
                },
                context_refs: vec![],
                depends_on: vec![],
                origin: origin.clone(),
            },
            TaskAttemptAuthorization::new("default")
                .unwrap()
                .with_origin(origin.clone()),
        )
        .await
        .unwrap();
    let running = record_at(&executor, &owner, &id, 1, TaskState::Running).await;
    tokio::time::timeout(Duration::from_secs(8), async {
        while std::fs::read_to_string(cwd.join("starts")).ok().as_deref() != Some("x") {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the real first command must start on B");
    rua_server::services::chat_task::capture_task_reference(
        &executor.state.rua_root,
        &executor.device_id,
        &running,
    )
    .unwrap();
    let publication = rua_server::sync_publication::publish_sync_fact(
        &executor.state,
        rua_server::sync_publication::SyncPublicationFact::StableConversationState {
            scope_root: executor.state.chat_workspace().as_path().to_path_buf(),
            address: owner.chat_address.clone(),
        },
    )
    .await;
    assert!(publication.errors.is_empty(), "{:?}", publication.errors);
    source.synchronize(&executor).await;
    let (status, handoff) = source
        .request(
            Method::PUT,
            &chat,
            "/authority-placement",
            json!({"device_id":current.device_id}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{handoff}");
    executor.synchronize(&current).await;
    assert_eq!(current.placement(&chat).await.device_id, current.device_id);
    let (status, queried) = current
        .request(Method::GET, &chat, &format!("/tasks/{id}"), Value::Null)
        .await;
    assert_eq!(status, StatusCode::OK, "{queried}");
    assert_eq!(queried["task_id"], id);
    assert_eq!(queried["state"], "running");
    assert_eq!(
        executor
            .state
            .task_runtime
            .get(&owner, &id)
            .await
            .unwrap()
            .attempts[0]
            .device,
        running.attempts[0].device
    );
    for peer in [&source, &current] {
        assert!(
            peer.state
                .task_runtime
                .list(&owner, Default::default())
                .await
                .unwrap()
                .is_empty(),
            "handoff must not create a Task copy"
        );
    }
    let (status, cancelled) = current
        .request(
            Method::POST,
            &chat,
            &format!("/tasks/{id}/cancel"),
            Value::Null,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{cancelled}");
    let first = record_at(&executor, &owner, &id, 1, TaskState::Cancelled).await;
    delivered(&executor, &current, &owner, &id, 1, 1).await;
    assert_eq!(
        source.captured.requests.lock().await.len(),
        1,
        "the old Authority must not process the result"
    );
    assert_eq!(executor.captured.requests.lock().await.len(), 0);
    // Re-deliver the same real terminal record, as after a lost receipt.
    let sink = rua_server::services::task_notification::ServerTaskNotificationSink::new(
        &executor.state.rua_root,
    );
    sink.install_local_device_id(executor.device_id.clone());
    sink.install_tasks(&executor.state.task_runtime);
    sink.install_watch_delivery(executor.state.conversation_input_intake.clone());
    tokio::time::timeout(
        Duration::from_secs(15),
        sink.terminal(rua_task::TaskCompletionNotification {
            record: first.clone(),
        }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(note_count(&current, &chat, &id, 1).await, 1);
    assert_eq!(current.captured.requests.lock().await.len(), 1);
    drop(sink);

    let next_agent = "handoff-next-agent";
    let response = rua_server::router::build_app(current.state.clone(), None)
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/agents")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"id":next_agent,"name":"Next Task Agent"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let token = super::approved_phone_file::logged_in(&current.state, "handoff-owner").await;
    super::approved_phone_file::request(
        &current.state,
        Method::PUT,
        &format!("/api/agents/{next_agent}/security/approval-mode"),
        json!({"approval_mode":"ask_first"}),
        Some(&token),
    )
    .await;
    current
        .state
        .registry
        .get_or_create(next_agent)
        .await
        .unwrap()
        .set_react_runtime(Arc::new(PersistingCaptureRuntime(current.captured.clone())))
        .await;
    let (status, selection) = current
        .request(
            Method::PATCH,
            &chat,
            "/agent-selection",
            json!({"expected_agent_id":"default","agent_id":next_agent}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{selection}");
    let current_cwd = current
        .state
        .rua_root
        .canonicalize()
        .unwrap()
        .join("new-chat-cwd");
    std::fs::create_dir(&current_cwd).unwrap();
    let (status, directory) = current
        .request(
            Method::PATCH,
            &chat,
            "/execution-directory",
            json!({"device_id":current.device_id,"path":current_cwd}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{directory}");
    let current_header = current
        .store()
        .load_chat_header(&chat)
        .await
        .unwrap()
        .unwrap();
    let current_epoch = current_header.extra["hook_configuration"]["epoch"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        current_header.extra["hook_configuration"]["security_config"]["tool_guard"]
            ["approval_mode"],
        "ask_first"
    );
    // Agent/cwd updates publish after the HTTP commit. Require the actual
    // current Chat snapshot on B before submitting the one Retry operation.
    let synchronized = tokio::time::timeout(Duration::from_secs(25), async {
        loop {
            executor.synchronize(&current).await;
            let header = executor
                .store()
                .load_chat_header(&chat)
                .await
                .unwrap()
                .unwrap();
            if header.extra["hook_configuration"] == current_header.extra["hook_configuration"]
                && executor.placement(&chat).await == current.placement(&chat).await
                && executor
                    .state
                    .inbound_inbox
                    .diagnostics(8)
                    .unwrap()
                    .counts
                    .retryable
                    == 0
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    if let Err(error) = synchronized {
        let header = executor
            .store()
            .load_chat_header(&chat)
            .await
            .unwrap()
            .unwrap();
        panic!("Retry configuration not applied: {error}; target agent={}, epoch={}, expected agent={next_agent}, epoch={current_epoch}, inbox={:?}",
            header.agent_id, header.extra["hook_configuration"]["epoch"], executor.state.inbound_inbox.diagnostics(8));
    }
    rua_config::atomic_write(&cwd.join("finish"), b"ready").unwrap();
    let (status, retry) = current
        .request(
            Method::POST,
            &chat,
            &format!("/tasks/{id}/retry"),
            Value::Null,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{retry}");
    assert_eq!(retry["task_id"], id);
    assert_eq!(retry["attempt_no"], 2);
    let waiting = record_at(&executor, &owner, &id, 2, TaskState::WaitingApproval).await;
    assert_eq!(
        std::fs::read_to_string(cwd.join("starts")).unwrap(),
        "x",
        "the new Ask First snapshot must guard the actual command"
    );
    assert!(!current_cwd.join("starts").exists());
    assert_eq!(waiting.attempts[1].authorization_agent_id, next_agent);
    assert_eq!(
        waiting.attempts[1].device.device_id,
        first.attempts[0].device.device_id
    );
    assert_eq!(
        waiting.attempts[1].device.public_cwd,
        first.attempts[0].device.public_cwd
    );
    assert_eq!(waiting.spec.origin, origin);
    let snapshot = super::approved_phone_file::request(
        &current.state,
        Method::GET,
        &format!("/api/chats/{chat}/approvals/snapshot"),
        Value::Null,
        Some(&token),
    )
    .await;
    let card = snapshot["pending"]
        .as_array()
        .unwrap()
        .iter()
        .find(|card| card["tool_call_id"] == format!("task:{id}:attempt:2:tool"))
        .expect("C must show B's actual Task approval");
    assert_eq!(card["agent_id"], next_agent);
    let approved = super::approved_phone_file::request(
        &current.state,
        Method::POST,
        &format!(
            "/api/chats/{chat}/approvals/{}/decide",
            card["id"].as_str().unwrap()
        ),
        json!({"decision":"run_once","decided_by":"webui"}),
        Some(&token),
    )
    .await;
    assert_eq!(approved["status"], "approved");
    let completed = record_at(&executor, &owner, &id, 2, TaskState::Completed).await;
    assert_eq!(
        completed.attempts[1].authorization_origin,
        TaskOrigin::default()
    );
    let approvals = executor
        .state
        .security
        .approval_authority()
        .snapshot_at(&owner.chat_address)
        .await;
    assert!(
        approvals.pending.is_empty(),
        "no Task approval may be left pending after execution"
    );
    assert!(approvals
        .resolved
        .iter()
        .any(|resolved| resolved.id == card["id"].as_str().unwrap()
            && resolved.status == rua_security::AuthorityStatus::Approved));
    delivered(&executor, &current, &owner, &id, 2, 2).await;
    assert_eq!(std::fs::read_to_string(cwd.join("starts")).unwrap(), "xx");
    assert!(!current_cwd.join("starts").exists());
    assert_eq!(
        completed.attempts[1].device.device_id,
        first.attempts[0].device.device_id
    );
    assert_eq!(
        completed.attempts[1].device.public_cwd,
        first.attempts[0].device.public_cwd
    );
    assert_eq!(completed.spec.origin, first.spec.origin);
    assert_eq!(
        current.captured.requests.lock().await[1].agent_id.as_str(),
        next_agent
    );
    assert_eq!(source.captured.requests.lock().await.len(), 1);
    for peer in [&source, &current] {
        assert!(peer
            .state
            .task_runtime
            .list(&owner, Default::default())
            .await
            .unwrap()
            .is_empty());
    }
    executor.state.task_runtime.shutdown().await.unwrap();
    source.close().await;
    executor.close().await;
    current.close().await;
}
