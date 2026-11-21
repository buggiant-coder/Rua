use super::*;

#[path = "task_terminal_receipts.rs"]
mod terminal_receipts;

#[path = "task_watch_receipts.rs"]
mod watch_receipts;

#[path = "task_result_recovery.rs"]
mod recovery_backlog;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn task_watch_routes_to_current_authority_and_repeated_receipts_do_not_repeat_turns() {
    let fixture = RemoteInputConfirmationFixture::new(Duration::ZERO, false).await;
    let release = Arc::new(Notify::new());
    let (address, turn, _, _) = prepare_running_file_calls(
        &fixture,
        &[ToolCall::new(
            "watch-origin",
            "read",
            serde_json::json!({"file_path":"/fixture"}),
        )],
        &release,
    )
    .await;
    release.notify_one();
    tokio::time::timeout(Duration::from_secs(5), async {
        while fixture
            .target_state
            .conversation_authority
            .active_turn(&address)
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
    let store = WorkspaceSessionStore::new(fixture.target_state.chat_workspace().as_path());
    let source_store = WorkspaceSessionStore::new(fixture.source_state.chat_workspace().as_path());
    // The executor's stale local wake setting cannot override the current Authority.
    source_store
        .set_collaboration_wake_suppressed(&fixture.chat_id, true)
        .await
        .unwrap();
    let activity = rua_task::TaskWatchOutputNotification {
        extra: Default::default(),
        notification_id: "remote-activity-stable-id".into(),
        owner: rua_task::TaskOwner::from_address(address.clone()),
        task_id: "phone-task".into(),
        attempt_no: 2,
        origin_turn_id: Some(turn.turn_id.clone()),
        projection: rua_task::TaskLiveOutputProjection::ActivityOnly,
        records: vec![rua_task::TaskLiveOutputRecord {
            source: "stdout".into(),
            text: "activity-only".into(),
        }],
    };
    for _ in 0..2 {
        tokio::time::timeout(
            Duration::from_secs(8),
            fixture
                .source_state
                .conversation_input_intake
                .deliver_task_watch_output(activity.clone()),
        )
        .await
        .unwrap()
        .unwrap();
    }
    let notes = store
        .read_runtime_notes_for_turn(&fixture.chat_id, &turn.turn_id)
        .await
        .unwrap();
    assert_eq!(
        notes
            .iter()
            .filter(|note| note.runtime_note_id == activity.notification_id)
            .count(),
        1
    );
    assert!(
        captured.requests.lock().await.is_empty(),
        "activity-only must not wake a model"
    );
    let model = rua_task::TaskWatchOutputNotification {
        notification_id: "remote-model-stable-id".into(),
        projection: rua_task::TaskLiveOutputProjection::ModelVisible,
        records: vec![rua_task::TaskLiveOutputRecord {
            source: "stdout".into(),
            text: "new target output".into(),
        }],
        ..activity
    };
    for _ in 0..2 {
        tokio::time::timeout(
            Duration::from_secs(8),
            fixture
                .source_state
                .conversation_input_intake
                .deliver_task_watch_output(model.clone()),
        )
        .await
        .expect("including a lost-receipt retry must finish")
        .unwrap();
        let mut persisted = false;
        for turn in store
            .read_turn_range(&fixture.chat_id, 1, usize::MAX)
            .await
            .unwrap()
        {
            if store
                .read_runtime_notes_for_turn(&fixture.chat_id, &turn.turn_id)
                .await
                .unwrap()
                .iter()
                .any(|note| note.runtime_note_id == model.notification_id)
            {
                assert!(
                    rua_session::TurnLifecycleStatus::parse(&turn.lifecycle.status)
                        .is_syncable_conversation_state(),
                    "Watch ACK must already have durable processing completion"
                );
                persisted = true;
            }
        }
        assert!(persisted, "Watch ACK requires the durable observation");
    }
    let requests = captured.requests.lock().await;
    assert_eq!(
        requests.len(),
        1,
        "same notification must not start a second model Turn"
    );
    assert_eq!(requests[0].agent_id.as_str(), "default");
    assert_eq!(requests[0].session_id.as_str(), fixture.chat_id);
    drop(requests);
    assert!(
        source_store
            .read_turn_range(&fixture.chat_id, 1, usize::MAX)
            .await
            .unwrap()
            .is_empty(),
        "executor does not materialize the Chat observation locally"
    );
    fixture.shutdown().await;
}

struct CompletionRunner;
#[async_trait]
impl rua_task::TaskRunner for CompletionRunner {
    fn kind(&self) -> rua_task::TaskRunnerKind {
        rua_task::TaskRunnerKind::Tool
    }
    async fn run(
        &self,
        _request: rua_task::TaskRunRequest,
    ) -> Result<rua_task::TaskRunOutput, rua_task::TaskRunnerError> {
        Ok(rua_task::TaskRunOutput::completed(
            "terminal-from-execution-device",
        ))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn task_terminal_delivery_uses_legal_stable_wire_identity_and_durable_receipt_deduplication()
{
    use rua_task::TaskNotificationSink;
    let fixture = RemoteInputConfirmationFixture::new(Duration::ZERO, false).await;
    let release = Arc::new(Notify::new());
    let (address, turn, _, _) = prepare_running_file_calls(
        &fixture,
        &[ToolCall::new(
            "terminal-origin",
            "read",
            serde_json::json!({"file_path":"/fixture"}),
        )],
        &release,
    )
    .await;
    release.notify_one();
    tokio::time::timeout(Duration::from_secs(5), async {
        while fixture
            .target_state
            .conversation_authority
            .active_turn(&address)
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
    let source_store = WorkspaceSessionStore::new(fixture.source_state.chat_workspace().as_path());
    source_store
        .set_collaboration_wake_suppressed(&fixture.chat_id, true)
        .await
        .unwrap();
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
    let owner = rua_task::TaskOwner::from_address(address.clone());
    let id = tasks
        .create(
            rua_task::TaskSpec {
                owner: owner.clone(),
                instruction: "publish real terminal receipt".into(),
                runner: rua_task::TaskRunnerSpec::Tool {
                    tool_name: "fixture".into(),
                    arguments: serde_json::json!({}),
                },
                origin: rua_task::TaskOrigin {
                    turn_id: Some(turn.turn_id),
                    tool_call_id: Some("terminal-origin".into()),
                },
                context_refs: vec![],
                depends_on: vec![],
            },
            rua_task::TaskAttemptAuthorization::new("default").unwrap(),
        )
        .await
        .unwrap();
    let target_store = WorkspaceSessionStore::new(fixture.target_state.chat_workspace().as_path());
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let terminal = tasks.get(&owner, &id).await.unwrap().state.is_terminal();
            let delivered = tasks
                .pending_terminal_deliveries(Some(&owner))
                .await
                .unwrap()
                .is_empty();
            if terminal
                && delivered
                && captured.requests.lock().await.len() == 1
                && fixture
                    .target_state
                    .conversation_authority
                    .active_turn(&address)
                    .await
                    .is_none()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
    })
    .await
    .expect("actual terminal crosses Device Session and target outbox is acknowledged");
    let record = tasks.get(&owner, &id).await.unwrap();
    // A recovered lost acknowledgement reuses the same observation and command identity.
    sink.terminal(rua_task::TaskCompletionNotification { record })
        .await
        .unwrap();
    assert_eq!(captured.requests.lock().await.len(), 1);
    let expected = format!("task:{id}:attempt:1");
    let mut found = Vec::new();
    for turn in target_store
        .read_turn_range(&fixture.chat_id, 1, usize::MAX)
        .await
        .unwrap()
    {
        found.extend(
            target_store
                .read_runtime_notes_for_turn(&fixture.chat_id, &turn.turn_id)
                .await
                .unwrap()
                .into_iter()
                .filter(|note| note.runtime_note_id == expected),
        );
    }
    assert_eq!(found.len(), 1);
    assert!(tasks
        .pending_terminal_deliveries(Some(&owner))
        .await
        .unwrap()
        .is_empty());
    assert!(
        source_store
            .read_turn_range(&fixture.chat_id, 1, usize::MAX)
            .await
            .unwrap()
            .is_empty(),
        "stale source suppression must not append/ack locally"
    );
    tasks.shutdown().await.unwrap();
    fixture.shutdown().await;
}
