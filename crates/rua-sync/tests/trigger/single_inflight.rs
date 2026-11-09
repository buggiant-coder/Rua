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
            tokio::time::sleep(d).await;
            c.fetch_add(1, Ordering::SeqCst);
            Ok(SyncCycleResult::default())
        })
    })
}

#[tokio::test(start_paused = true)]
async fn only_one_cycle_runs_at_a_time() {
    let counter = Arc::new(AtomicU32::new(0));
    let config = TriggerConfig {
        write_quiet_period: Duration::from_millis(100),
        write_max_delay: Duration::from_millis(200),
        periodic_interval: Duration::from_secs(6000),
        app_resume_min_interval: Duration::ZERO,
    };

    let sched = SyncScheduler::start(
        slow_counting_sync_fn(counter.clone(), Duration::from_secs(5)),
        config,
    );
    let handle = sched.handle();

    let h = handle.clone();
    tokio::spawn(async move {
        let _ = h.trigger_manual().await;
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    handle.notify_resume();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let h = handle.clone();
    tokio::spawn(async move {
        let _ = h.trigger_manual().await;
    });

    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(counter.load(Ordering::SeqCst), 0);

    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(counter.load(Ordering::SeqCst), 1);

    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(counter.load(Ordering::SeqCst), 2);

    tokio::time::sleep(Duration::from_secs(10)).await;
    assert_eq!(counter.load(Ordering::SeqCst), 2);

    sched.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn app_resume_fires_immediately() {
    let counter = Arc::new(AtomicU32::new(0));
    let config = TriggerConfig {
        write_quiet_period: Duration::from_millis(500),
        write_max_delay: Duration::from_secs(1),
        periodic_interval: Duration::from_secs(6000),
        app_resume_min_interval: Duration::ZERO,
    };
    let sched = SyncScheduler::start(
        slow_counting_sync_fn(counter.clone(), Duration::from_millis(100)),
        config,
    );
    let handle = sched.handle();

    handle.notify_resume();

    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(counter.load(Ordering::SeqCst), 1);

    sched.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn manual_cycle_consumes_pending_write_without_later_duplicate() {
    let counter = Arc::new(AtomicU32::new(0));
    let config = TriggerConfig {
        write_quiet_period: Duration::from_millis(500),
        write_max_delay: Duration::from_secs(1),
        periodic_interval: Duration::from_secs(6000),
        app_resume_min_interval: Duration::ZERO,
    };
    let sched = SyncScheduler::start(
        slow_counting_sync_fn(counter.clone(), Duration::ZERO),
        config,
    );
    let handle = sched.handle();

    handle.notify_write();
    tokio::time::sleep(Duration::from_millis(100)).await;
    handle.trigger_manual().await.unwrap();
    assert_eq!(counter.load(Ordering::SeqCst), 1);

    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(counter.load(Ordering::SeqCst), 1);

    sched.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn app_resume_cycle_consumes_pending_write_without_later_duplicate() {
    let counter = Arc::new(AtomicU32::new(0));
    let config = TriggerConfig {
        write_quiet_period: Duration::from_millis(500),
        write_max_delay: Duration::from_secs(1),
        periodic_interval: Duration::from_secs(6000),
        app_resume_min_interval: Duration::ZERO,
    };
    let sched = SyncScheduler::start(
        slow_counting_sync_fn(counter.clone(), Duration::ZERO),
        config,
    );
    let handle = sched.handle();

    handle.notify_write();
    tokio::time::sleep(Duration::from_millis(100)).await;
    handle.notify_resume();
    tokio::time::sleep(Duration::from_millis(1)).await;
    assert_eq!(counter.load(Ordering::SeqCst), 1);

    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(counter.load(Ordering::SeqCst), 1);

    sched.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn duplicate_app_resume_within_min_interval_is_dropped() {
    let counter = Arc::new(AtomicU32::new(0));
    let config = TriggerConfig {
        write_quiet_period: Duration::from_millis(500),
        write_max_delay: Duration::from_secs(1),
        periodic_interval: Duration::from_secs(6000),
        app_resume_min_interval: Duration::from_secs(10),
    };
    let sched = SyncScheduler::start(
        slow_counting_sync_fn(counter.clone(), Duration::from_millis(100)),
        config,
    );
    let handle = sched.handle();

    handle.notify_resume();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(counter.load(Ordering::SeqCst), 1);

    handle.notify_resume();
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(counter.load(Ordering::SeqCst), 1);

    tokio::time::sleep(Duration::from_secs(10)).await;
    handle.notify_resume();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(counter.load(Ordering::SeqCst), 2);

    sched.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn pending_duplicate_app_resume_does_not_force_followup_cycle() {
    let counter = Arc::new(AtomicU32::new(0));
    let config = TriggerConfig {
        write_quiet_period: Duration::from_millis(500),
        write_max_delay: Duration::from_secs(1),
        periodic_interval: Duration::from_secs(6000),
        app_resume_min_interval: Duration::from_secs(10),
    };
    let sched = SyncScheduler::start(
        slow_counting_sync_fn(counter.clone(), Duration::from_secs(5)),
        config,
    );
    let handle = sched.handle();

    handle.notify_resume();
    tokio::time::sleep(Duration::from_secs(1)).await;
    handle.notify_resume();

    tokio::time::sleep(Duration::from_secs(12)).await;
    assert_eq!(counter.load(Ordering::SeqCst), 1);

    sched.shutdown().await;
}
