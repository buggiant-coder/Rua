use super::*;

async fn complete_pending(
    tasks: &Arc<rua_task::TaskRuntime>,
    address: &ChatAddress,
    turn_id: String,
) -> String {
    let owner = rua_task::TaskOwner::from_address(address.clone());
    let id = tasks
        .create(
            rua_task::TaskSpec {
                owner: owner.clone(),
                instruction: "recover terminal without starving another Chat".into(),
                runner: rua_task::TaskRunnerSpec::Tool {
                    tool_name: "fixture".into(),
                    arguments: serde_json::json!({}),
                },
                origin: rua_task::TaskOrigin {
                    turn_id: Some(turn_id),
                    tool_call_id: Some("recovery-origin".into()),
                },
                context_refs: vec![],
                depends_on: vec![],
            },
            rua_task::TaskAttemptAuthorization::new("default").unwrap(),
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while !tasks.get(&owner, &id).await.unwrap().state.is_terminal()
            || tasks
                .pending_terminal_deliveries(Some(&owner))
                .await
                .unwrap()
                .is_empty()
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    id
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn blocked_chat_result_keeps_its_outbox_without_starving_later_chat_recovery() {
    let mut fixture = RemoteInputConfirmationFixture::new(Duration::ZERO, false).await;
    let busy_release = Arc::new(Notify::new());
    let (busy, busy_turn, _) = prepare_running_file_calls(
        &fixture,
        &[ToolCall::new(
            "busy-origin",
            "read",
            serde_json::json!({"file_path":"/fixture"}),
        )],
        &busy_release,
    )
    .await;
    fixture.chat_id = uuid::Uuid::new_v4().to_string();
    fixture.submission_id = uuid::Uuid::new_v4().to_string();
    let ready_release = Arc::new(Notify::new());
    let (ready, ready_turn, _) = prepare_running_file_calls(
        &fixture,
        &[ToolCall::new(
            "ready-origin",
            "read",
            serde_json::json!({"file_path":"/fixture"}),
        )],
        &ready_release,
    )
    .await;
    ready_release.notify_one();
    tokio::time::timeout(Duration::from_secs(5), async {
        while fixture
            .target_state
            .conversation_authority
            .active_turn(&ready)
            .await
            .is_some()
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let captured = Arc::new(CapturingAgentRuntime::default());
    fixture
        .target_state
        .registry
        .get_or_create("default")
        .await
        .unwrap()
        .set_react_runtime(captured.clone())
        .await;
    let sink = Arc::new(
        rua_server::services::task_notification::ServerTaskNotificationSink::new(
            fixture.source_state.rua_root.clone(),
        ),
    );
    sink.install_local_device_id(fixture.source.device_id.clone());
    let data = TempDir::new().unwrap();
    let mut config = rua_task::TaskRuntimeConfig::new(data.path());
    config.device.device_id = Some(fixture.source.device_id.clone());
    config.device.public_cwd = Some(fixture.source_state.rua_root.to_string_lossy().into_owned());
    let tasks = Arc::new(rua_task::TaskRuntime::open(config).unwrap());
    sink.install_tasks(&tasks);
    tasks.register_runner(Arc::new(CompletionRunner));
    // Delivery is intentionally unavailable until both durable records exist.
    let busy_id = complete_pending(&tasks, &busy, busy_turn.turn_id).await;
    let ready_id = complete_pending(&tasks, &ready, ready_turn.turn_id).await;
    let pending = tasks.pending_terminal_deliveries(None).await.unwrap();
    assert_eq!(
        pending
            .iter()
            .map(|r| r.task_id.as_str())
            .collect::<Vec<_>>(),
        [busy_id.as_str(), ready_id.as_str()]
    );
    sink.install_watch_delivery(fixture.source_state.conversation_input_intake.clone());
    let started = std::time::Instant::now();
    tokio::time::timeout(Duration::from_secs(18), sink.resume_pending(None))
        .await
        .expect("one busy Chat cannot block recovery of all later results")
        .unwrap();
    assert!(started.elapsed() >= Duration::from_secs(10));
    let pending = tasks.pending_terminal_deliveries(None).await.unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(
        pending[0].task_id, busy_id,
        "timeout must retain the unacknowledged result"
    );
    assert!(fixture
        .target_state
        .conversation_authority
        .active_turn(&busy)
        .await
        .is_some());
    assert_eq!(
        fixture
            .target_state
            .conversation_authority
            .queue_snapshot(&busy)
            .await
            .len(),
        1
    );
    tokio::time::timeout(Duration::from_secs(5), async {
        while captured.requests.lock().await.len() != 1 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        captured.requests.lock().await[0].session_id.as_str(),
        ready.chat_id().as_str()
    );
    busy_release.notify_one();
    tokio::time::timeout(Duration::from_secs(8), async {
        while fixture
            .target_state
            .conversation_authority
            .active_turn(&busy)
            .await
            .is_some()
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    sink.resume_pending(None).await.unwrap();
    assert!(tasks
        .pending_terminal_deliveries(None)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        captured.requests.lock().await.len(),
        2,
        "recovery must not replay a durable note"
    );
    tasks.shutdown().await.unwrap();
    fixture.shutdown().await;
}
