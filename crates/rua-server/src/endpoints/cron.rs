use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use rua_core::RuaError;
use serde::Deserialize;
use std::collections::{BTreeSet, HashSet};
use std::sync::Arc;
use tracing::{info, warn};

use rua_config::types::cron::{
    CronJobSpec, DispatchSpec, DispatchTarget, JobRuntimeSpec, ScheduleSpec,
};

use crate::endpoints::dto;
use crate::errors::ApiError;
use crate::extractors::AgentExtractorContext;
use crate::services::chat_index;
use crate::services::cron_dispatch_history::{
    record_cron_dispatch_result, CronDispatchHistoryEntry, CronDispatchHistoryStore,
};
use crate::state::AppState;

pub fn root_router() -> Router<AppState> {
    Router::new()
        .route("/cron/jobs", get(list_jobs).post(create_job))
        .route("/cron/dispatch-targets", get(list_dispatch_targets))
        .route(
            "/cron/jobs/{jobId}",
            get(get_job).put(update_job).delete(delete_job),
        )
        .route("/cron/jobs/{jobId}/pause", post(pause_job))
        .route("/cron/jobs/{jobId}/resume", post(resume_job))
        .route("/cron/jobs/{jobId}/run", post(run_job))
        .route("/cron/jobs/{jobId}/state", get(get_job_state))
        .route("/cron/jobs/{jobId}/history", get(get_job_history))
}

pub fn agent_scoped_router() -> Router<AppState> {
    Router::new()
        .route(
            "/agents/{agentId}/cron/jobs",
            get(list_jobs_agent).post(create_job_agent),
        )
        .route(
            "/agents/{agentId}/cron/dispatch-targets",
            get(list_dispatch_targets_agent),
        )
        .route(
            "/agents/{agentId}/cron/jobs/{jobId}",
            get(get_job_agent)
                .put(update_job_agent)
                .delete(delete_job_agent),
        )
        .route(
            "/agents/{agentId}/cron/jobs/{jobId}/pause",
            post(pause_job_agent),
        )
        .route(
            "/agents/{agentId}/cron/jobs/{jobId}/resume",
            post(resume_job_agent),
        )
        .route(
            "/agents/{agentId}/cron/jobs/{jobId}/run",
            post(run_job_agent),
        )
        .route(
            "/agents/{agentId}/cron/jobs/{jobId}/state",
            get(get_job_state_agent),
        )
        .route(
            "/agents/{agentId}/cron/jobs/{jobId}/history",
            get(get_job_history_agent),
        )
}

