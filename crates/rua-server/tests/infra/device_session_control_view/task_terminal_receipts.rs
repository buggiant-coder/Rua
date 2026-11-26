use super::*;
use rua_task::TaskNotificationSink;

async fn terminal_runtime(
    fixture: &RemoteInputConfirmationFixture,
    turn_id: String,
) -> (
    TempDir,
    Arc<rua_task::TaskRuntime>,
    Arc<rua_server::services::task_notification::ServerTaskNotificationSink>,
    String,
) {
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
    let tasks = Arc::new(rua_task::TaskRuntime::open_with_sink(config, sink.clone()).unwrap());
    sink.install_tasks(&tasks);
    sink.install_watch_delivery(fixture.source_state.conversation_input_intake.clone());
    tasks.register_runner(Arc::new(CompletionRunner));
    let id = tasks
        .create(
            rua_task::TaskSpec {
                owner: rua_task::TaskOwner::new(&fixture.chat_id),
                instruction: "persist terminal before receipt".into(),
                runner: rua_task::TaskRunnerSpec::Tool {
                    tool_name: "fixture".into(),
                    arguments: serde_json::json!({}),
                },
                origin: rua_task::TaskOrigin {
                    turn_id: Some(turn_id),
                    tool_call_id: Some("terminal-receipt-call".into()),
                },
                context_refs: vec![],
                depends_on: vec![],
            },
            rua_task::TaskAttemptAuthorization::new("default").unwrap(),
        )
        .await
        .unwrap();
    (data, tasks, sink, id)
}

