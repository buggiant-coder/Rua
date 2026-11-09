use std::sync::Arc;
use std::time::Duration;

use rua_sync::transport::SyncCycleResult;
use rua_sync::trigger::{SyncCycleFn, SyncScheduler, TriggerConfig, TriggerReason};

fn sync_fn_returning_peer_id(peer_id: &'static str) -> SyncCycleFn {
    Arc::new(move |_reason: TriggerReason| {
        Box::pin(async move {
            Ok(SyncCycleResult {
                peer_node_id: peer_id.to_string(),
                events_received: 3,
                ..Default::default()
            })
        })
    })
}

#[tokio::test(start_paused = true)]
async fn manual_trigger_returns_peer_node_id() {
    let config = TriggerConfig {
        write_quiet_period: Duration::from_millis(100),
        write_max_delay: Duration::from_millis(200),
        periodic_interval: Duration::from_secs(6000),
        app_resume_min_interval: Duration::ZERO,
    };
    let sched = SyncScheduler::start(sync_fn_returning_peer_id("peer_abc123"), config);
    let handle = sched.handle();

    let result = handle.trigger_manual().await;
    let cycle = result.expect("cycle should succeed");
    assert_eq!(cycle.peer_node_id, "peer_abc123");
    assert_eq!(cycle.events_received, 3);

    sched.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn peer_node_id_propagates_through_coalesced_cycles() {
    let config = TriggerConfig {
        write_quiet_period: Duration::from_millis(100),
        write_max_delay: Duration::from_millis(200),
        periodic_interval: Duration::from_secs(6000),
        app_resume_min_interval: Duration::ZERO,
    };
    let sched = SyncScheduler::start(sync_fn_returning_peer_id("peer_xyz789"), config);
    let handle = sched.handle();

    let r1 = handle.trigger_manual().await.expect("cycle 1");
    assert_eq!(r1.peer_node_id, "peer_xyz789");

    tokio::time::sleep(Duration::from_millis(200)).await;
    let r2 = handle.trigger_manual().await.expect("cycle 2");
    assert_eq!(r2.peer_node_id, "peer_xyz789");

    sched.shutdown().await;
}