#[derive(Debug, Clone, Deserialize)]
struct DispatchTargetsQuery {
    #[serde(default)]
    channel: Option<String>,
    #[serde(default)]
    keyword: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

fn map_cells_error(e: RuaError) -> ApiError {
    ApiError::Internal(format!("cron cell store error: {e}"))
}

fn spec_to_dto(spec: &CronJobSpec) -> dto::CronJob {
    let schedule = dto::CronScheduleSpec {
        schedule_type: spec.schedule.r#type.clone(),
        cron: if !spec.schedule.cron.is_empty() {
            spec.schedule.cron.clone()
        } else {
            spec.cron_expr.clone().unwrap_or_default()
        },
        run_at: spec.schedule.run_at,
        timezone: spec.schedule.timezone.clone(),
        repeat_every_days: spec.schedule.repeat_every_days,
        repeat_end_type: spec.schedule.repeat_end_type.clone(),
        repeat_until: spec.schedule.repeat_until,
        repeat_count: spec.schedule.repeat_count,
    };

    let dispatch = match &spec.dispatch {
        Some(d) => dto::CronDispatchSpec {
            dispatch_type: d.r#type.clone(),
            channel: d.channel.clone(),
            target: d
                .target
                .as_ref()
                .map(|t| dto::CronDispatchTarget {
                    user_id: t.user_id.clone(),
                    session_id: t.session_id.clone(),
                })
                .unwrap_or_default(),
            mode: d.mode.clone(),
            meta: d.meta.clone(),
        },
        None => dto::CronDispatchSpec::default(),
    };

    let runtime = spec
        .runtime
        .as_ref()
        .map(|r| dto::CronJobRuntimeSpec {
            max_concurrency: Some(r.max_concurrency),
            timeout_seconds: Some(r.timeout_seconds),
            misfire_grace_seconds: Some(r.misfire_grace_seconds),
            share_session: Some(r.share_session),
            tool_safety: Some(r.tool_safety),
        })
        .unwrap_or_default();

    let text = spec.text.clone().or_else(|| spec.prompt.clone());

    dto::CronJob {
        id: spec.id.clone(),
        name: spec.name.clone(),
        enabled: spec.enabled,
        schedule,
        task_type: spec.task_type.clone(),
        text,
        request: spec.request.clone(),
        save_result_to_inbox: spec.save_result_to_inbox,
        dispatch,
        runtime,
        meta: spec.meta.clone(),
        extra: spec.extra.clone(),
    }
}

fn create_to_spec(create: dto::CronJobCreate, agent_id: &str) -> CronJobSpec {
    let dispatch_target = if create.dispatch.target.user_id.is_empty()
        && create.dispatch.target.session_id.is_empty()
    {
        None
    } else {
        Some(DispatchTarget {
            user_id: create.dispatch.target.user_id,
            session_id: create.dispatch.target.session_id,
            extra: serde_json::Map::new(),
        })
    };

    CronJobSpec {
        id: uuid::Uuid::new_v4().to_string(),
        name: create.name,
        enabled: create.enabled.unwrap_or(true),
        paused: false,
        schedule: ScheduleSpec {
            r#type: if create.schedule.schedule_type.is_empty() {
                "cron".into()
            } else {
                create.schedule.schedule_type
            },
            cron: create.schedule.cron,
            run_at: create.schedule.run_at,
            timezone: if create.schedule.timezone.is_empty() {
                "UTC".into()
            } else {
                create.schedule.timezone
            },
            repeat_every_days: create.schedule.repeat_every_days,
            repeat_end_type: create.schedule.repeat_end_type,
            repeat_until: create.schedule.repeat_until,
            repeat_count: create.schedule.repeat_count,
            extra: serde_json::Map::new(),
        },
        task_type: if create.task_type.is_empty() {
            "text".into()
        } else {
            create.task_type
        },
        text: create.text,
        request: create.request,
        save_result_to_inbox: create.save_result_to_inbox,
        dispatch: Some(DispatchSpec {
            r#type: if create.dispatch.dispatch_type.is_empty() {
                "channel".into()
            } else {
                create.dispatch.dispatch_type
            },
            channel: create.dispatch.channel,
            target: dispatch_target,
            mode: if create.dispatch.mode.is_empty() {
                "stream".into()
            } else {
                create.dispatch.mode
            },
            meta: create.dispatch.meta,
            extra: serde_json::Map::new(),
        }),
        runtime: create.runtime.map(|r| JobRuntimeSpec {
            max_concurrency: r.max_concurrency.unwrap_or(1),
            timeout_seconds: r.timeout_seconds.unwrap_or(120),
            misfire_grace_seconds: r.misfire_grace_seconds.unwrap_or(600),
            share_session: r.share_session.unwrap_or(true),
            tool_safety: r.tool_safety.unwrap_or(false),
            extra: serde_json::Map::new(),
        }),
        meta: create.meta.unwrap_or_default(),
        cron_expr: None,
        agent_id: Some(agent_id.to_string()),
        prompt: None,
        missed_policy: None,
        max_retries: None,
        retry_delay_secs: None,
        extra: create.extra,
    }
}

