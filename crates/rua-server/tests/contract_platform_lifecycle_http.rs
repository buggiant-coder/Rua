use std::future::pending;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::Request;
use axum::http::{Method, StatusCode};
use axum::Router;
use futures::StreamExt;
use http_body_util::BodyExt;
use rua_server::endpoints::platform::router as platform_router;
use rua_server::services::conversation_authority::{
    ConversationActiveTurnPhase, ConversationActiveTurnStart, ConversationDriverKind,
};
use rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch;
use rua_server::services::user_notification::{UserNotificationRequest, UserNotificationSender};
use rua_server::sync_runtime_controller::{SyncRuntimeParts, SyncRuntimeState};
use rua_server::{AppState, ServerProfile};
use rua_sync::liveactor_apply::LiveApplyHandle;
use rua_sync::power_policy::{LoadedSyncPowerPolicy, SyncPowerPolicyConfig, SyncPowerPolicyMode};
use rua_sync::transport::SyncCycleResult;
use rua_sync::trigger::{SyncCycleFn, SyncScheduler, TriggerConfig, TriggerReason};
use serde_json::{json, Value};
use serial_test::serial;
use tempfile::TempDir;
use tower::ServiceExt;

fn build_app(profile: ServerProfile) -> (Router, TempDir, AppState) {
    let tmp = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join("workspaces/default")).expect("seed dir");
    let state = AppState::from_tempdir(tmp.path().to_path_buf(), profile);
    let app = Router::new()
        .nest("/api", platform_router())
        .with_state(state.clone());
    (app, tmp, state)
}

fn build_paired_app(profile: ServerProfile) -> (Router, TempDir, AppState) {
    let tmp = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join("workspaces/default")).expect("seed dir");
    let sync_dir = tmp.path().join("sync");
    std::fs::create_dir_all(&sync_dir).expect("sync dir");
    rua_sync::identity::NetworkSecret::generate()
        .save(&sync_dir)
        .expect("network secret");
    rua_sync::control_store::SyncControlStore::from_sync_dir(&sync_dir)
        .write_enabled(true)
        .expect("enable sync");
    let state = AppState::from_tempdir(tmp.path().to_path_buf(), profile);
    let app = Router::new()
        .nest("/api", platform_router())
        .with_state(state.clone());
    (app, tmp, state)
}

fn set_background_grace(state: &AppState, grace: Duration) {
    let host_profile = state.sync_runtime_host_profile();
    let config = SyncPowerPolicyConfig {
        mode: SyncPowerPolicyMode::Custom,
        periodic_interval_secs: Some(60),
        background_flush_grace_secs: Some(grace.as_secs()),
        allow_background_runtime: Some(false),
        allow_background_scheduled_sync: Some(false),
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
            if reason == TriggerReason::AppResume {
                cycles.fetch_add(1, Ordering::SeqCst);
            }
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

async fn start_runtime(state: &AppState, cycles: Arc<AtomicUsize>) {
    state
        .sync_runtime
        .ensure_active(move || async move { Ok(runtime_parts(cycles)) })
        .await
        .expect("start runtime");
    state.device_network_availability.mark_ready();
}

async fn request(app: &Router, event: &str) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/platform/lifecycle")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({ "event": event })).expect("serialize body"),
                ))
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

async fn get(app: &Router, uri: &str) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(uri)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("oneshot")
}

async fn wait_for_cycles(cycles: &AtomicUsize, expected: usize) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        if cycles.load(Ordering::SeqCst) >= expected {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(cycles.load(Ordering::SeqCst), expected);
}

async fn wait_for_network_suspended(state: &AppState) {
    let mut availability = state.device_network_availability.subscribe();

    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if availability.borrow().state
                == rua_server::services::device_network_availability::DeviceNetworkAvailabilityState::Suspended
            {
                break;
            }
            availability
                .changed()
                .await
                .expect("Device Network availability stream");
        }
    })
    .await
    .expect("Device Network did not suspend");
    assert!(state.device_network.lock().await.is_none());
}

async fn wait_for_endpoint_closed(endpoint: &iroh::Endpoint) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        if endpoint.is_closed() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(endpoint.is_closed());
}

