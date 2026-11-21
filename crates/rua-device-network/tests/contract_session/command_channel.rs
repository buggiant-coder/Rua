use super::*;

async fn pair(first: u8) -> (TestRuntime, TestRuntime) {
    let network = [first + 2; 32];
    let b = build_runtime(first + 1, network, vec![peer_entry(first)], test_config()).await;
    let a = build_runtime(
        first,
        network,
        vec![peer_entry_with_direct_addrs(
            first + 1,
            direct_addrs(&b.addr),
        )],
        test_config(),
    )
    .await;
    (a, b)
}

async fn wait_for_streams(manager: &DeviceSessionManager, expected: usize) {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if manager
                .diagnostics()
                .sessions
                .iter()
                .map(|session| session.active_logical_streams)
                .sum::<usize>()
                == expected
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("command lease count converged");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn command_channel_retains_both_connections_and_releases_each_lease() {
    let (a, b) = pair(231).await;
    let source = a.runtime.session_manager();
    let target = b.runtime.session_manager();
    let channel = source
        .open_command_channel(&b.device_id, Duration::from_secs(5))
        .await
        .unwrap();
    channel
        .command(b"retain-command-connection", Duration::from_secs(2))
        .await
        .unwrap();
    let context = b.handler.command_contexts.lock().unwrap()[0].clone();
    assert_eq!(context.peer_device_id(), a.device_id);
    assert_eq!(
        context.generation(),
        target.diagnostics().sessions[0].generation.unwrap()
    );
    assert_eq!(channel.peer_device_id(), b.device_id);
    wait_for_streams(source, 1).await;
    wait_for_streams(target, 1).await;
    drop(channel);
    wait_for_streams(source, 0).await;
    assert!(
        !context.is_closed(),
        "dropping a caller lease is not a physical disconnect"
    );
    b.handler.command_contexts.lock().unwrap().clear();
    drop(context);
    wait_for_streams(target, 0).await;
    a.runtime.shutdown().await.unwrap();
    b.runtime.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn command_channel_timeout_does_not_replay_or_close_the_live_generation() {
    let (a, b) = pair(234).await;
    let channel = a
        .runtime
        .session_manager()
        .open_command_channel(&b.device_id, Duration::from_secs(5))
        .await
        .unwrap();
    channel
        .command(b"retain-command-connection", Duration::from_secs(2))
        .await
        .unwrap();
    let context = b.handler.command_contexts.lock().unwrap()[0].clone();
    let generation = channel.generation();
    b.handler.hang_next.store(true, Ordering::SeqCst);
    assert!(channel
        .command(b"one-side-effect", Duration::from_millis(80))
        .await
        .is_err());
    assert_eq!(b.handler.calls.load(Ordering::SeqCst), 2);
    assert!(!channel.is_closed());
    assert!(!context.is_closed());
    assert_eq!(
        channel
            .command(b"still-live", Duration::from_secs(2))
            .await
            .unwrap(),
        b"still-live"
    );
    assert_eq!(channel.generation(), generation);
    b.handler.hang_release.notify_one();
    assert_eq!(
        b.handler.calls.load(Ordering::SeqCst),
        3,
        "the timed-out mutation was never replayed"
    );
    b.runtime
        .session_manager()
        .disconnect_peer(&a.device_id)
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), channel.closed())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), context.closed())
        .await
        .unwrap();
    assert!(channel
        .command(b"must-not-reconnect", Duration::from_secs(1))
        .await
        .is_err());
    assert_eq!(b.handler.calls.load(Ordering::SeqCst), 3);
    a.runtime.shutdown().await.unwrap();
    b.runtime.shutdown().await.unwrap();
}
