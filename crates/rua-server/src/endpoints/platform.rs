use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::header;
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};

use futures::stream::unfold;
use serde::{Deserialize, Serialize};

use crate::state::AppState;
use crate::sync_runtime_controller::{SyncRuntimeError, SyncRuntimeState};

pub fn router() -> Router<AppState> {
    Router::new()
        .merge(super::desktop_automation::router())
        .merge(super::desktop_control::router())
        .merge(super::phone_use::router())
        .route("/platform/lifecycle", post(handle_lifecycle))
        .route(
            "/platform/conversation-activity",
            get(stream_conversation_activity),
        )
        .route(
            "/platform/foreground-operations",
            get(get_foreground_operations),
        )
        .route(
            "/platform/foreground-operations/events",
            get(stream_foreground_operations),
        )
        .route(
            "/platform/device-network-availability",
            get(get_device_network_availability),
        )
        .route(
            "/platform/device-network-availability/events",
            get(stream_device_network_availability),
        )
        .route(
            "/platform/user-notifications/events",
            get(stream_user_notifications),
        )
}

#[derive(Deserialize)]
struct LifecycleRequest {
    event: LifecycleEvent,
    #[serde(default)]
    scope: LifecycleScope,
}

#[derive(Default, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum LifecycleScope {
    #[default]
    Host,
    Document,
}

#[derive(Deserialize, Serialize, Debug, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum LifecycleEvent {
    Foreground,
    Background,
}

#[derive(Serialize)]
struct LifecycleResponse {
    accepted: bool,
    event: LifecycleEvent,
    host_profile: rua_sync::power_policy::SyncRuntimeHostProfile,
    action: &'static str,
    runtime: RuntimeStateResponse,
}

#[derive(Serialize)]
struct RuntimeStateResponse {
    state: &'static str,
    pending_grace: bool,
}

impl From<SyncRuntimeState> for RuntimeStateResponse {
    fn from(state: SyncRuntimeState) -> Self {
        match state {
            SyncRuntimeState::Unavailable => Self {
                state: "unavailable",
                pending_grace: false,
            },
            SyncRuntimeState::NotPaired => Self {
                state: "not_paired",
                pending_grace: false,
            },
            SyncRuntimeState::Active => Self {
                state: "active",
                pending_grace: false,
            },
            SyncRuntimeState::WarmSuspended { pending_grace } => Self {
                state: "warm_suspended",
                pending_grace,
            },
        }
    }
}

#[derive(Serialize)]
struct LifecycleErrorResponse {
    reason: &'static str,
    detail: String,
}