async fn wait_for_device_network_ready(state: &AppState) {
    let mut availability = state.device_network_availability.subscribe();
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if state.device_network.lock().await.is_some()
                && availability.borrow().state
                == rua_server::services::device_network_availability::DeviceNetworkAvailabilityState::Ready
                && state.sync_runtime.state().await == SyncRuntimeState::Active
                && state.sync_runtime.trigger_handle().await.is_some()
            {
                break;
            }
            tokio::select! {
                changed = availability.changed() => {
                    changed.expect("Device Network availability stream");
                }
                _ = tokio::time::sleep(Duration::from_millis(20)) => {}
            }
        }
    })
    .await
    .expect("Device Network and Sync Runtime did not become ready");
    assert!(state.device_network.lock().await.is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mobile_background_enters_grace_then_warm_suspends() {
    let (app, _tmp, state) = build_app(ServerProfile::MobileApk);
    set_background_grace(&state, Duration::from_secs(1));
    start_runtime(&state, Arc::new(AtomicUsize::new(0))).await;

    let resp = request(&app, "background").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["host_profile"], "mobile-apk");
    assert_eq!(body["action"], "background_grace_started");
    assert_eq!(body["runtime"]["state"], "warm_suspended");
    assert_eq!(body["runtime"]["pending_grace"], true);
    assert!(state.sync_runtime.trigger_handle().await.is_some());
    let residency = state.foreground_operations.snapshot();
    assert!(residency.locks_required);
    assert_eq!(
        residency.residency_state,
        rua_server::services::foreground_operation::MobileResidencyState::BackgroundGrace
    );

    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert_eq!(
        state.sync_runtime.state().await,
        SyncRuntimeState::WarmSuspended {
            pending_grace: false
        }
    );
    assert!(state.sync_runtime.trigger_handle().await.is_none());
    let residency = state.foreground_operations.snapshot();
    assert!(!residency.locks_required);
    assert_eq!(
        residency.residency_state,
        rua_server::services::foreground_operation::MobileResidencyState::Suspended
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(device_network_lifecycle)]
async fn mobile_foreground_during_grace_cancels_suspend_and_triggers_resume_cycle() {
    let (app, _tmp, state) = build_paired_app(ServerProfile::MobileApk);
    set_background_grace(&state, Duration::from_secs(1));
    state
        .ensure_sync_runtime_active()
        .await
        .expect("start paired Sync runtime");
    state.sync_runtime.shutdown_unavailable().await;
    state
        .ensure_device_network_runtime()
        .await
        .expect("start paired Device Network Runtime");
    let cycles = Arc::new(AtomicUsize::new(0));
    start_runtime(&state, Arc::clone(&cycles)).await;

    let background = request(&app, "background").await;
    assert_eq!(background.status(), StatusCode::OK);
    assert_eq!(
        state.sync_runtime.state().await,
        SyncRuntimeState::WarmSuspended {
            pending_grace: true
        }
    );

    let foreground = request(&app, "foreground").await;
    assert_eq!(foreground.status(), StatusCode::OK);
    let body = read_json(foreground).await;
    assert_eq!(body["action"], "foreground_resume_requested");
    wait_for_cycles(&cycles, 1).await;
    assert_eq!(state.sync_runtime.state().await, SyncRuntimeState::Active);

    tokio::time::sleep(Duration::from_millis(25)).await;
    assert_eq!(state.sync_runtime.state().await, SyncRuntimeState::Active);
    assert!(state.sync_runtime.trigger_handle().await.is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[serial(device_network_lifecycle)]
async fn mobile_grace_closes_complete_device_network_and_foreground_rebinds_it() {
    let (app, _tmp, state) = build_paired_app(ServerProfile::MobileApk);
    set_background_grace(&state, Duration::from_secs(1));
    state
        .ensure_sync_runtime_active()
        .await
        .expect("start paired runtime");
    let first_runtime = state
        .device_network
        .lock()
        .await
        .as_ref()
        .cloned()
        .expect("Device Network Runtime");
    let endpoint_id = first_runtime.endpoint().id();
    let first_endpoint = first_runtime.endpoint().clone();

    let background = request(&app, "background").await;
    assert_eq!(background.status(), StatusCode::OK);
    wait_for_network_suspended(&state).await;
    wait_for_endpoint_closed(&first_endpoint).await;
    assert!(first_endpoint.is_closed());
    let stopped = first_runtime.diagnostics();
    assert_eq!(stopped.state, "stopped");
    assert!(!stopped.gossip_running);
    assert!(state.sync_node.lock().await.is_none());
    assert!(state
        .sync_transfer_session_manager
        .read()
        .expect("session manager lock")
        .is_none());

    let foreground = request(&app, "foreground").await;
    assert_eq!(foreground.status(), StatusCode::OK);
    let body = read_json(foreground).await;
    assert_eq!(body["action"], "foreground_resume_requested");
    wait_for_device_network_ready(&state).await;
    let second_runtime = state
        .device_network
        .lock()
        .await
        .as_ref()
        .cloned()
        .expect("rebound Device Network Runtime");
    assert_eq!(second_runtime.endpoint().id(), endpoint_id);
    assert!(!second_runtime.endpoint().is_closed());
    assert!(state.sync_node.lock().await.is_some());
    assert!(state.sync_runtime.trigger_handle().await.is_some());

    let second_endpoint = second_runtime.endpoint().clone();
    let background = request(&app, "background").await;
    assert_eq!(background.status(), StatusCode::OK);
    wait_for_network_suspended(&state).await;
    wait_for_endpoint_closed(&second_endpoint).await;
    assert!(second_endpoint.is_closed());
    let foreground = request(&app, "foreground").await;
    assert_eq!(foreground.status(), StatusCode::OK);
    wait_for_device_network_ready(&state).await;
    assert_eq!(
        state
            .device_network
            .lock()
            .await
            .as_ref()
            .expect("second rebound runtime")
            .endpoint()
            .id(),
        endpoint_id
    );

    state.clear_sync_node().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mobile_foreground_ack_does_not_wait_for_repair() {
    let (app, _tmp, state) = build_app(ServerProfile::MobileApk);
    start_runtime(&state, Arc::new(AtomicUsize::new(0))).await;
    let repair_guard = state.inbound_apply_lock.lock().await;

    let response = tokio::time::timeout(Duration::from_millis(200), request(&app, "foreground"))
        .await
        .expect("foreground acknowledgement should not wait for repair");
    assert_eq!(response.status(), StatusCode::OK);
    let body = read_json(response).await;
    assert_eq!(body["action"], "foreground_resume_requested");

    drop(repair_guard);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[serial(device_network_lifecycle)]
async fn mobile_foreground_reuses_and_revalidates_an_already_connected_device_network() {
    let (app, _tmp, state) = build_paired_app(ServerProfile::MobileApk);
    state
        .ensure_sync_runtime_active()
        .await
        .expect("start paired runtime");
    let runtime = state
        .device_network
        .lock()
        .await
        .as_ref()
        .cloned()
        .expect("Device Network Runtime");
    let session_manager = runtime.session_manager().clone();
    let notifications_before = session_manager.diagnostics().network_change_notifications;
    let before = state.device_network_availability.snapshot();
    assert_eq!(
        before.state,
        rua_server::services::device_network_availability::DeviceNetworkAvailabilityState::Ready
    );

    let response = request(&app, "foreground").await;
    assert_eq!(response.status(), StatusCode::OK);
    wait_for_device_network_ready(&state).await;
    let after = state.device_network_availability.snapshot();
    assert!(after.revision > before.revision);
    assert_eq!(
        session_manager.diagnostics().network_change_notifications,
        notifications_before + 1
    );
    assert!(session_manager.same_instance(
        state
            .device_network
            .lock()
            .await
            .as_ref()
            .expect("resident Device Network Runtime")
            .session_manager()
    ));

    state.clear_sync_node().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(device_network_lifecycle)]
async fn realtime_mobile_background_keeps_device_network_resident() {
    let (app, _tmp, state) = build_paired_app(ServerProfile::MobileApk);
    set_policy(&state, SyncPowerPolicyMode::Realtime);
    state
        .ensure_sync_runtime_active()
        .await
        .expect("start paired runtime");
    let first_runtime = state
        .device_network
        .lock()
        .await
        .as_ref()
        .cloned()
        .expect("Device Network Runtime");
    let endpoint = first_runtime.endpoint().clone();
    let session_manager = first_runtime.session_manager().clone();
    let notifications_before = session_manager.diagnostics().network_change_notifications;

    let background = request(&app, "background").await;
    assert_eq!(background.status(), StatusCode::OK);
    let body = read_json(background).await;
    assert_eq!(body["action"], "background_runtime_retained");
    let residency = state.foreground_operations.snapshot();
    assert!(residency.locks_required);
    assert_eq!(
        residency.residency_state,
        rua_server::services::foreground_operation::MobileResidencyState::ResidentIdle
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(state.sync_runtime.state().await, SyncRuntimeState::Active);
    assert!(!endpoint.is_closed());
    assert!(state.device_network.lock().await.is_some());
    assert!(state.sync_node.lock().await.is_some());

    let foreground = request(&app, "foreground").await;
    assert_eq!(foreground.status(), StatusCode::OK);
    let body = read_json(foreground).await;
    assert_eq!(body["action"], "foreground_resume_requested");
    wait_for_device_network_ready(&state).await;
    let resumed_runtime = state
        .device_network
        .lock()
        .await
        .as_ref()
        .cloned()
        .expect("resident Device Network Runtime");
    assert!(session_manager.same_instance(resumed_runtime.session_manager()));
    assert_eq!(
        session_manager.diagnostics().network_change_notifications,
        notifications_before + 1
    );
    assert!(!endpoint.is_closed());

    state.clear_sync_node().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[serial(device_network_lifecycle)]
async fn active_transfer_lease_keeps_mobile_network_until_final_release() {
    let (app, _tmp, state) = build_paired_app(ServerProfile::MobileApk);
    set_background_grace(&state, Duration::from_secs(1));
    state
        .ensure_sync_runtime_active()
        .await
        .expect("start paired runtime");
    let endpoint = state
        .device_network
        .lock()
        .await
        .as_ref()
        .expect("Device Network Runtime")
        .endpoint()
        .clone();
    assert!(state.foreground_operations.acquire_transfer(
        "job:background-transfer".to_string(),
        "background-transfer".to_string(),
        "device-a".to_string(),
        "device-b".to_string(),
        100,
    ));

    let background = request(&app, "background").await;
    assert_eq!(background.status(), StatusCode::OK);
    let body = read_json(background).await;
    assert_eq!(
        body["action"],
        "background_runtime_retained_for_foreground_operation"
    );
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert!(!endpoint.is_closed());
    assert!(state.device_network.lock().await.is_some());

    assert!(state
        .foreground_operations
        .release("job:background-transfer"));
    let release_deadline = Instant::now() + Duration::from_secs(1);
    while Instant::now() < release_deadline
        && state.sync_runtime.state().await
            != (SyncRuntimeState::WarmSuspended {
                pending_grace: true,
            })
    {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        state.sync_runtime.state().await,
        SyncRuntimeState::WarmSuspended {
            pending_grace: true
        }
    );
    assert!(!endpoint.is_closed());
    let residency = state.foreground_operations.snapshot();
    assert!(residency.locks_required);
    assert_eq!(
        residency.residency_state,
        rua_server::services::foreground_operation::MobileResidencyState::BackgroundGrace
    );
    wait_for_network_suspended(&state).await;
    wait_for_endpoint_closed(&endpoint).await;
    assert_eq!(
        state.sync_runtime.state().await,
        SyncRuntimeState::WarmSuspended {
            pending_grace: false
        }
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(device_network_lifecycle)]
async fn active_conversation_survives_background_and_approval_resume_cancels_suspend() {
    let (app, _tmp, state) = build_paired_app(ServerProfile::MobileApk);
    set_background_grace(&state, Duration::from_secs(1));
    state
        .ensure_sync_runtime_active()
        .await
        .expect("start paired runtime");
    let endpoint = state
        .device_network
        .lock()
        .await
        .as_ref()
        .expect("Device Network Runtime")
        .endpoint()
        .clone();
    let active_turn = state
        .conversation_authority
        .start_active_turn(ConversationActiveTurnStart {
            address: rua_core::ChatAddress::agent(
                "default".to_string(),
                "chat-background".to_string(),
            ),
            driver: ConversationDriverKind::Rua,
            authority_device_id: "device-mobile".to_string(),
            authority_epoch: ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
            driver_detail: None,
        })
        .await;

    let background = request(&app, "background").await;
    assert_eq!(background.status(), StatusCode::OK);
    assert_eq!(
        read_json(background).await["action"],
        "background_runtime_retained_for_foreground_operation"
    );
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert!(!endpoint.is_closed());

    state
        .conversation_authority
        .transition_active_turn_phase(
            &rua_core::ChatAddress::agent("default", "chat-background"),
            Some(&active_turn.turn_id),
            ConversationActiveTurnPhase::WaitingApproval,
        )
        .await
        .expect("waiting approval");
    let grace_deadline = Instant::now() + Duration::from_secs(1);
    while Instant::now() < grace_deadline
        && state.sync_runtime.state().await
            != (SyncRuntimeState::WarmSuspended {
                pending_grace: true,
            })
    {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        state.sync_runtime.state().await,
        SyncRuntimeState::WarmSuspended {
            pending_grace: true
        }
    );

    state
        .conversation_authority
        .transition_active_turn_phase(
            &rua_core::ChatAddress::agent("default", "chat-background"),
            Some(&active_turn.turn_id),
            ConversationActiveTurnPhase::RunningTool,
        )
        .await
        .expect("approval resume");
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert!(!endpoint.is_closed());
    assert!(state.device_network.lock().await.is_some());

    state
        .conversation_authority
        .transition_active_turn_phase(
            &rua_core::ChatAddress::agent("default", "chat-background"),
            Some(&active_turn.turn_id),
            ConversationActiveTurnPhase::WaitingApproval,
        )
        .await
        .expect("second approval wait");
    wait_for_network_suspended(&state).await;
    wait_for_endpoint_closed(&endpoint).await;
    state
        .conversation_authority
        .finish_active_turn(&rua_core::ChatAddress::agent("default", "chat-background"))
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(device_network_lifecycle)]
async fn realtime_does_not_suspend_when_final_transfer_lease_is_released_in_background() {
    let (app, _tmp, state) = build_paired_app(ServerProfile::MobileApk);
    set_policy(&state, SyncPowerPolicyMode::Realtime);
    state
        .ensure_sync_runtime_active()
        .await
        .expect("start paired runtime");
    let endpoint = state
        .device_network
        .lock()
        .await
        .as_ref()
        .expect("Device Network Runtime")
        .endpoint()
        .clone();
    assert!(state.foreground_operations.acquire_transfer(
        "job:realtime-transfer".to_string(),
        "realtime-transfer".to_string(),
        "device-a".to_string(),
        "device-b".to_string(),
        10,
    ));

    assert_eq!(request(&app, "background").await.status(), StatusCode::OK);
    assert!(state.foreground_operations.release("job:realtime-transfer"));
    tokio::time::sleep(Duration::from_millis(100)).await;

    assert!(!endpoint.is_closed());
    assert!(state.device_network.lock().await.is_some());
    let residency = state.foreground_operations.snapshot();
    assert!(residency.locks_required);
    assert_eq!(
        residency.residency_state,
        rua_server::services::foreground_operation::MobileResidencyState::ResidentIdle
    );
    state.clear_sync_node().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn foreground_operation_snapshot_and_event_stream_publish_transfer_progress() {
    let (app, _tmp, state) = build_app(ServerProfile::MobileApk);
    let response = get(&app, "/api/platform/foreground-operations/events").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "application/x-ndjson");
    let mut stream = response.into_body().into_data_stream();
    let initial = tokio::time::timeout(Duration::from_secs(1), stream.next())
        .await
        .expect("initial foreground operation event")
        .expect("initial event item")
        .expect("initial event bytes");
    let initial: Value = serde_json::from_slice(&initial).expect("initial event JSON");
    assert_eq!(initial["active_count"], 0);

    assert!(state.foreground_operations.acquire_transfer(
        "job:event-transfer".to_string(),
        "event-transfer".to_string(),
        "device-a".to_string(),
        "device-b".to_string(),
        100,
    ));
    state
        .foreground_operations
        .update_transfer("job:event-transfer", 25, 100);
    let event = tokio::time::timeout(Duration::from_secs(1), stream.next())
        .await
        .expect("transfer foreground operation event")
        .expect("transfer event item")
        .expect("transfer event bytes");
    let event: Value = serde_json::from_slice(&event).expect("transfer event JSON");
    assert_eq!(event["active_count"], 1);

    let snapshot = read_json(get(&app, "/api/platform/foreground-operations").await).await;
    assert_eq!(snapshot["operations"][0]["bytes_written"], 25);
    assert_eq!(snapshot["operations"][0]["kind"], "airdrop_transfer");
    assert_eq!(snapshot["locks_required"], true);
    assert_eq!(snapshot["operations"][0]["total_bytes"], 100);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn device_network_availability_snapshot_and_events_are_ordered() {
    let (app, _tmp, state) = build_app(ServerProfile::MobileApk);
    let initial = read_json(get(&app, "/api/platform/device-network-availability").await).await;
    let initial_revision = initial["revision"]
        .as_u64()
        .expect("initial availability revision");
    assert_eq!(initial["state"], "suspended");

    state.device_network_availability.mark_starting();
    let snapshot = read_json(get(&app, "/api/platform/device-network-availability").await).await;
    let starting_revision = snapshot["revision"]
        .as_u64()
        .expect("starting availability revision");
    assert!(starting_revision > initial_revision);
    assert_eq!(snapshot["state"], "starting");

    let response = get(&app, "/api/platform/device-network-availability/events").await;
    assert_eq!(response.headers()["content-type"], "text/event-stream");
    let mut stream = response.into_body().into_data_stream();
    let first = tokio::time::timeout(Duration::from_secs(1), stream.next())
        .await
        .expect("initial availability event")
        .expect("availability stream item")
        .expect("availability stream bytes");
    let text = String::from_utf8(first.to_vec()).expect("availability event UTF-8");
    assert!(text.contains("event: device_network_availability"));
    assert!(text.contains(&format!("\"revision\":{starting_revision}")));
    assert!(text.contains("\"state\":\"starting\""));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_user_notification_is_streamed_once_with_payload_unchanged() {
    let (app, _tmp, state) = build_app(ServerProfile::Desktop);
    let response = get(&app, "/api/platform/user-notifications/events").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "text/event-stream");
    let mut stream = response.into_body().into_data_stream();
    let target_device_id =
        rua_server::services::device_context::current_sync_device_id(&state.sync_device)
            .expect("local Device id");
    let request = UserNotificationRequest {
        target_device_id,
        sender: UserNotificationSender {
            id: "agent:researcher".to_string(),
            name: "Research Agent".to_string(),
        },
        title: "Run complete".to_string(),
        body: Some("The requested analysis is ready.".to_string()),
        rua_uri: Some("rua://agents/researcher/chats/chat-42?turn=7#result".to_string()),
    };

    state.user_notifications.send(request.clone()).await;

    let event = tokio::time::timeout(Duration::from_secs(1), stream.next())
        .await
        .expect("user notification event")
        .expect("notification stream item")
        .expect("notification event bytes");
    let text = String::from_utf8(event.to_vec()).expect("notification event UTF-8");
    assert!(text.contains("event: user_notification"));
    let data = text
        .lines()
        .find_map(|line| line.strip_prefix("data: "))
        .expect("notification event data");
    let actual: UserNotificationRequest =
        serde_json::from_str(data).expect("notification event JSON");
    assert_eq!(actual, request);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn desktop_background_is_accepted_without_warm_suspend() {
    let (app, _tmp, state) = build_app(ServerProfile::Desktop);
    start_runtime(&state, Arc::new(AtomicUsize::new(0))).await;

    let resp = request(&app, "background").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["host_profile"], "desktop");
    assert_eq!(body["action"], "accepted_no_suspend");
    assert_eq!(body["runtime"]["state"], "active");

    tokio::time::sleep(Duration::from_millis(25)).await;
    assert_eq!(state.sync_runtime.state().await, SyncRuntimeState::Active);
    assert!(state.sync_runtime.trigger_handle().await.is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn desktop_foreground_dispatches_resume_hint_without_warm_suspend_policy() {
    let (app, _tmp, state) = build_app(ServerProfile::Desktop);
    let cycles = Arc::new(AtomicUsize::new(0));
    start_runtime(&state, Arc::clone(&cycles)).await;

    let resp = request(&app, "foreground").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["host_profile"], "desktop");
    assert_eq!(body["action"], "foreground_resume_hint_triggered");
    assert_eq!(body["runtime"]["state"], "active");
    wait_for_cycles(&cycles, 1).await;

    assert_eq!(state.sync_runtime.state().await, SyncRuntimeState::Active);
    assert!(state.sync_runtime.trigger_handle().await.is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn terminal_requests_wait_for_an_in_progress_foreground_rebind() {
    use rua_server::services::device_command_bus::{TerminalDeviceAction, TerminalDeviceCommand};
    use rua_server::services::terminal::{TerminalRemoteActivityRequest, TerminalStreamCursor};

    let (_app, _tmp, state) = build_app(ServerProfile::MobileApk);
    state.device_network_availability.mark_starting();
    let command = state.terminal_remote_client.deliver(TerminalDeviceCommand {
        command_id: "foreground-command".into(),
        source_device_id: "source".into(),
        target_device_id: "target".into(),
        action: TerminalDeviceAction::ListProfiles,
    });
    let activity = state
        .terminal_remote_activity_client
        .subscribe(TerminalRemoteActivityRequest {
            source_device_id: "source".into(),
            target_device_id: "target".into(),
            session_id: "foreground-terminal".into(),
            viewer_id: "viewer".into(),
            retain_authority: true,
            cursor: TerminalStreamCursor {
                next_activity_sequence: 0,
                next_output_offset: 0,
            },
        });
    tokio::pin!(command, activity);
    assert!(
        tokio::time::timeout(Duration::from_millis(30), &mut command)
            .await
            .is_err(),
        "terminal command rejected the foreground runtime before rebind completed"
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(30), &mut activity)
            .await
            .is_err(),
        "terminal activity rejected the foreground runtime before rebind completed"
    );
    state
        .device_network_availability
        .mark_failed("acceptance rebind failure");
    assert!(tokio::time::timeout(Duration::from_secs(1), command)
        .await
        .unwrap()
        .is_err());
    assert!(tokio::time::timeout(Duration::from_secs(1), activity)
        .await
        .unwrap()
        .is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn document_foreground_does_not_start_or_rebind_mobile_runtime() {
    let (app, _tmp, state) = build_app(ServerProfile::MobileApk);
    state.device_network_availability.mark_suspended();
    let before = state.device_network_availability.snapshot();
    let response = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/platform/lifecycle")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"event":"foreground","scope":"document"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(state.device_network_availability.snapshot(), before);
    assert!(state.device_network.lock().await.is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(device_network_lifecycle)]
async fn network_change_keeps_the_runtime_and_does_not_resume_a_suspended_host() {
    let (app, _tmp, state) = build_paired_app(ServerProfile::MobileApk);
    state.ensure_sync_runtime_active().await.unwrap();
    let endpoint_id = state
        .device_network
        .lock()
        .await
        .as_ref()
        .unwrap()
        .endpoint()
        .id();
    for active in [true, false] {
        if !active {
            state.sync_runtime.warm_suspend().await.unwrap();
            state.suspend_device_network_runtime().await;
        }
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/platform/network-change")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = read_json(response).await;
        assert_eq!(body["notified"], active);
        if active {
            assert_eq!(
                state
                    .device_network
                    .lock()
                    .await
                    .as_ref()
                    .unwrap()
                    .endpoint()
                    .id(),
                endpoint_id
            );
        } else {
            assert!(state.device_network.lock().await.is_none());
            assert!(matches!(
                state.sync_runtime.state().await,
                SyncRuntimeState::WarmSuspended { .. }
            ));
        }
    }
}