fn apply_update(spec: &mut CronJobSpec, update: dto::CronJobUpdate) {
    if let Some(name) = update.name {
        spec.name = name;
    }
    if let Some(schedule) = update.schedule {
        if !schedule.cron.is_empty() {
            spec.schedule.cron = schedule.cron;
        }
        if schedule.run_at.is_some() {
            spec.schedule.run_at = schedule.run_at;
        }
        if !schedule.timezone.is_empty() {
            spec.schedule.timezone = schedule.timezone;
        }
        if !schedule.schedule_type.is_empty() {
            spec.schedule.r#type = schedule.schedule_type;
        }
        if schedule.repeat_every_days.is_some() {
            spec.schedule.repeat_every_days = schedule.repeat_every_days;
        }
        if schedule.repeat_end_type.is_some() {
            spec.schedule.repeat_end_type = schedule.repeat_end_type;
        }
        if schedule.repeat_until.is_some() {
            spec.schedule.repeat_until = schedule.repeat_until;
        }
        if schedule.repeat_count.is_some() {
            spec.schedule.repeat_count = schedule.repeat_count;
        }
    }
    if let Some(task_type) = update.task_type {
        spec.task_type = task_type;
    }
    if update.text.is_some() {
        spec.text = update.text;
    }
    if update.request.is_some() {
        spec.request = update.request;
    }
    if update.save_result_to_inbox.is_some() {
        spec.save_result_to_inbox = update.save_result_to_inbox;
    }
    if let Some(enabled) = update.enabled {
        spec.enabled = enabled;
        if enabled {
            spec.paused = false;
        }
    }
    if let Some(dispatch) = update.dispatch {
        let target = if dispatch.target.user_id.is_empty() && dispatch.target.session_id.is_empty()
        {
            None
        } else {
            Some(DispatchTarget {
                user_id: dispatch.target.user_id,
                session_id: dispatch.target.session_id,
                extra: serde_json::Map::new(),
            })
        };
        if let Some(ref mut d) = spec.dispatch {
            if !dispatch.channel.is_empty() {
                d.channel = dispatch.channel;
            }
            if !dispatch.dispatch_type.is_empty() {
                d.r#type = dispatch.dispatch_type;
            }
            if !dispatch.mode.is_empty() {
                d.mode = dispatch.mode;
            }
            if let Some(target) = target {
                d.target = Some(target);
            }
            if !dispatch.meta.is_empty() {
                d.meta = dispatch.meta;
            }
        } else {
            spec.dispatch = Some(DispatchSpec {
                r#type: if dispatch.dispatch_type.is_empty() {
                    "channel".into()
                } else {
                    dispatch.dispatch_type
                },
                channel: dispatch.channel,
                target,
                mode: if dispatch.mode.is_empty() {
                    "stream".into()
                } else {
                    dispatch.mode
                },
                meta: dispatch.meta,
                extra: serde_json::Map::new(),
            });
        }
    }
    if let Some(runtime) = update.runtime {
        spec.runtime = Some(JobRuntimeSpec {
            max_concurrency: runtime.max_concurrency.unwrap_or(1),
            timeout_seconds: runtime.timeout_seconds.unwrap_or(120),
            misfire_grace_seconds: runtime.misfire_grace_seconds.unwrap_or(600),
            share_session: runtime.share_session.unwrap_or(true),
            tool_safety: runtime.tool_safety.unwrap_or(false),
            extra: serde_json::Map::new(),
        });
    }
    if let Some(meta) = update.meta {
        spec.meta = meta;
    }
    for (k, v) in update.extra {
        spec.extra.insert(k, v);
    }
}

