use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn watch_waits_for_durable_completion_and_failed_persistence_can_retry() {
    let fixture = RemoteInputConfirmationFixture::new(Duration::ZERO, false).await;
    let release = Arc::new(Notify::new());
    let (address, turn, _) = prepare_running_file_calls(
        &fixture,
        &[ToolCall::new(
            "watch-receipt-origin",
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
    let notification = rua_task::TaskWatchOutputNotification {
        extra: Default::default(),
        notification_id: "watch-durable-completion".into(),
        owner: rua_task::TaskOwner::from_address(address),
        task_id: "phone-task".into(),
        attempt_no: 1,
        origin_turn_id: Some(turn.turn_id),
        projection: rua_task::TaskLiveOutputProjection::ModelVisible,
        records: vec![rua_task::TaskLiveOutputRecord {
            source: "stdout".into(),
            text: "watch receipt".into(),
        }],
    };
    let deliver = |notification: rua_task::TaskWatchOutputNotification| {
        let intake = fixture.source_state.conversation_input_intake.clone();
        tokio::spawn(async move { intake.deliver_task_watch_output(notification).await })
    };
    let wait_requests = |count| {
        let captured = captured.clone();
        async move {
            tokio::time::timeout(Duration::from_secs(8), async {
                while captured.requests.lock().await.len() < count {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
        }
    };
    let first = deliver(notification.clone());
    wait_requests(1).await;
    let store = WorkspaceSessionStore::new(fixture.target_state.chat_workspace().as_path());
    let storage_barrier = store.lock_chat_storage(&fixture.chat_id).await;
    model_release.notify_one();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        !first.is_finished(),
        "model completion alone must not acknowledge a Watch"
    );
    drop(storage_barrier);
    tokio::time::timeout(Duration::from_secs(8), first)
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    let failed_notification = rua_task::TaskWatchOutputNotification {
        notification_id: "watch-failed-completion".into(),
        ..notification
    };
    let failed = deliver(failed_notification.clone());
    wait_requests(2).await;
    let storage_barrier = store.lock_chat_storage(&fixture.chat_id).await;
    let index = store.chat_dir(&fixture.chat_id).join("turn_index.json");
    let backup = index.with_extension("receipt-backup");
    std::fs::rename(&index, &backup).unwrap();
    std::fs::create_dir(&index).unwrap();
    drop(storage_barrier);
    model_release.notify_one();
    let failure = tokio::time::timeout(Duration::from_secs(8), failed)
        .await
        .unwrap()
        .unwrap();
    assert!(
        failure.is_err(),
        "a failed durable completion must not acknowledge the Watch"
    );
    std::fs::remove_dir(&index).unwrap();
    std::fs::rename(&backup, &index).unwrap();
    let retry = deliver(failed_notification.clone());
    wait_requests(3).await;
    model_release.notify_one();
    tokio::time::timeout(Duration::from_secs(8), retry)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(8), deliver(failed_notification))
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        captured.requests.lock().await.len(),
        3,
        "only the failed processing may retry; acknowledged processing must deduplicate"
    );
    fixture.shutdown().await;
}
