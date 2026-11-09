use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use rua_sync::transport::SyncCycleResult;
use rua_sync::trigger::{SyncCycleFn, SyncScheduler, TriggerConfig, TriggerReason};

fn slow_counting_sync_fn(counter: Arc<AtomicU32>, cycle_duration: Duration) -> SyncCycleFn {
    Arc::new(move |_reason: TriggerReason| {
        let c = counter.clone();
        let d = cycle_duration;
        Box::pin(async move {
            c.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(d).await;
            Ok(SyncCycleResult::default())
        })
    })
}

#[tokio::test(start_paused = true)]
async fn overdue_writes_during_cycle_start_one_immediate_followup() {
    let counter = Arc::new(AtomicU32::new(0));
    let config = TriggerConfig {
        write_quiet_period: Duration::from_millis(500),
        write_max_delay: Duration::from_secs(1),
        periodic_interval: Duration::from_secs(6000),
        app_resume_min_interval: Duration::ZERO,
    };
    let sched = SyncScheduler::start(
        slow_counting_sync_fn(counter.clone(), Duration::from_secs(2)),
        config,
    );
    let handle = sched.handle();

    handle.notify_write();
    tokio::time::sleep(Duration::from_millis(501)).await;
    assert_eq!(counter.load(Ordering::SeqCst), 1);

    handle.notify_write();
    tokio::time::sleep(Duration::from_millis(400)).await;
    handle.notify_write();
    tokio::time::sleep(Duration::from_millis(400)).await;
    handle.notify_write();

    tokio::time::sleep(Duration::from_millis(1_201)).await;
    assert_eq!(counter.load(Ordering::SeqCst), 2);

    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(counter.load(Ordering::SeqCst), 2);

    sched.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn not_yet_due_followup_waits_only_for_remaining_deadline() {
    let counter = Arc::new(AtomicU32::new(0));
    let config = TriggerConfig {
        write_quiet_period: Duration::from_millis(500),
        write_max_delay: Duration::from_secs(1),
        periodic_interval: Duration::from_secs(6000),
        app_resume_min_interval: Duration::ZERO,
    };
    let sched = SyncScheduler::start(
        slow_counting_sync_fn(counter.clone(), Duration::from_millis(300)),
        config,
    );
    let handle = sched.handle();

    let manual = {
        let handle = handle.clone();
        tokio::spawn(async move { handle.trigger_manual().await })
    };
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(counter.load(Ordering::SeqCst), 1);
    handle.notify_write();

    tokio::time::sleep(Duration::from_millis(499)).await;
    assert_eq!(counter.load(Ordering::SeqCst), 1);

    tokio::time::sleep(Duration::from_millis(2)).await;
    assert_eq!(counter.load(Ordering::SeqCst), 2);
    manual.await.unwrap().unwrap();

    sched.shutdown().await;
}