async fn wait_queued(fixture: &RemoteInputConfirmationFixture, address: &ChatAddress) {
    tokio::time::timeout(Duration::from_secs(8), async {
        while fixture
            .target_state
            .conversation_authority
            .queue_snapshot(address)
            .await
            .is_empty()
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("terminal observation is queued behind the active Turn");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_outbox_waits_for_durable_note_but_not_for_model_completion() {
    let fixture = RemoteInputConfirmationFixture::new(Duration::ZERO, false).await;
    let release = Arc::new(Notify::new());
    let (address, turn, _) = prepare_running_file_calls(
        &fixture,
        &[ToolCall::new(
            "receipt-origin",
            "read",
            serde_json::json!({"file_path":"/fixture"}),
        )],
        &release,
    )
    .await;
    let model_release = Arc::new(Notify::new());
    let captured = Arc::new(CapturingAgentRuntime {
        held_tool_calls: Some((vec![], model_release.clone())),
        ..Default::default()
    });
    fixture
        .target_state
        .registry
        .get_or_create("default")
        .await
        .unwrap()
        .set_react_runtime(captured.clone())
        .await;
    let (_data, tasks, sink, id) = terminal_runtime(&fixture, turn.turn_id).await;
    let owner = rua_task::TaskOwner::from_address(address.clone());
    wait_queued(&fixture, &address).await;
    assert_eq!(
        tasks
            .pending_terminal_deliveries(Some(&owner))
            .await
            .unwrap()
            .len(),
        1,
        "queue acceptance is not a durable receipt"
    );
    let record = tasks.get(&owner, &id).await.unwrap();
    let repeat_sink = sink.clone();
    let duplicate = tokio::spawn(async move {
        repeat_sink
            .terminal(rua_task::TaskCompletionNotification { record })
            .await
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        !duplicate.is_finished(),
        "in-flight duplicate must join the same pending receipt"
    );
    assert_eq!(
        fixture
            .target_state
            .conversation_authority
            .queue_snapshot(&address)
            .await
            .len(),
        1
    );
    let store = WorkspaceSessionStore::new(fixture.target_state.chat_workspace().as_path());
    let storage_barrier = store.lock_chat_storage(&fixture.chat_id).await;
    release.notify_one();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        tasks
            .pending_terminal_deliveries(Some(&owner))
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(captured.requests.lock().await.is_empty());
    drop(storage_barrier);
    tokio::time::timeout(Duration::from_secs(8), async {
        while !tasks
            .pending_terminal_deliveries(Some(&owner))
            .await
            .unwrap()
            .is_empty()
            || captured.requests.lock().await.is_empty()
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("durable note acknowledges the outbox while the model is still running");
    assert!(fixture
        .target_state
        .conversation_authority
        .active_turn(&address)
        .await
        .is_some());
    tokio::time::timeout(Duration::from_secs(3), duplicate)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let record = tasks.get(&owner, &id).await.unwrap();
    tokio::time::timeout(
        Duration::from_secs(3),
        sink.terminal(rua_task::TaskCompletionNotification { record }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        captured.requests.lock().await.len(),
        1,
        "durable note in an active Turn is already received"
    );
    model_release.notify_one();
    tasks.shutdown().await.unwrap();
    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_terminal_materialization_retains_outbox_and_retry_persists_once() {
    let fixture = RemoteInputConfirmationFixture::new(Duration::ZERO, false).await;
    let release = Arc::new(Notify::new());
    let (address, turn, _) = prepare_running_file_calls(
        &fixture,
        &[ToolCall::new(
            "failed-receipt-origin",
            "read",
            serde_json::json!({"file_path":"/fixture"}),
        )],
        &release,
    )
    .await;
    let captured = Arc::new(CapturingAgentRuntime::default());
    fixture
        .target_state
        .registry
        .get_or_create("default")
        .await
        .unwrap()
        .set_react_runtime(captured.clone())
        .await;
    let (_data, tasks, sink, id) = terminal_runtime(&fixture, turn.turn_id).await;
    let owner = rua_task::TaskOwner::from_address(address.clone());
    wait_queued(&fixture, &address).await;
    let scope = rua_core::ChatStore::resolve(&fixture.target_state.rua_root, address.clone());
    let invalid_pending = scope
        .root()
        .join("sessions")
        .join(&fixture.chat_id)
        .join(".pending_runtime_notes.json");
    assert!(!invalid_pending.exists());
    std::fs::create_dir(&invalid_pending).unwrap();
    release.notify_one();
    tokio::time::timeout(Duration::from_secs(8), async {
        while tasks
            .terminal_delivery_error(&owner, &id, 1)
            .await
            .unwrap()
            .is_none()
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("failed persistence sends an error receipt");
    assert_eq!(
        tasks
            .pending_terminal_deliveries(Some(&owner))
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(
        captured.requests.lock().await.is_empty(),
        "model must not run before the observation is durable"
    );
    std::fs::remove_dir(&invalid_pending).unwrap();
    let record = tasks.get(&owner, &id).await.unwrap();
    tokio::time::timeout(
        Duration::from_secs(8),
        sink.terminal(rua_task::TaskCompletionNotification { record }),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(tasks
        .pending_terminal_deliveries(Some(&owner))
        .await
        .unwrap()
        .is_empty());
    let store = WorkspaceSessionStore::new(scope.root());
    let expected = format!("task:{id}:attempt:1");
    let mut count = 0;
    for turn in store
        .read_turn_range(&fixture.chat_id, 1, usize::MAX)
        .await
        .unwrap()
    {
        count += store
            .read_runtime_notes_for_turn(&fixture.chat_id, &turn.turn_id)
            .await
            .unwrap()
            .iter()
            .filter(|note| note.runtime_note_id == expected)
            .count();
    }
    assert_eq!(count, 1);
    tasks.shutdown().await.unwrap();
    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn discarded_terminal_observations_fail_their_receipts_and_can_be_requeued() {
    let fixture = RemoteInputConfirmationFixture::new(Duration::ZERO, false).await;
    let release = Arc::new(Notify::new());
    let (address, turn, _) = prepare_running_file_calls(
        &fixture,
        &[ToolCall::new(
            "discard-origin",
            "read",
            serde_json::json!({"file_path":"/fixture"}),
        )],
        &release,
    )
    .await;
    let captured = Arc::new(CapturingAgentRuntime::default());
    fixture
        .target_state
        .registry
        .get_or_create("default")
        .await
        .unwrap()
        .set_react_runtime(captured.clone())
        .await;
    let (_data, tasks, sink, id) = terminal_runtime(&fixture, turn.turn_id).await;
    let owner = rua_task::TaskOwner::from_address(address.clone());
    let authority = &fixture.target_state.conversation_authority;
    let mut repeats = Vec::new();
    let mut previous_submission = None;
    for operation in 0..3 {
        wait_queued(&fixture, &address).await;
        let pending = authority.queue_snapshot(&address).await;
        assert_eq!(pending.len(), 1);
        assert_ne!(
            previous_submission.as_ref(),
            Some(&pending[0].submission_id)
        );
        previous_submission = Some(pending[0].submission_id.clone());
        match operation {
            0 => authority.clear_queue(&address).await,
            1 => authority.discard_unclaimed_inputs(&address).await,
            _ => {
                authority
                    .delete_queued_input(&address, &pending[0].input_id)
                    .await
                    .unwrap();
            }
        }
        assert!(authority.queue_snapshot(&address).await.is_empty());
        assert_eq!(
            tasks
                .pending_terminal_deliveries(Some(&owner))
                .await
                .unwrap()
                .len(),
            1
        );
        let record = tasks.get(&owner, &id).await.unwrap();
        let sink = sink.clone();
        repeats.push(tokio::spawn(async move {
            sink.terminal(rua_task::TaskCompletionNotification { record })
                .await
        }));
    }
    wait_queued(&fixture, &address).await;
    authority
        .request_stop(
            Some(&fixture.target_state.streams),
            CONSOLE_CHANNEL_ID,
            &address,
        )
        .await
        .unwrap();
    assert_eq!(
        authority.queue_snapshot(&address).await.len(),
        1,
        "ordinary Stop keeps background observations"
    );
    assert!(
        !repeats.last().unwrap().is_finished(),
        "ordinary Stop must not fail a retained receipt"
    );
    release.notify_one();
    for repeat in repeats {
        tokio::time::timeout(Duration::from_secs(8), repeat)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
    tokio::time::timeout(Duration::from_secs(8), async {
        while !tasks
            .pending_terminal_deliveries(Some(&owner))
            .await
            .unwrap()
            .is_empty()
            || captured.requests.lock().await.is_empty()
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(captured.requests.lock().await.len(), 1);
    tasks.shutdown().await.unwrap();
    fixture.shutdown().await;
}