fn normalize_job_spec_for_write(spec: &mut CronJobSpec) -> Result<(), ApiError> {
    if spec.schedule.r#type.is_empty() {
        spec.schedule.r#type = "cron".to_string();
    }
    if spec.schedule.timezone.is_empty() {
        spec.schedule.timezone = "UTC".to_string();
    }

    match spec.schedule.r#type.as_str() {
        "cron" => {
            if spec.schedule.cron.trim().is_empty() {
                return Err(ApiError::BadRequest(
                    "schedule.type is cron but cron is empty".to_string(),
                ));
            }
            spec.schedule.run_at = None;
            spec.schedule.repeat_every_days = None;
            spec.schedule.repeat_end_type = None;
            spec.schedule.repeat_until = None;
            spec.schedule.repeat_count = None;
        }
        "once" => {
            let Some(run_at) = spec.schedule.run_at else {
                return Err(ApiError::BadRequest(
                    "schedule.type is once but run_at is missing".to_string(),
                ));
            };
            spec.schedule.cron.clear();
            if spec.schedule.repeat_every_days.is_none() {
                spec.schedule.repeat_end_type = None;
                spec.schedule.repeat_until = None;
                spec.schedule.repeat_count = None;
            } else {
                if spec.schedule.repeat_end_type.is_none() {
                    spec.schedule.repeat_end_type = Some("never".to_string());
                }
                match spec.schedule.repeat_end_type.as_deref() {
                    Some("never") => {
                        spec.schedule.repeat_until = None;
                        spec.schedule.repeat_count = None;
                    }
                    Some("until") => {
                        let repeat_until = spec.schedule.repeat_until.ok_or_else(|| {
                            ApiError::BadRequest(
                                "repeat_end_type is until but repeat_until is missing".to_string(),
                            )
                        })?;
                        if repeat_until <= run_at {
                            return Err(ApiError::BadRequest(
                                "repeat_until must be later than run_at".to_string(),
                            ));
                        }
                        spec.schedule.repeat_count = None;
                    }
                    Some("count") => {
                        if spec.schedule.repeat_count.is_none() {
                            return Err(ApiError::BadRequest(
                                "repeat_end_type is count but repeat_count is missing".to_string(),
                            ));
                        }
                        spec.schedule.repeat_until = None;
                    }
                    Some(other) => {
                        return Err(ApiError::BadRequest(format!(
                            "unsupported repeat_end_type: {other}"
                        )));
                    }
                    None => {}
                }
            }
        }
        other => {
            return Err(ApiError::BadRequest(format!(
                "unsupported schedule.type: {other}"
            )));
        }
    }

    match spec.task_type.as_str() {
        "text" => {
            if spec
                .text
                .as_deref()
                .map(str::trim)
                .is_none_or(|text| text.is_empty())
            {
                return Err(ApiError::BadRequest(
                    "task_type is text but text is empty".to_string(),
                ));
            }
            spec.request = None;
        }
        "agent" => {
            if spec.request.is_none() {
                return Err(ApiError::BadRequest(
                    "task_type is agent but request is missing".to_string(),
                ));
            }
            if let (Some(request), Some(dispatch)) = (&mut spec.request, &spec.dispatch) {
                if let (Some(obj), Some(target)) =
                    (request.as_object_mut(), dispatch.target.as_ref())
                {
                    obj.insert(
                        "user_id".to_string(),
                        serde_json::Value::String(target.user_id.clone()),
                    );
                    obj.insert(
                        "session_id".to_string(),
                        serde_json::Value::String(target.session_id.clone()),
                    );
                }
            }
        }
        other => {
            return Err(ApiError::BadRequest(format!(
                "unsupported task_type: {other}"
            )))
        }
    }

    if spec.save_result_to_inbox.is_none() {
        spec.save_result_to_inbox =
            Some(!(spec.task_type == "text" && spec.schedule.r#type == "cron"));
    }

    if spec.runtime.is_none() {
        spec.runtime = Some(JobRuntimeSpec {
            max_concurrency: 1,
            timeout_seconds: 120,
            misfire_grace_seconds: 600,
            share_session: true,
            tool_safety: false,
            extra: serde_json::Map::new(),
        });
    }

    Ok(())
}

fn resolve_job_message(
    spec: &CronJobSpec,
    run_override: Option<&dto::CronJobRunOverride>,
) -> Result<String, ApiError> {
    if let Some(ovr) = run_override {
        if let Some(ref msg) = ovr.message {
            if !msg.is_empty() {
                return Ok(msg.clone());
            }
        }
    }

    if spec.task_type == "agent" {
        if let Some(ref request) = spec.request {
            if let Some(input) = request.get("input") {
                let s = match input {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                if !s.is_empty() {
                    return Ok(s);
                }
            }
        }
    }

    if let Some(ref text) = spec.text {
        if !text.is_empty() {
            return Ok(text.clone());
        }
    }

    Err(ApiError::BadRequest(format!(
        "Cron job '{}' has no input message",
        spec.id
    )))
}

fn is_channel_dispatch_job(spec: &CronJobSpec) -> bool {
    spec.dispatch
        .as_ref()
        .map(|dispatch| dispatch.r#type.is_empty() || dispatch.r#type == "channel")
        .unwrap_or(true)
}

fn history_entry_to_dto(entry: CronDispatchHistoryEntry) -> dto::CronDispatchHistoryEntry {
    dto::CronDispatchHistoryEntry {
        id: entry.id,
        job_id: entry.job_id,
        agent_id: entry.agent_id,
        job_name: entry.job_name,
        task_type: entry.task_type,
        channel: entry.channel,
        user_id: entry.user_id,
        session_id: entry.session_id,
        mode: entry.mode,
        status: entry.status,
        success: entry.success,
        error: entry.error,
        duration_ms: entry.duration_ms,
        completed_at: entry.completed_at,
    }
}

fn job_belongs_to_agent(spec: &CronJobSpec, agent_id: &str) -> bool {
    spec.agent_id.as_deref().unwrap_or("default") == agent_id
}

fn cron_cells_for_agent(state: &AppState, agent_id: &str) -> rua_cron::CronCells {
    rua_cron::CronCells::new(
        Arc::clone(&state.cell_store),
        Arc::clone(&state.cron_scheduler),
        state
            .rua_root
            .join("workspaces")
            .join(agent_id)
            .join("jobs.json"),
    )
}

fn cron_job_not_found(job_id: &str) -> Response {
    ApiError::NotFound(format!("Cron job '{job_id}' not found")).into_response()
}

async fn do_list_dispatch_targets(
    workspace_dir: &std::path::Path,
    query: DispatchTargetsQuery,
) -> Response {
    let channel_filter = query
        .channel
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let keyword = query
        .keyword
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_ascii_lowercase());
    let limit = query.limit.unwrap_or(500).clamp(1, 2000);

    let chats = match chat_index::list_all(workspace_dir).await {
        Ok(chats) => chats,
        Err(e) => {
            return ApiError::Internal(format!("failed to list cron dispatch targets: {e}"))
                .into_response();
        }
    };

    let mut seen = HashSet::new();
    let mut channels = BTreeSet::new();
    let mut items = Vec::new();

    for chat in chats {
        if channel_filter
            .as_deref()
            .is_some_and(|ch| chat.channel != ch)
        {
            continue;
        }
        if let Some(keyword) = &keyword {
            let haystack =
                format!("{} {} {}", chat.channel, chat.user_id, chat.id).to_ascii_lowercase();
            if !haystack.contains(keyword) {
                continue;
            }
        }

        channels.insert(chat.channel.clone());
        let key = (chat.channel.clone(), chat.user_id.clone(), chat.id.clone());
        if seen.insert(key.clone()) {
            items.push(dto::CronDispatchTargetItem {
                channel: key.0,
                user_id: key.1,
                session_id: key.2,
            });
            if items.len() >= limit {
                break;
            }
        }
    }

    channels.insert("console".to_string());
    let response = dto::CronDispatchTargetsResponse {
        channels: channels.into_iter().collect(),
        items,
    };
    (StatusCode::OK, Json(response)).into_response()
}

async fn do_list_jobs(state: &AppState, agent_id: &str) -> Response {
    let cron_cells = cron_cells_for_agent(state, agent_id);
    match cron_cells.list().await {
        Ok(jobs) => {
            let dtos: Vec<dto::CronJob> = jobs
                .iter()
                .filter(|job| job_belongs_to_agent(job, agent_id))
                .map(spec_to_dto)
                .collect();
            (StatusCode::OK, Json(dtos)).into_response()
        }
        Err(e) => map_cells_error(e).into_response(),
    }
}

async fn do_get_job(state: &AppState, agent_id: &str, job_id: &str) -> Response {
    let cron_cells = cron_cells_for_agent(state, agent_id);
    match cron_cells.cell(job_id).read().await {
        Ok(Some(spec)) if job_belongs_to_agent(&spec, agent_id) => {
            (StatusCode::OK, Json(spec_to_dto(&spec))).into_response()
        }
        Ok(Some(_)) | Ok(None) => cron_job_not_found(job_id),
        Err(e) => map_cells_error(e).into_response(),
    }
}

async fn do_create_job(state: &AppState, agent_id: &str, body: dto::CronJobCreate) -> Response {
    let mut spec = create_to_spec(body, agent_id);
    if let Err(e) = normalize_job_spec_for_write(&mut spec) {
        return e.into_response();
    }
    let cron_cells = cron_cells_for_agent(state, agent_id);
    match cron_cells.cell(&spec.id).write(spec.clone()).await {
        Ok(_) => (StatusCode::CREATED, Json(spec_to_dto(&spec))).into_response(),
        Err(e) => map_cells_error(e).into_response(),
    }
}

async fn do_update_job(
    state: &AppState,
    agent_id: &str,
    job_id: &str,
    body: dto::CronJobUpdate,
) -> Response {
    let cron_cells = cron_cells_for_agent(state, agent_id);
    let mut spec = match cron_cells.cell(job_id).read().await {
        Ok(Some(s)) if job_belongs_to_agent(&s, agent_id) => s,
        Ok(Some(_)) | Ok(None) => return cron_job_not_found(job_id),
        Err(e) => return map_cells_error(e).into_response(),
    };
    apply_update(&mut spec, body);
    if let Err(e) = normalize_job_spec_for_write(&mut spec) {
        return e.into_response();
    }
    match cron_cells.cell(job_id).write(spec.clone()).await {
        Ok(_) => (StatusCode::OK, Json(spec_to_dto(&spec))).into_response(),
        Err(e) => map_cells_error(e).into_response(),
    }
}

async fn do_delete_job(state: &AppState, agent_id: &str, job_id: &str) -> Response {
    let cron_cells = cron_cells_for_agent(state, agent_id);
    match cron_cells.cell(job_id).read().await {
        Ok(Some(spec)) if job_belongs_to_agent(&spec, agent_id) => {}
        Ok(Some(_)) | Ok(None) => return cron_job_not_found(job_id),
        Err(e) => return map_cells_error(e).into_response(),
    }
    match cron_cells.cell(job_id).delete().await {
        Ok(()) => (StatusCode::OK, Json(dto::SuccessResponse::ok())).into_response(),
        Err(e) => map_cells_error(e).into_response(),
    }
}

async fn do_set_paused(state: &AppState, agent_id: &str, job_id: &str, paused: bool) -> Response {
    let cron_cells = cron_cells_for_agent(state, agent_id);
    let current = match cron_cells.cell(job_id).read().await {
        Ok(Some(s)) if job_belongs_to_agent(&s, agent_id) => s,
        Ok(Some(_)) | Ok(None) => return cron_job_not_found(job_id),
        Err(e) => return map_cells_error(e).into_response(),
    };
    if current.paused == paused {
        return (StatusCode::OK, Json(spec_to_dto(&current))).into_response();
    }
    let result = cron_cells
        .cell(job_id)
        .update(move |existing| {
            let mut spec = existing.unwrap_or(current);
            spec.paused = paused;
            spec
        })
        .await;
    match result {
        Ok(spec) => (StatusCode::OK, Json(spec_to_dto(&spec))).into_response(),
        Err(e) => map_cells_error(e).into_response(),
    }
}

async fn do_run_job(
    state: &AppState,
    agent_id: &str,
    job_id: &str,
    run_override: Option<dto::CronJobRunOverride>,
) -> Response {
    let cron_cells = cron_cells_for_agent(state, agent_id);
    let spec = match cron_cells.cell(job_id).read().await {
        Ok(Some(s)) if job_belongs_to_agent(&s, agent_id) => s,
        Ok(Some(_)) | Ok(None) => return cron_job_not_found(job_id),
        Err(e) => return map_cells_error(e).into_response(),
    };

    info!(
        job_id = %spec.id,
        job_name = %spec.name,
        agent_id = %agent_id,
        task_type = %spec.task_type,
        enabled = %spec.enabled,
        "cron manual run: dispatching job"
    );

    let mut run_spec = spec.clone();
    if run_override.is_some() {
        let message = match resolve_job_message(&run_spec, run_override.as_ref()) {
            Ok(m) => m,
            Err(e) => return e.into_response(),
        };
        if run_spec.task_type == "agent" {
            let mut request = run_spec
                .request
                .take()
                .and_then(|value| value.as_object().cloned())
                .unwrap_or_default();
            request.insert("input".to_string(), serde_json::Value::String(message));
            run_spec.request = Some(serde_json::Value::Object(request));
        } else {
            run_spec.text = Some(message);
        }
    }

    let data_dir = state.rua_root.clone();
    let registry = Arc::clone(&state.registry);
    let push_store = state.push_message_store.clone();
    tokio::spawn(async move {
        let channel_sender: Arc<dyn rua_cron::CronChannelSender> = Arc::new(
            crate::cron_bootstrap::WorkspaceCronChannelSender::new(Arc::clone(&registry))
                .with_push_store(push_store),
        );
        let agent_runner: Arc<dyn rua_cron::CronAgentRunner> = Arc::new(
            crate::cron_bootstrap::WorkspaceCronAgentRunner::new(Arc::clone(&registry)),
        );
        let executor = rua_cron::CronExecutor::new(channel_sender, agent_runner);
        let result = executor.execute(&run_spec).await;
        if result.success {
            info!(job_id = %run_spec.id, "cron manual run completed: status=success");
        } else {
            warn!(
                job_id = %run_spec.id,
                error = ?result.error,
                "cron manual run failed"
            );
        }
        if is_channel_dispatch_job(&run_spec) {
            record_cron_dispatch_result(
                &data_dir,
                &run_spec,
                result.success,
                result.error.clone(),
                result.duration_ms,
            );
        }
    });

    (StatusCode::OK, Json(serde_json::json!({ "started": true }))).into_response()
}

async fn do_get_job_state(state: &AppState, agent_id: &str, job_id: &str) -> Response {
    let cron_cells = cron_cells_for_agent(state, agent_id);
    match cron_cells.cell(job_id).read().await {
        Ok(Some(spec)) if job_belongs_to_agent(&spec, agent_id) => {
            let history =
                CronDispatchHistoryStore::new(state.rua_root.clone()).list_for_job(job_id);
            let latest = history.last().cloned();
            let next_run_at = if spec.enabled && !spec.paused {
                rua_cron::compute_next_run_for_schedule(&spec.schedule)
                    .map(|time| time.to_rfc3339())
            } else {
                None
            };
            let state = dto::CronJobState {
                next_run_at,
                last_run_at: latest.as_ref().map(|entry| entry.completed_at.clone()),
                last_status: latest.as_ref().map(|entry| entry.status.clone()),
                last_error: latest.as_ref().and_then(|entry| entry.error.clone()),
                history: history.into_iter().map(history_entry_to_dto).collect(),
            };
            (StatusCode::OK, Json(state)).into_response()
        }
        Ok(Some(_)) | Ok(None) => cron_job_not_found(job_id),
        Err(e) => map_cells_error(e).into_response(),
    }
}

async fn do_get_job_history(state: &AppState, agent_id: &str, job_id: &str) -> Response {
    let cron_cells = cron_cells_for_agent(state, agent_id);
    match cron_cells.cell(job_id).read().await {
        Ok(Some(spec)) if job_belongs_to_agent(&spec, agent_id) => {
            let history =
                CronDispatchHistoryStore::new(state.rua_root.clone()).list_for_job(job_id);
            let items: Vec<_> = history.into_iter().map(history_entry_to_dto).collect();
            (StatusCode::OK, Json(items)).into_response()
        }
        Ok(Some(_)) | Ok(None) => cron_job_not_found(job_id),
        Err(e) => map_cells_error(e).into_response(),
    }
}

async fn list_jobs(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
) -> Response {
    do_list_jobs(&state, &ctx.agent_id).await
}

async fn list_dispatch_targets(
    Extension(ctx): Extension<AgentExtractorContext>,
    Query(query): Query<DispatchTargetsQuery>,
) -> Response {
    do_list_dispatch_targets(&ctx.runtime.workspace_dir(), query).await
}

async fn get_job(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
    Path(job_id): Path<String>,
) -> Response {
    do_get_job(&state, &ctx.agent_id, &job_id).await
}

async fn create_job(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
    Json(body): Json<dto::CronJobCreate>,
) -> Response {
    do_create_job(&state, &ctx.agent_id, body).await
}

async fn update_job(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
    Path(job_id): Path<String>,
    Json(body): Json<dto::CronJobUpdate>,
) -> Response {
    do_update_job(&state, &ctx.agent_id, &job_id, body).await
}

async fn delete_job(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
    Path(job_id): Path<String>,
) -> Response {
    do_delete_job(&state, &ctx.agent_id, &job_id).await
}

async fn pause_job(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
    Path(job_id): Path<String>,
) -> Response {
    do_set_paused(&state, &ctx.agent_id, &job_id, true).await
}

async fn resume_job(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
    Path(job_id): Path<String>,
) -> Response {
    do_set_paused(&state, &ctx.agent_id, &job_id, false).await
}

async fn run_job(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
    Path(job_id): Path<String>,
    body: Option<Json<dto::CronJobRunOverride>>,
) -> Response {
    let run_override = body.map(|Json(b)| b);
    do_run_job(&state, &ctx.agent_id, &job_id, run_override).await
}

async fn get_job_state(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
    Path(job_id): Path<String>,
) -> Response {
    do_get_job_state(&state, &ctx.agent_id, &job_id).await
}

async fn get_job_history(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
    Path(job_id): Path<String>,
) -> Response {
    do_get_job_history(&state, &ctx.agent_id, &job_id).await
}

async fn list_jobs_agent(State(state): State<AppState>, Path(agent_id): Path<String>) -> Response {
    do_list_jobs(&state, &agent_id).await
}

async fn list_dispatch_targets_agent(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Query(query): Query<DispatchTargetsQuery>,
) -> Response {
    let runtime = match state.registry.get_or_create(&agent_id).await {
        Ok(runtime) => runtime,
        Err(err) => {
            return ApiError::Internal(format!("Agent '{agent_id}' is unavailable: {err}"))
                .into_response();
        }
    };
    do_list_dispatch_targets(&runtime.workspace_dir(), query).await
}

async fn get_job_agent(
    State(state): State<AppState>,
    Path((agent_id, job_id)): Path<(String, String)>,
) -> Response {
    do_get_job(&state, &agent_id, &job_id).await
}

async fn create_job_agent(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Json(body): Json<dto::CronJobCreate>,
) -> Response {
    do_create_job(&state, &agent_id, body).await
}

async fn update_job_agent(
    State(state): State<AppState>,
    Path((agent_id, job_id)): Path<(String, String)>,
    Json(body): Json<dto::CronJobUpdate>,
) -> Response {
    do_update_job(&state, &agent_id, &job_id, body).await
}

async fn delete_job_agent(
    State(state): State<AppState>,
    Path((agent_id, job_id)): Path<(String, String)>,
) -> Response {
    do_delete_job(&state, &agent_id, &job_id).await
}

async fn pause_job_agent(
    State(state): State<AppState>,
    Path((agent_id, job_id)): Path<(String, String)>,
) -> Response {
    do_set_paused(&state, &agent_id, &job_id, true).await
}

async fn resume_job_agent(
    State(state): State<AppState>,
    Path((agent_id, job_id)): Path<(String, String)>,
) -> Response {
    do_set_paused(&state, &agent_id, &job_id, false).await
}

async fn run_job_agent(
    State(state): State<AppState>,
    Path((agent_id, job_id)): Path<(String, String)>,
    body: Option<Json<dto::CronJobRunOverride>>,
) -> Response {
    let run_override = body.map(|Json(b)| b);
    do_run_job(&state, &agent_id, &job_id, run_override).await
}

async fn get_job_state_agent(
    State(state): State<AppState>,
    Path((agent_id, job_id)): Path<(String, String)>,
) -> Response {
    do_get_job_state(&state, &agent_id, &job_id).await
}

async fn get_job_history_agent(
    State(state): State<AppState>,
    Path((agent_id, job_id)): Path<(String, String)>,
) -> Response {
    do_get_job_history(&state, &agent_id, &job_id).await
}
