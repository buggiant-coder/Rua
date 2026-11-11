use super::*;

pub(super) async fn warm_slow_pair() -> (TestRuntime, TestRuntime, DelayedNetwork) {
    let (a, b, network) = slow_pair().await;
    assert_eq!(
        a.runtime
            .session_manager()
            .command(
                &b.device_id,
                b.addr.clone(),
                b"warm",
                Duration::from_secs(20),
            )
            .await
            .expect("warm isolated slow QUIC connection"),
        b"warm",
    );
    // The measured RTT may come from an open path even when iroh has not
    // selected a path in this connection's observation. Do not require that
    // separate selection flag before exercising a real slow connection.
    let manager = a.runtime.session_manager();
    let diagnostics = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let diagnostics = manager.diagnostics();
            if diagnostics.sessions[0].rtt_ms.is_some() {
                break diagnostics;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "slow QUIC RTT was not observed: {:?}",
            manager.diagnostics()
        )
    });
    assert!(
        diagnostics.sessions[0].rtt_ms.expect("measured QUIC RTT") > 1000,
        "actual slow QUIC path: {:?}",
        diagnostics.sessions[0],
    );
    (a, b, network)
}

async fn wait_for_single_probe(manager: &DeviceSessionManager) {
    tokio::time::timeout(Duration::from_millis(300), async {
        loop {
            let diagnostics = manager.diagnostics();
            let active = diagnostics.sessions[0].active_logical_streams;
            assert!(
                active <= 1,
                "timeout cleanup must coalesce per generation: {diagnostics:?}"
            );
            if active == 1 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("tracked health probe started");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn slow_live_and_bulk_deadlines_return_before_the_shared_health_probe() {
    let (a, b, network) = warm_slow_pair().await;
    let manager = a.runtime.session_manager();
    let generation = manager.diagnostics().sessions[0].generation;
    b.runtime
        .session_manager()
        .set_live_activity_handler(Arc::new(EchoLiveActivityHandler::default()));
    b.runtime
        .session_manager()
        .set_bulk_transfer_handler(Arc::new(EchoBulkTransferHandler::default()));

    let started = Instant::now();
    let (live, bulk) = tokio::join!(
        manager.open_live_activity_to(&b.device_id, b"short", Duration::from_millis(100)),
        manager.open_bulk_transfer_to(&b.device_id, Duration::from_millis(100)),
    );
    assert!(matches!(
        live,
        Err(DeviceSessionError::Timeout {
            stage: "Live Activity subscription",
            ..
        })
    ));
    assert!(matches!(
        bulk,
        Err(DeviceSessionError::Timeout {
            stage: "Bulk Transfer stream opening",
            ..
        })
    ));
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "caller deadline must not wait for a 1.3s health exchange: {:?}",
        started.elapsed()
    );
    wait_for_single_probe(manager).await;
    tokio::time::sleep(Duration::from_millis(1800)).await;
    assert_eq!(manager.diagnostics().sessions[0].generation, generation);

    let mut live = manager
        .open_live_activity_to(&b.device_id, b"after", Duration::from_secs(5))
        .await
        .expect("reuse live generation");
    assert_eq!(
        live.next_frame().await.expect("live frame"),
        Some(b"after".to_vec())
    );
    let mut bulk = manager
        .open_bulk_transfer_to(&b.device_id, Duration::from_secs(5))
        .await
        .expect("reuse bulk generation");
    bulk.send_mut().write_u32(4).await.expect("bulk length");
    bulk.send_mut()
        .write_all(b"bulk")
        .await
        .expect("bulk bytes");
    assert_eq!(bulk.recv_mut().read_u32().await.expect("echo length"), 4);
    let mut bytes = [0; 4];
    bulk.recv_mut()
        .read_exact(&mut bytes)
        .await
        .expect("echo bytes");
    assert_eq!(&bytes, b"bulk");
    assert_eq!(manager.diagnostics().sessions[0].generation, generation);
    assert_eq!(manager.diagnostics().physical_dial_attempts, 1);
    drop(live);
    drop(bulk);
    a.runtime.shutdown().await.expect("shutdown A");
    b.runtime.shutdown().await.expect("shutdown B");
    network.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn blackholed_slow_generation_is_evicted_within_the_health_budget() {
    let (a, b, network) = warm_slow_pair().await;
    let manager = a.runtime.session_manager();
    network.set_blackhole(true);
    let error = manager
        .command_once_to(&b.device_id, b"lost", Duration::from_millis(100))
        .await
        .expect_err("caller deadline");
    assert!(matches!(
        error,
        DeviceSessionError::Timeout {
            stage: "command exchange",
            ..
        }
    ));
    wait_for_single_probe(manager).await;
    tokio::time::timeout(Duration::from_secs(6), async {
        while manager.diagnostics().sessions[0].generation.is_some() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("genuinely dead transport must be evicted within a finite probe budget");
    let diagnostics = manager.diagnostics();
    assert_eq!(
        diagnostics.sessions[0].state,
        DeviceSessionState::NotConnected
    );
    assert_eq!(diagnostics.physical_dial_attempts, 1);
    assert_eq!(
        b.handler.calls.load(Ordering::SeqCst),
        1,
        "no replay or delayed dispatch"
    );
    a.runtime.shutdown().await.expect("shutdown A");
    b.runtime.shutdown().await.expect("shutdown B");
    network.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_cancels_a_coalesced_slow_health_probe() {
    let (a, b, network) = warm_slow_pair().await;
    let manager = a.runtime.session_manager();
    network.set_blackhole(true);
    let started = Instant::now();
    let errors = futures::future::join_all(
        (0..16).map(|_| manager.command_once_to(&b.device_id, b"lost", Duration::from_millis(100))),
    )
    .await;
    assert!(errors.iter().all(|result| matches!(
        result,
        Err(DeviceSessionError::Timeout {
            stage: "command exchange",
            ..
        })
    )));
    assert!(started.elapsed() < Duration::from_millis(500));
    wait_for_single_probe(manager).await;
    tokio::time::timeout(Duration::from_millis(500), manager.shutdown())
        .await
        .expect("shutdown cancels the entire probe subtree without waiting for its RTT budget");
    assert!(manager
        .diagnostics()
        .sessions
        .iter()
        .all(|session| session.generation.is_none()));
    assert_eq!(manager.diagnostics().physical_dial_attempts, 1);
    a.runtime.shutdown().await.expect("shutdown A");
    b.runtime.shutdown().await.expect("shutdown B");
    network.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn network_change_probe_keeps_a_generation_with_a_measured_slow_path() {
    let (a, b, network) = warm_slow_pair().await;
    let manager = a.runtime.session_manager();
    let generation = manager.diagnostics().sessions[0].generation;
    let report = tokio::time::timeout(Duration::from_secs(5), manager.notify_network_change())
        .await
        .expect("bounded network-change probe");
    assert_eq!(report.probed_sessions, 1);
    assert_eq!(report.invalidated_sessions, 0);
    assert_eq!(manager.diagnostics().sessions[0].generation, generation);
    assert_eq!(manager.diagnostics().physical_dial_attempts, 1);
    a.runtime.shutdown().await.expect("shutdown A");
    b.runtime.shutdown().await.expect("shutdown B");
    network.shutdown().await;
}

// A prior QUIC consumer can select the peer's transport before the Device
// Session connection exists. Iroh's peer-wide route stays unchanged, so this
// newer connection has real open-path RTT without an is_selected observation.
async fn warm_unselected_slow_pair() -> (
    TestRuntime,
    TestRuntime,
    DelayedNetwork,
    iroh::endpoint::Connection,
) {
    let (a, b, network) = slow_pair().await;
    let anchor = a
        .runtime
        .endpoint()
        .connect(b.addr.clone(), rua_device_network::DEVICE_SESSION_ALPN)
        .await
        .expect("establish the peer route before the Device Session");
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if anchor
                .paths()
                .iter()
                .any(|path| path.rtt() > Duration::from_secs(1))
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("prior QUIC connection has a real measured slow path");
    assert_eq!(
        a.runtime
            .session_manager()
            .command(
                &b.device_id,
                b.addr.clone(),
                b"warm",
                Duration::from_secs(20)
            )
            .await
            .expect("authenticate the new session on the existing route"),
        b"warm",
    );
    let diagnostics = a.runtime.session_manager().diagnostics();
    assert_eq!(diagnostics.sessions[0].state, DeviceSessionState::Connected);
    assert_eq!(
        diagnostics.sessions[0].path_class, "unknown",
        "the regression requires an actual missing selected-path observation: {diagnostics:?}",
    );
    (a, b, network, anchor)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unselected_slow_path_retains_generation_after_logical_timeout() {
    let (a, b, network, anchor) = warm_unselected_slow_pair().await;
    let manager = a.runtime.session_manager();
    let before = manager.diagnostics();
    let generation = before.sessions[0].generation;
    let started = Instant::now();
    let error = manager
        .command_once_to(&b.device_id, b"short", Duration::from_millis(100))
        .await
        .expect_err("caller deadline precedes the slow round trip");
    assert!(matches!(
        error,
        DeviceSessionError::Timeout {
            stage: "command exchange",
            ..
        }
    ));
    assert!(started.elapsed() < Duration::from_millis(500));
    wait_for_single_probe(manager).await;
    tokio::time::sleep(Duration::from_millis(1800)).await;
    let after = manager.diagnostics();
    assert_eq!(after.sessions[0].generation, generation,
        "missing selected-path diagnostics must not retire a healthy slow QUIC connection: {after:?}");
    assert_eq!(after.sessions[0].state, DeviceSessionState::Connected);
    assert_eq!(after.physical_dial_attempts, 1);
    assert!(after.sessions[0].rtt_ms.expect("unselected connection RTT") > 1000);
    assert_eq!(
        b.handler.calls.load(Ordering::SeqCst),
        2,
        "no timed-out command replay"
    );
    anchor.close(iroh::endpoint::VarInt::from_u32(0), b"fixture complete");
    a.runtime.shutdown().await.expect("shutdown A");
    b.runtime.shutdown().await.expect("shutdown B");
    network.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unselected_slow_path_survives_network_change_and_evicts_a_blackhole() {
    let (a, b, network, anchor) = warm_unselected_slow_pair().await;
    let manager = a.runtime.session_manager();
    let generation = manager.diagnostics().sessions[0].generation;
    let report = tokio::time::timeout(Duration::from_secs(5), manager.notify_network_change())
        .await
        .expect("bounded probe on the unselected but healthy slow path");
    assert_eq!(report.probed_sessions, 1);
    assert_eq!(report.invalidated_sessions, 0);
    assert_eq!(manager.diagnostics().sessions[0].generation, generation);
    network.set_blackhole(true);
    manager
        .command_once_to(&b.device_id, b"lost", Duration::from_millis(100))
        .await
        .expect_err("caller deadline on the dead transport");
    wait_for_single_probe(manager).await;
    tokio::time::timeout(Duration::from_secs(6), async {
        while manager.diagnostics().sessions[0].generation.is_some() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("unselected dead path still has a finite health budget");
    assert_eq!(manager.diagnostics().physical_dial_attempts, 1);
    assert_eq!(
        b.handler.calls.load(Ordering::SeqCst),
        1,
        "no blackholed command replay"
    );
    anchor.close(iroh::endpoint::VarInt::from_u32(0), b"fixture complete");
    a.runtime.shutdown().await.expect("shutdown A");
    b.runtime.shutdown().await.expect("shutdown B");
    network.shutdown().await;
}