fn sync_runtime_error_response(error: SyncRuntimeError) -> Response {
    let status = match error.reason() {
        "not_paired" | "runtime_suspended" | "runtime_unavailable" => {
            StatusCode::UNPROCESSABLE_ENTITY
        }
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (
        status,
        Json(LifecycleErrorResponse {
            reason: error.reason(),
            detail: error.detail().to_string(),
        }),
    )
        .into_response()
}

async fn handle_lifecycle(
    state: axum::extract::State<AppState>,
    Json(body): Json<LifecycleRequest>,
) -> Response {
    let host_profile = state.sync_runtime_host_profile();
    if body.scope == LifecycleScope::Document
        || host_profile != rua_sync::power_policy::SyncRuntimeHostProfile::MobileApk
    {
        return handle_process_running_lifecycle(state.0, body.event, host_profile).await;
    }

    match body.event {
        LifecycleEvent::Foreground => handle_mobile_foreground(state.0, host_profile).await,
        LifecycleEvent::Background => handle_mobile_background(state.0, host_profile).await,
    }
}

async fn handle_process_running_lifecycle(
    state: AppState,
    event: LifecycleEvent,
    host_profile: rua_sync::power_policy::SyncRuntimeHostProfile,
) -> Response {
    let action = match event {
        LifecycleEvent::Foreground => {
            if let Some(handle) = state.sync_runtime.trigger_handle().await {
                handle.notify_resume();
                "foreground_resume_hint_triggered"
            } else {
                "accepted_no_suspend"
            }
        }
        LifecycleEvent::Background => "accepted_no_suspend",
    };

    lifecycle_response(
        event,
        host_profile,
        action,
        state.sync_runtime.state().await,
    )
}

async fn handle_mobile_foreground(
    state: AppState,
    host_profile: rua_sync::power_policy::SyncRuntimeHostProfile,
) -> Response {
    state.foreground_operations.enter_foreground();
    let needs_rebind = state.device_network_availability.snapshot().state
        != crate::services::device_network_availability::DeviceNetworkAvailabilityState::Ready;
    state.device_network_availability.mark_starting();
    let resume_state = state.clone();
    tokio::spawn(async move {
        if needs_rebind {
            resume_state.sync_runtime.shutdown_unavailable().await;
        }
        let app_state = resume_state.clone();
        if let Err(error) = resume_state
            .sync_runtime
            .notify_foreground(move || async move { app_state.build_sync_runtime_parts().await })
            .await
        {
            if resume_state.device_network.lock().await.is_none() {
                resume_state
                    .device_network_availability
                    .mark_failed(error.detail());
                tracing::warn!(
                    reason = error.reason(),
                    detail = error.detail(),
                    "[platform-lifecycle] foreground Device Network resume failed"
                );
                return;
            }
            tracing::warn!(
                reason = error.reason(),
                detail = error.detail(),
                "[platform-lifecycle] foreground Sync resume failed; revalidating resident Device Network"
            );
        }
        let runtime = resume_state.device_network.lock().await.as_ref().cloned();
        let Some(runtime) = runtime else {
            resume_state
                .device_network_availability
                .mark_failed("Device Network Runtime is not initialized after foreground resume");
            return;
        };
        let report = runtime.session_manager().notify_network_change().await;
        tracing::debug!(
            probed_sessions = report.probed_sessions,
            invalidated_sessions = report.invalidated_sessions,
            "[platform-lifecycle] foreground Device Session recovery completed"
        );
        resume_state.device_network_availability.mark_ready();
        if let Err(error) = resume_state.repair_scan_and_drain_sync_inbox().await {
            tracing::warn!(
                error = %error,
                "[platform-lifecycle] foreground repair scan/drain failed"
            );
        }
        if let Err(error) = resume_state.sync_runtime.trigger_app_resume().await {
            tracing::warn!(
                reason = error.reason(),
                detail = error.detail(),
                "[platform-lifecycle] foreground app-resume Sync trigger failed"
            );
        }
    });

    lifecycle_response(
        LifecycleEvent::Foreground,
        host_profile,
        "foreground_resume_requested",
        state.sync_runtime.state().await,
    )
}

async fn handle_mobile_background(
    state: AppState,
    host_profile: rua_sync::power_policy::SyncRuntimeHostProfile,
) -> Response {
    let policy = state.effective_sync_power_policy();
    let grace_generation = state
        .foreground_operations
        .enter_background(policy.allow_background_runtime);
    if policy.allow_background_runtime {
        return lifecycle_response(
            LifecycleEvent::Background,
            host_profile,
            "background_runtime_retained",
            state.sync_runtime.state().await,
        );
    }

    if state.foreground_operations.has_active() {
        if let Err(error) = state.retain_device_network_for_foreground_operation().await {
            return sync_runtime_error_response(error);
        }
        return lifecycle_response(
            LifecycleEvent::Background,
            host_profile,
            "background_runtime_retained_for_foreground_operation",
            state.sync_runtime.state().await,
        );
    }

    let grace = Duration::from_secs(policy.background_flush_grace_secs);
    let app_state = state.clone();
    if let Some(grace_generation) = grace_generation {
        let foreground_operations = Arc::clone(&state.foreground_operations);
        if let Err(e) = state
            .sync_runtime
            .notify_background_with_suspend(grace, move || async move {
                app_state.suspend_device_network_runtime().await;
                foreground_operations.mark_suspended(grace_generation);
            })
            .await
        {
            return sync_runtime_error_response(e);
        }
    }

    lifecycle_response(
        LifecycleEvent::Background,
        host_profile,
        "background_grace_started",
        state.sync_runtime.state().await,
    )
}

async fn get_foreground_operations(
    state: axum::extract::State<AppState>,
) -> Json<crate::services::foreground_operation::ForegroundOperationSnapshot> {
    Json(state.foreground_operations.snapshot())
}

async fn stream_foreground_operations(state: axum::extract::State<AppState>) -> Response {
    let snapshots = state.foreground_operations.subscribe();
    let stream = unfold((snapshots, true), |(mut snapshots, initial)| async move {
        if !initial && snapshots.changed().await.is_err() {
            return None;
        }
        let mut line = serde_json::to_vec(&*snapshots.borrow()).ok()?;
        line.push(b'\n');
        Some((Ok::<_, std::convert::Infallible>(line), (snapshots, false)))
    });
    Response::builder()
        .header(header::CONTENT_TYPE, "application/x-ndjson")
        .header(header::CACHE_CONTROL, "no-cache")
        .body(Body::from_stream(stream))
        .expect("foreground operation stream response")
}

async fn get_device_network_availability(
    state: axum::extract::State<AppState>,
) -> Json<crate::services::device_network_availability::DeviceNetworkAvailabilitySnapshot> {
    Json(state.device_network_availability.snapshot())
}

async fn stream_device_network_availability(state: axum::extract::State<AppState>) -> Response {
    let snapshots = state.device_network_availability.subscribe();
    let stream = unfold((snapshots, true), |(mut snapshots, initial)| async move {
        if !initial && snapshots.changed().await.is_err() {
            return None;
        }
        let data = serde_json::to_string(&*snapshots.borrow()).ok()?;
        Some((
            Ok::<_, std::convert::Infallible>(
                Event::default()
                    .event("device_network_availability")
                    .data(data),
            ),
            (snapshots, false),
        ))
    });
    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

async fn stream_user_notifications(state: axum::extract::State<AppState>) -> Response {
    let notifications = state.user_notification_events.subscribe();
    let stream = unfold(notifications, |mut notifications| async move {
        loop {
            match notifications.recv().await {
                Ok(notification) => {
                    let data = serde_json::to_string(&notification).ok()?;
                    return Some((
                        Ok::<_, std::convert::Infallible>(
                            Event::default().event("user_notification").data(data),
                        ),
                        notifications,
                    ));
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
            }
        }
    });
    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

fn lifecycle_response(
    event: LifecycleEvent,
    host_profile: rua_sync::power_policy::SyncRuntimeHostProfile,
    action: &'static str,
    runtime: SyncRuntimeState,
) -> Response {
    (
        StatusCode::OK,
        Json(LifecycleResponse {
            accepted: true,
            event,
            host_profile,
            action,
            runtime: runtime.into(),
        }),
    )
        .into_response()
}

async fn stream_conversation_activity(state: axum::extract::State<AppState>) -> Response {
    let hub = Arc::clone(&state.chat_activity_wake_hub.navigation);
    let (snapshot, events) = hub.subscribe();
    let stream = unfold(
        (Some(snapshot), events, hub, state.shutdown_rx()),
        |(mut initial, mut events, hub, mut shutdown)| async move {
            if *shutdown.borrow() {
                return None;
            }
            let event = match initial.take() {
                Some(snapshot) => snapshot,
                None => loop {
                    tokio::select! {
                        changed = shutdown.changed() => {
                            if changed.is_err() || *shutdown.borrow() { return None; }
                        }
                        event = events.recv() => match event {
                            Ok(event) => break event,
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                                let (snapshot, receiver) = hub.subscribe();
                                events = receiver;
                                break snapshot;
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
                        },
                    }
                },
            };
            let mut line = serde_json::to_vec(&event).ok()?;
            line.push(b'\n');
            Some((
                Ok::<_, std::convert::Infallible>(line),
                (initial, events, hub, shutdown),
            ))
        },
    );
    Response::builder()
        .header(header::CONTENT_TYPE, "application/x-ndjson")
        .header(header::CACHE_CONTROL, "no-cache")
        .body(Body::from_stream(stream))
        .expect("conversation navigation activity response")
}
