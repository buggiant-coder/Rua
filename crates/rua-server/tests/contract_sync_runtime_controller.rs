use std::future::pending;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use rua_server::sync_runtime_controller::{
    SyncRuntimeController, SyncRuntimeError, SyncRuntimeParts, SyncRuntimeState,
};
use rua_sync::liveactor_apply::LiveApplyHandle;
use rua_sync::transport::SyncCycleResult;
use rua_sync::trigger::{SyncCycleFn, SyncScheduler, TriggerConfig, TriggerReason};

fn test_config() -> TriggerConfig {
    TriggerConfig {
        write_quiet_period: Duration::from_secs(60),
        write_max_delay: Duration::from_secs(60),
        periodic_interval: Duration::from_secs(60 * 60),
        app_resume_min_interval: Duration::ZERO,
    }
}

fn runtime_parts(cycles: Arc<AtomicUsize>, liveactors: Arc<AtomicUsize>) -> SyncRuntimeParts {
    let sync_fn: SyncCycleFn = Arc::new(move |_reason: TriggerReason| {
        let cycles = Arc::clone(&cycles);
        Box::pin(async move {
            cycles.fetch_add(1, Ordering::SeqCst);
            Ok(SyncCycleResult {
                peer_node_id: "peer-a".to_string(),
                ..SyncCycleResult::default()
            })
        })
    });
    let scheduler = SyncScheduler::start(sync_fn, test_config());
    liveactors.fetch_add(1, Ordering::SeqCst);
    let liveactor = LiveApplyHandle::from_task(tokio::spawn(pending::<()>()));
    SyncRuntimeParts::new(scheduler, Some(liveactor))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn warm_suspend_stops_runtime_and_resume_restarts_it() {
    let controller = SyncRuntimeController::default();
    let cycles = Arc::new(AtomicUsize::new(0));
    let liveactors = Arc::new(AtomicUsize::new(0));

    controller
        .ensure_active({
            let cycles = Arc::clone(&cycles);
            let liveactors = Arc::clone(&liveactors);
            move || async move { Ok(runtime_parts(cycles, liveactors)) }
        })
        .await
        .expect("start runtime");
    assert_eq!(controller.state().await, SyncRuntimeState::Active);
    assert!(controller.trigger_handle().await.is_some());
    assert_eq!(liveactors.load(Ordering::SeqCst), 1);

    controller.warm_suspend().await.expect("warm suspend");
    assert_eq!(
        controller.state().await,
        SyncRuntimeState::WarmSuspended {
            pending_grace: false
        }
    );
    assert!(controller.trigger_handle().await.is_none());

    controller.warm_suspend().await.expect("duplicate suspend");
    assert_eq!(liveactors.load(Ordering::SeqCst), 1);

    controller
        .resume_foreground({
            let cycles = Arc::clone(&cycles);
            let liveactors = Arc::clone(&liveactors);
            move || async move { Ok(runtime_parts(cycles, liveactors)) }
        })
        .await
        .expect("resume runtime");
    assert_eq!(controller.state().await, SyncRuntimeState::Active);
    assert!(controller.trigger_handle().await.is_some());
    assert_eq!(liveactors.load(Ordering::SeqCst), 2);

    let result = controller
        .trigger_manual_sync(|| async {
            Err(SyncRuntimeError::unavailable(
                "unexpected resume while already active",
            ))
        })
        .await
        .expect("manual sync");
    assert_eq!(result.peer_node_id, "peer-a");
    assert_eq!(cycles.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn manual_sync_from_warm_suspend_resumes_before_running_cycle() {
    let controller = SyncRuntimeController::default();
    let cycles = Arc::new(AtomicUsize::new(0));
    let liveactors = Arc::new(AtomicUsize::new(0));

    controller
        .ensure_active({
            let cycles = Arc::clone(&cycles);
            let liveactors = Arc::clone(&liveactors);
            move || async move { Ok(runtime_parts(cycles, liveactors)) }
        })
        .await
        .expect("start runtime");
    controller.warm_suspend().await.expect("warm suspend");

    let result = controller
        .trigger_manual_sync({
            let cycles = Arc::clone(&cycles);
            let liveactors = Arc::clone(&liveactors);
            move || async move { Ok(runtime_parts(cycles, liveactors)) }
        })
        .await
        .expect("manual sync resumes runtime");

    assert_eq!(result.peer_node_id, "peer-a");
    assert_eq!(controller.state().await, SyncRuntimeState::Active);
    assert_eq!(liveactors.load(Ordering::SeqCst), 2);
    assert_eq!(cycles.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn manual_sync_reports_not_paired_without_runtime() {
    let controller = SyncRuntimeController::default();
    controller.mark_not_paired().await;

    let err = controller
        .trigger_manual_sync(|| async {
            Err(SyncRuntimeError::unavailable(
                "unexpected start for not paired runtime",
            ))
        })
        .await
        .expect_err("manual sync should fail");

    assert_eq!(err.reason(), "not_paired");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scheduled_background_sync_runs_full_suspend_hook() {
    let controller = SyncRuntimeController::default();
    let cycles = Arc::new(AtomicUsize::new(0));
    let liveactors = Arc::new(AtomicUsize::new(0));
    let suspends = Arc::new(AtomicUsize::new(0));

    controller
        .ensure_active({
            let cycles = Arc::clone(&cycles);
            let liveactors = Arc::clone(&liveactors);
            move || async move { Ok(runtime_parts(cycles, liveactors)) }
        })
        .await
        .expect("start runtime");
    controller
        .trigger_scheduled_background_sync_with_suspend(
            || async {
                Err(SyncRuntimeError::unavailable(
                    "runtime should already be active",
                ))
            },
            false,
            {
                let suspends = Arc::clone(&suspends);
                move || async move {
                    suspends.fetch_add(1, Ordering::SeqCst);
                }
            },
        )
        .await
        .expect("scheduled sync");

    assert_eq!(cycles.load(Ordering::SeqCst), 1);
    assert_eq!(suspends.load(Ordering::SeqCst), 1);
    assert_eq!(
        controller.state().await,
        SyncRuntimeState::WarmSuspended {
            pending_grace: false
        }
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn foreground_resume_waits_for_full_background_suspend() {
    let controller = Arc::new(SyncRuntimeController::default());
    let cycles = Arc::new(AtomicUsize::new(0));
    let liveactors = Arc::new(AtomicUsize::new(0));
    controller
        .ensure_active({
            let cycles = Arc::clone(&cycles);
            let liveactors = Arc::clone(&liveactors);
            move || async move { Ok(runtime_parts(cycles, liveactors)) }
        })
        .await
        .expect("start runtime");

    let suspend_started = Arc::new(tokio::sync::Notify::new());
    let release_suspend = Arc::new(tokio::sync::Notify::new());
    controller
        .notify_background_with_suspend(Duration::from_millis(10), {
            let suspend_started = Arc::clone(&suspend_started);
            let release_suspend = Arc::clone(&release_suspend);
            move || async move {
                suspend_started.notify_one();
                release_suspend.notified().await;
            }
        })
        .await
        .expect("background grace");

    suspend_started.notified().await;
    let resume_starts = Arc::new(AtomicUsize::new(0));
    let resume = {
        let controller = Arc::clone(&controller);
        let cycles = Arc::clone(&cycles);
        let liveactors = Arc::clone(&liveactors);
        let resume_starts = Arc::clone(&resume_starts);
        tokio::spawn(async move {
            controller
                .resume_foreground(move || async move {
                    resume_starts.fetch_add(1, Ordering::SeqCst);
                    Ok(runtime_parts(cycles, liveactors))
                })
                .await
        })
    };
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(resume_starts.load(Ordering::SeqCst), 0);

    release_suspend.notify_one();
    resume
        .await
        .expect("resume task")
        .expect("foreground resume");
    assert_eq!(resume_starts.load(Ordering::SeqCst), 1);
    assert_eq!(controller.state().await, SyncRuntimeState::Active);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unavailable_runtime_still_runs_background_suspend_after_grace() {
    let controller = Arc::new(SyncRuntimeController::default());
    let suspended = Arc::new(tokio::sync::Notify::new());

    controller
        .notify_background_with_suspend(Duration::from_millis(10), {
            let suspended = Arc::clone(&suspended);
            move || async move {
                suspended.notify_one();
            }
        })
        .await
        .expect("background grace");

    tokio::time::timeout(Duration::from_secs(1), suspended.notified())
        .await
        .expect("suspend hook must run");
    assert_eq!(
        controller.state().await,
        SyncRuntimeState::WarmSuspended {
            pending_grace: false
        }
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn foreground_during_scheduled_sync_prevents_post_cycle_suspend() {
    let controller = Arc::new(SyncRuntimeController::default());
    let cycle_started = Arc::new(tokio::sync::Notify::new());
    let release_cycle = Arc::new(tokio::sync::Notify::new());
    let sync_fn: SyncCycleFn = Arc::new({
        let cycle_started = Arc::clone(&cycle_started);
        let release_cycle = Arc::clone(&release_cycle);
        move |_reason| {
            let cycle_started = Arc::clone(&cycle_started);
            let release_cycle = Arc::clone(&release_cycle);
            Box::pin(async move {
                cycle_started.notify_one();
                release_cycle.notified().await;
                Ok(SyncCycleResult::default())
            })
        }
    });
    let scheduler = SyncScheduler::start(sync_fn, test_config());
    controller
        .ensure_active(move || async move { Ok(SyncRuntimeParts::new(scheduler, None)) })
        .await
        .expect("start runtime");

    let suspends = Arc::new(AtomicUsize::new(0));
    let scheduled = {
        let controller = Arc::clone(&controller);
        let suspends = Arc::clone(&suspends);
        tokio::spawn(async move {
            controller
                .trigger_scheduled_background_sync_with_suspend(
                    || async {
                        Err(SyncRuntimeError::unavailable(
                            "runtime should already be active",
                        ))
                    },
                    false,
                    move || async move {
                        suspends.fetch_add(1, Ordering::SeqCst);
                    },
                )
                .await
        })
    };

    cycle_started.notified().await;
    controller
        .resume_foreground(|| async {
            Err(SyncRuntimeError::unavailable(
                "active runtime must be retained",
            ))
        })
        .await
        .expect("foreground while cycle is active");
    release_cycle.notify_one();
    scheduled
        .await
        .expect("scheduled task")
        .expect("scheduled cycle");

    assert_eq!(suspends.load(Ordering::SeqCst), 0);
    assert_eq!(controller.state().await, SyncRuntimeState::Active);
    assert!(controller.trigger_handle().await.is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unpaired_runtime_still_suspends_device_network_and_preserves_pairing_state() {
    let controller = Arc::new(SyncRuntimeController::default());
    controller.mark_not_paired().await;
    for _ in 0..2 {
        let suspended = Arc::new(tokio::sync::Notify::new());
        controller
            .notify_background_with_suspend(Duration::from_millis(10), {
                let suspended = Arc::clone(&suspended);
                move || async move {
                    suspended.notify_one();
                }
            })
            .await
            .expect("unpaired background grace");
        tokio::time::timeout(Duration::from_secs(1), suspended.notified())
            .await
            .expect("Device Network must suspend even without Sync pairing");
        assert_eq!(controller.state().await, SyncRuntimeState::NotPaired);
    }
    let suspended = Arc::new(AtomicUsize::new(0));
    controller
        .notify_background_with_suspend(Duration::from_millis(50), {
            let suspended = Arc::clone(&suspended);
            move || async move {
                suspended.fetch_add(1, Ordering::SeqCst);
            }
        })
        .await
        .expect("grace before foreground");
    let error = controller
        .notify_foreground(|| async { panic!("An unpaired foreground must not start Sync") })
        .await
        .expect_err("still unpaired");
    assert_eq!(error.reason(), "not_paired");
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(suspended.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pairing_during_unpaired_grace_starts_real_sync_parts() {
    let controller = Arc::new(SyncRuntimeController::default());
    controller.mark_not_paired().await;
    controller
        .notify_background(Duration::from_secs(10))
        .await
        .unwrap();
    let error = controller
        .trigger_manual_sync(|| async { panic!("Unpaired manual sync must not start a runtime") })
        .await
        .unwrap_err();
    assert_eq!(error.reason(), "not_paired");
    let liveactors = Arc::new(AtomicUsize::new(0));
    controller
        .ensure_active({
            let liveactors = Arc::clone(&liveactors);
            move || async move { Ok(runtime_parts(Arc::new(AtomicUsize::new(0)), liveactors)) }
        })
        .await
        .unwrap();
    assert_eq!(liveactors.load(Ordering::SeqCst), 1);
    assert_eq!(controller.state().await, SyncRuntimeState::Active);
    assert!(controller.trigger_handle().await.is_some());
    controller.warm_suspend().await.unwrap();
}
