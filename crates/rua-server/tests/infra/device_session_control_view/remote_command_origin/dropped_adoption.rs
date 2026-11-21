use super::*;

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropped_adoption_reconciles_the_reference_after_real_foreground_cancellation() {
    let fixture = RemoteInputConfirmationFixture::new(Duration::ZERO, false).await;
    let cwd = fixture
        .source_state
        .rua_root
        .canonicalize()
        .unwrap()
        .join("dropped-adoption");
    std::fs::create_dir(&cwd).unwrap();
    let call = ToolCall::new(
        "dropped-adoption",
        "bash",
        json!({
            "device_id":fixture.source.device_id,"cwd":cwd,"yield_seconds":1,
            "command":"printf once >> starts; sleep 60; printf unexpected > late"
        }),
    );
    let f = Origin::new(fixture, std::slice::from_ref(&call)).await;
    let recorder = &f.fixture.file_reads;
    recorder
        .hold_next_command_adopt
        .store(true, Ordering::SeqCst);
    let arguments = call.arguments.clone();
    let context = f.context(&call);
    let running = tokio::spawn(async move {
        rua_tool::ShellTool::new(Arc::new(rua_tool::RuntimeCommandExecutor::new()))
            .call(arguments, context)
            .await
    });
    tokio::time::timeout(
        Duration::from_secs(10),
        recorder.command_adopt_arrived.notified(),
    )
    .await
    .unwrap();
    let task_id = recorder
        .command_requests
        .lock()
        .unwrap()
        .iter()
        .find(|command| command.action == ChatCommandAction::Adopt)
        .unwrap()
        .context
        .execution_id
        .clone();
    let owner = rua_task::TaskOwner::new(&f.fixture.chat_id);
    let scope =
        rua_core::ChatStore::resolve(&f.fixture.target_state.rua_root, owner.chat_address.clone());
    let directory = scope.root().join("sessions").join(&f.fixture.chat_id);
    let before =
        rua_session::task_refs::ChatTaskRef::load(&directory, &f.fixture.chat_id, &task_id)
            .unwrap()
            .unwrap();
    assert!(!before.not_accepted && before.last_target_evidence.is_none());
    running.abort();
    assert!(running.await.unwrap_err().is_cancelled());
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let reference =
                rua_session::task_refs::ChatTaskRef::load(&directory, &f.fixture.chat_id, &task_id)
                    .unwrap()
                    .unwrap();
            if reference.not_accepted {
                assert!(reference.last_target_evidence.is_none());
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect(
        "Drop cleanup must resolve the pending reference from the target's real terminal receipt",
    );
    recorder.release_command_adopt.notify_one();
    assert!(matches!(
        f.fixture
            .source_state
            .task_runtime
            .get(&owner, &task_id)
            .await,
        Err(rua_task::TaskError::NotFound(_))
    ));
    assert_eq!(std::fs::read_to_string(cwd.join("starts")).unwrap(), "once");
    assert!(!cwd.join("late").exists());
    {
        let requests = recorder.command_requests.lock().unwrap();
        for action in [ChatCommandAction::Start, ChatCommandAction::Adopt] {
            assert_eq!(
                requests
                    .iter()
                    .filter(|command| command.action == action)
                    .count(),
                1
            );
        }
        assert!(requests
            .iter()
            .any(|command| command.action == ChatCommandAction::Cancel));
    }
    f.shutdown().await;
}
