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

#[tokio::test(start_paused = true)]
async fn periodic_fires_on_interval() {
    let counter = Arc::new(AtomicU32::new(0));
    let config = TriggerConfig {
        write_quiet_period: Duration::from_millis(500),
        write_max_delay: Duration::from_secs(1),
        periodic_interval: Duration::from_secs(60),
        app_resume_min_interval: Duration::ZERO,
    };
    let sched = SyncScheduler::start(counting_sync_fn(counter.clone()), config);

    tokio::time::sleep(Duration::from_secs(61)).await;
    assert_eq!(counter.load(Ordering::SeqCst), 1);

    tokio::time::sleep(Duration::from_secs(60)).await;
    assert_eq!(counter.load(Ordering::SeqCst), 2);

    sched.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn periodic_does_not_fire_before_interval() {
    let counter = Arc::new(AtomicU32::new(0));
    let config = TriggerConfig {
        write_quiet_period: Duration::from_millis(500),
        write_max_delay: Duration::from_secs(1),
        periodic_interval: Duration::from_secs(600),
        app_resume_min_interval: Duration::ZERO,
    };
    let sched = SyncScheduler::start(counting_sync_fn(counter.clone()), config);

    tokio::time::sleep(Duration::from_secs(300)).await;
    assert_eq!(counter.load(Ordering::SeqCst), 0);

    sched.shutdown().await;
}
