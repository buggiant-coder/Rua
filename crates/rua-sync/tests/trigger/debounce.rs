use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use rua_sync::transport::SyncCycleResult;
use rua_sync::trigger::{SyncCycleFn, SyncScheduler, TriggerConfig, TriggerReason};

fn counting_sync_fn(counter: Arc<AtomicU32>) -> SyncCycleFn {
    Arc::new(move |_reason: TriggerReason| {
        let c = counter.clone();
        Box::pin(async move {
            c.fetch_add(1, Ordering::SeqCst);
            Ok(SyncCycleResult::default())
        })
    })
}

#[test]
fn default_write_deadlines_match_product_contract() {
    let config = TriggerConfig::default();
    assert_eq!(config.write_quiet_period, Duration::from_millis(500));
    assert_eq!(config.write_max_delay, Duration::from_secs(1));
}

#[tokio::test(start_paused = true)]
async fn single_write_fires_after_quiet_period() {
    let counter = Arc::new(AtomicU32::new(0));
    let config = TriggerConfig {
        write_quiet_period: Duration::from_millis(500),
        write_max_delay: Duration::from_secs(1),
        periodic_interval: Duration::from_secs(6000),
        app_resume_min_interval: Duration::ZERO,
    };
    let sched = SyncScheduler::start(counting_sync_fn(counter.clone()), config);
    let handle = sched.handle();

    handle.notify_write();

    tokio::time::sleep(Duration::from_millis(499)).await;
    assert_eq!(counter.load(Ordering::SeqCst), 0);

    tokio::time::sleep(Duration::from_millis(2)).await;
    assert_eq!(counter.load(Ordering::SeqCst), 1);

    sched.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn sustained_writes_fire_at_maximum_deadline() {
    let counter = Arc::new(AtomicU32::new(0));
    let config = TriggerConfig {
        write_quiet_period: Duration::from_millis(500),
        write_max_delay: Duration::from_secs(1),
        periodic_interval: Duration::from_secs(6000),
        app_resume_min_interval: Duration::ZERO,
    };
    let sched = SyncScheduler::start(counting_sync_fn(counter.clone()), config);
    let handle = sched.handle();

    for _ in 0..10 {
        handle.notify_write();
        tokio::time::sleep(Duration::from_millis(90)).await;
    }
    assert_eq!(counter.load(Ordering::SeqCst), 0);

    tokio::time::sleep(Duration::from_millis(101)).await;
    assert_eq!(counter.load(Ordering::SeqCst), 1);

    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(counter.load(Ordering::SeqCst), 1);

    sched.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn quiet_deadline_moves_with_last_write_without_moving_maximum() {
    let counter = Arc::new(AtomicU32::new(0));
    let config = TriggerConfig {
        write_quiet_period: Duration::from_millis(500),
        write_max_delay: Duration::from_secs(1),
        periodic_interval: Duration::from_secs(6000),
        app_resume_min_interval: Duration::ZERO,
    };
    let sched = SyncScheduler::start(counting_sync_fn(counter.clone()), config);
    let handle = sched.handle();

    handle.notify_write();
    tokio::time::sleep(Duration::from_millis(300)).await;
    handle.notify_write();
    tokio::time::sleep(Duration::from_millis(499)).await;
    assert_eq!(counter.load(Ordering::SeqCst), 0);

    tokio::time::sleep(Duration::from_millis(2)).await;
    assert_eq!(counter.load(Ordering::SeqCst), 1);

    sched.shutdown().await;
}
