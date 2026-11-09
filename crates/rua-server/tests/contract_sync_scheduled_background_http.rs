use std::future::pending;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::Request;
use axum::http::{Method, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use rua_server::endpoints::sync::router as sync_router;
use rua_server::sync_runtime_controller::{SyncRuntimeParts, SyncRuntimeState};
use rua_server::{AppState, ServerProfile};
use rua_sync::liveactor_apply::LiveApplyHandle;
use rua_sync::power_policy::{LoadedSyncPowerPolicy, SyncPowerPolicyConfig, SyncPowerPolicyMode};
use rua_sync::transport::SyncCycleResult;
use rua_sync::trigger::{SyncCycleFn, SyncScheduler, TriggerConfig, TriggerReason};
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;

fn build_app(profile: ServerProfile) -> (Router, TempDir, AppState) {
    let tmp = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join("workspaces/default")).expect("seed dir");
    let state = AppState::from_tempdir(tmp.path().to_path_buf(), profile);
    let app = Router::new()
        .nest("/api", sync_router())
        .with_state(state.clone());
    (app, tmp, state)
}

fn set_policy(state: &AppState, mode: SyncPowerPolicyMode) {
    let host_profile = state.sync_runtime_host_profile();
    let config = SyncPowerPolicyConfig {
        mode,
        periodic_interval_secs: None,
        background_flush_grace_secs: None,
        allow_background_runtime: None,
        allow_background_scheduled_sync: None,
    };
    let loaded = LoadedSyncPowerPolicy {
        host_profile,
        stored_policy: Some(config.clone()),
        effective_policy: config.normalize(host_profile).expect("valid policy"),
    };
    *state
        .sync_power_policy
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = loaded;
}

fn runtime_parts(cycles: Arc<AtomicUsize>) -> SyncRuntimeParts {
    let sync_fn: SyncCycleFn = Arc::new(move |reason: TriggerReason| {
        let cycles = Arc::clone(&cycles);
        Box::pin(async move {
            assert_eq!(reason, TriggerReason::Manual);
            cycles.fetch_add(1, Ordering::SeqCst);
            Ok(SyncCycleResult {
                peer_node_id: "peer-a".to_string(),
                ..SyncCycleResult::default()
            })
        })
    });
    let scheduler = SyncScheduler::start(
        sync_fn,
        TriggerConfig {
            write_quiet_period: Duration::from_secs(60),
            write_max_delay: Duration::from_secs(60),
            periodic_interval: Duration::from_secs(60 * 60),
            app_resume_min_interval: Duration::ZERO,
        },
    );
    let liveactor = LiveApplyHandle::from_task(tokio::spawn(pending::<()>()));
    SyncRuntimeParts::new(scheduler, Some(liveactor))
}

async fn post_scheduled(app: &Router) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/sync/scheduled-background")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("oneshot")
}

async fn read_json(resp: axum::response::Response) -> Value {
    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    serde_json::from_slice(&bytes).expect("response JSON")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn power_saver_rejects_scheduled_background_sync_without_starting_runtime() {
    let (app, _tmp, state) = build_app(ServerProfile::MobileApk);
    state.sync_enabled.store(true, Ordering::SeqCst);
    set_policy(&state, SyncPowerPolicyMode::PowerSaver);

    let resp = post_scheduled(&app).await;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let body = read_json(resp).await;
    assert_eq!(body["action"], "scheduled_sync_disabled");
    assert_eq!(body["host_profile"], "mobile-apk");
    assert_eq!(body["scheduled"], false);
    assert_eq!(
        state.sync_runtime.state().await,
        SyncRuntimeState::Unavailable
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn balanced_mobile_runs_one_cycle_then_returns_to_warm_suspend() {
    let (app, _tmp, state) = build_app(ServerProfile::MobileApk);
    state.sync_enabled.store(true, Ordering::SeqCst);
    set_policy(&state, SyncPowerPolicyMode::Balanced);
    let cycles = Arc::new(AtomicUsize::new(0));
    state
        .sync_runtime
        .ensure_active({
            let cycles = Arc::clone(&cycles);
            move || async move { Ok(runtime_parts(cycles)) }
        })
        .await
        .expect("start runtime");

    let resp = post_scheduled(&app).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["action"], "scheduled_sync_completed");
    assert_eq!(body["scheduled"], true);
    assert_eq!(body["events_received"], 0);
    assert_eq!(cycles.load(Ordering::SeqCst), 1);
    assert_eq!(
        state.sync_runtime.state().await,
        SyncRuntimeState::WarmSuspended {
            pending_grace: false
        }
    );
    assert!(state.sync_runtime.trigger_handle().await.is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn balanced_mobile_keeps_runtime_while_background_transfer_lease_is_active() {
    let (app, _tmp, state) = build_app(ServerProfile::MobileApk);
    state.sync_enabled.store(true, Ordering::SeqCst);
    set_policy(&state, SyncPowerPolicyMode::Balanced);
    state
        .sync_power_policy
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .effective_policy
        .background_flush_grace_secs = 1;
    let cycles = Arc::new(AtomicUsize::new(0));
    state
        .sync_runtime
        .ensure_active({
            let cycles = Arc::clone(&cycles);
            move || async move { Ok(runtime_parts(cycles)) }
        })
        .await
        .expect("start runtime");
    assert!(state.foreground_operations.acquire_transfer(
        "job:scheduled-transfer".to_string(),
        "scheduled-transfer".to_string(),
        "device-a".to_string(),
        "device-b".to_string(),
        10,
    ));
    assert_eq!(state.foreground_operations.enter_background(false), None);

    let resp = post_scheduled(&app).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(cycles.load(Ordering::SeqCst), 1);
    assert_eq!(state.sync_runtime.state().await, SyncRuntimeState::Active);
    assert!(state.sync_runtime.trigger_handle().await.is_some());

    assert!(state
        .foreground_operations
        .release("job:scheduled-transfer"));
    let deadline = std::time::Instant::now() + Duration::from_secs(1);
    while std::time::Instant::now() < deadline
        && state.sync_runtime.state().await == SyncRuntimeState::Active
    {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        state.sync_runtime.state().await,
        SyncRuntimeState::WarmSuspended {
            pending_grace: true
        }
    );
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert_eq!(
        state.sync_runtime.state().await,
        SyncRuntimeState::WarmSuspended {
            pending_grace: false
        }
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn balanced_mobile_reports_not_paired_without_retryable_success_shape() {
    let (app, _tmp, state) = build_app(ServerProfile::MobileApk);
    state.sync_enabled.store(true, Ordering::SeqCst);
    set_policy(&state, SyncPowerPolicyMode::Balanced);

    let resp = post_scheduled(&app).await;
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = read_json(resp).await;
    assert_eq!(body["reason"], "not_paired");
    assert_eq!(
        state.sync_runtime.state().await,
        SyncRuntimeState::NotPaired
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn non_mobile_profile_accepts_noop_without_scheduled_attempt() {
    let (app, _tmp, state) = build_app(ServerProfile::Desktop);
    state.sync_enabled.store(true, Ordering::SeqCst);
    set_policy(&state, SyncPowerPolicyMode::Balanced);

    let resp = post_scheduled(&app).await;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let body = read_json(resp).await;
    assert_eq!(body["action"], "unsupported_host_profile");
    assert_eq!(body["host_profile"], "desktop");
    assert_eq!(body["scheduled"], false);
}
