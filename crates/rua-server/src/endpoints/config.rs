use std::path::{Path, PathBuf};

use axum::extract::{Path as AxumPath, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, head, post};
use axum::{Json, Router};
use rua_agent::profile_store::AgentProfileStore;
use rua_channel::core::console::{ConsoleChatRequest, ConsoleSubmitKind};
use rua_config::{atomic_write_json, AgentProfileConfig, RootConfig};
use serde::Deserialize;
use serde_json::json;

use crate::endpoints::dto;
use crate::errors::{ApiError, ValidationError};
use crate::state::AppState;
use crate::user_timezone::detected_user_timezone;

pub(crate) async fn workspace_working_dir(state: &AppState) -> Result<PathBuf, ApiError> {
    let runtime = state.registry.get_or_create("default").await.map_err(|e| {
        ApiError::Internal(format!(
            "failed to obtain workspace runtime for config: {e}"
        ))
    })?;
    Ok(runtime.rua_root())
}

fn root_config_path(working_dir: &Path) -> PathBuf {
    working_dir.join("config.json")
}

pub(crate) fn load_root_config(working_dir: &Path) -> Result<RootConfig, ApiError> {
    let path = root_config_path(working_dir);
    match std::fs::read(&path) {
        Ok(bytes) if bytes.is_empty() => Ok(RootConfig::default()),
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|e| ApiError::Internal(format!("failed to parse config.json: {e}"))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(RootConfig::default()),
        Err(e) => Err(ApiError::Internal(format!(
            "failed to read config.json: {e}"
        ))),
    }
}

pub(crate) fn save_root_config(working_dir: &Path, config: &RootConfig) -> Result<(), ApiError> {
    let value = serde_json::to_value(config)
        .map_err(|e| ApiError::Internal(format!("failed to serialize config: {e}")))?;
    atomic_write_json(&root_config_path(working_dir), &value)
        .map_err(|e| ApiError::Internal(format!("failed to write config.json: {e}")))
}

pub(crate) fn agent_profile_store(working_dir: &Path) -> AgentProfileStore {
    AgentProfileStore::new(working_dir.to_path_buf())
}

#[allow(dead_code)]
pub(crate) fn agent_json_path(working_dir: &Path, agent_id: Option<&str>) -> PathBuf {
    agent_profile_store(working_dir).agent_json_path(agent_id)
}

#[allow(dead_code)]
pub(crate) async fn load_agent_profile(
    working_dir: &Path,
    agent_id: Option<&str>,
    agent_cells: &rua_agent::AgentCells,
) -> Result<AgentProfileConfig, ApiError> {
    agent_profile_store(working_dir)
        .load_agent_config_via_cells(agent_cells, agent_id)
        .await
        .map_err(|e| ApiError::Internal(format!("failed to load agent.json: {e}")))
}

#[allow(dead_code)]
pub(crate) fn save_agent_profile(
    working_dir: &Path,
    agent_id: Option<&str>,
    config: &AgentProfileConfig,
) -> Result<(), ApiError> {
    let path = agent_json_path(working_dir, agent_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            ApiError::Internal(format!(
                "failed to create directory {}: {e}",
                parent.display()
            ))
        })?;
    }
    let value = serde_json::to_value(config)
        .map_err(|e| ApiError::Internal(format!("failed to serialize agent config: {e}")))?;
    atomic_write_json(&path, &value)
        .map_err(|e| ApiError::Internal(format!("failed to write {}: {e}", path.display())))
}

pub(crate) async fn read_heartbeat_cells(
    agent_cells: &rua_agent::AgentCells,
    agent_id: &str,
) -> Result<dto::HeartbeatConfig, ApiError> {
    let value: Option<serde_json::Value> = agent_cells
        .section::<serde_json::Value>(agent_id, "heartbeat")
        .read()
        .await
        .map_err(|e| ApiError::Internal(format!("failed to read heartbeat cell: {e}")))?;
    match value {
        None | Some(serde_json::Value::Null) => Ok(empty_heartbeat()),
        Some(value) => serde_json::from_value(value).map_err(|e| {
            ApiError::Internal(format!(
                "failed to parse heartbeat cell for {agent_id}: {e}"
            ))
        }),
    }
}

pub(crate) async fn write_heartbeat_cells(
    agent_cells: &rua_agent::AgentCells,
    agent_id: &str,
    config: dto::HeartbeatConfig,
) -> Result<dto::HeartbeatConfig, ApiError> {
    agent_cells
        .section::<dto::HeartbeatConfig>(agent_id, "heartbeat")
        .write(config.clone())
        .await
        .map_err(|e| ApiError::Internal(format!("failed to write heartbeat cell: {e}")))?;
    Ok(config)
}

pub(crate) async fn trigger_manual_heartbeat_run(
    state: &AppState,
    agent_id: String,
) -> Result<(), ApiError> {
    let runtime = state.registry.get_or_create(&agent_id).await.map_err(|e| {
        ApiError::Internal(format!(
            "failed to obtain workspace runtime for heartbeat: {e}"
        ))
    })?;
    let prompt = match rua_cron::heartbeat::load_heartbeat_md(&runtime.rua_root()) {
        Some(prompt) if !prompt.trim().is_empty() => prompt,
        _ => {
            tracing::debug!(agent_id = %agent_id, "manual heartbeat skipped: no HEARTBEAT.md");
            return Ok(());
        }
    };

    let outcome = state
        .console_facade
        .submit_chat(
            &agent_id,
            ConsoleChatRequest {
                message: prompt,
                chat_id: None,
                model: None,
                is_reconnect: false,
                attachments: Vec::new(),
                input: None,
                messages: None,
                extra: serde_json::Map::from_iter([(
                    "request_context".to_string(),
                    json!({ "source": "heartbeat" }),
                )]),
                channel_kind: None,
            },
        )
        .await;
    if !outcome.accepted || matches!(outcome.kind, ConsoleSubmitKind::Rejected) {
        return Err(ApiError::Unavailable(outcome.detail.unwrap_or_else(|| {
            "manual heartbeat input was rejected".to_string()
        })));
    }

    Ok(())
}

fn empty_heartbeat() -> dto::HeartbeatConfig {
    dto::HeartbeatConfig {
        enabled: false,
        every: String::new(),
        target: String::new(),
        active_hours: None,
        extra: serde_json::Map::new(),
    }
}

pub(crate) fn read_user_timezone(working_dir: &Path) -> Result<dto::UserTimezoneConfig, ApiError> {
    let config = load_root_config(working_dir)?;
    let timezone = config
        .user_timezone
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(detected_user_timezone);
    Ok(dto::UserTimezoneConfig {
        timezone,
        extra: serde_json::Map::new(),
    })
}

pub(crate) fn write_user_timezone(
    working_dir: &Path,
    config: dto::UserTimezoneConfig,
) -> Result<dto::UserTimezoneConfig, ApiError> {
    let mut root = load_root_config(working_dir)?;
    root.user_timezone = Some(config.timezone.trim().to_string());
    save_root_config(working_dir, &root)?;
    read_user_timezone(working_dir)
}

pub fn agent_scoped_config_router() -> Router<AppState> {
    Router::new()
        .route(
            "/agents/{agentId}/config/heartbeat",
            get(get_heartbeat).put(update_heartbeat),
        )
        .route(
            "/agents/{agentId}/config/heartbeat/run",
            post(run_heartbeat_now),
        )
        .route(
            "/agents/{agentId}/config/user-timezone",
            get(get_user_timezone).put(update_user_timezone),
        )
        .route(
            "/agents/{agentId}/config/agents/llm-routing",
            get(get_llm_routing).put(update_llm_routing),
        )
        .route(
            "/agents/{agentId}/config/security/tool-guard",
            get(get_tool_guard).put(update_tool_guard),
        )
        .route(
            "/agents/{agentId}/config/security/tool-guard/builtin-rules",
            get(get_builtin_rules),
        )
        .route(
            "/agents/{agentId}/config/security/approval-mode",
            get(get_approval_mode).put(update_approval_mode),
        )
        .route(
            "/agents/{agentId}/config/security/file-guard",
            get(get_file_guard).put(update_file_guard),
        )
        .route(
            "/agents/{agentId}/config/security/skill-scanner",
            get(get_skill_scanner).put(update_skill_scanner),
        )
        .route(
            "/agents/{agentId}/config/security/skill-scanner/blocked-history",
            get(get_blocked_history).delete(clear_blocked_history),
        )
        .route(
            "/agents/{agentId}/config/security/skill-scanner/blocked-history/{index}",
            delete(delete_blocked_history_entry),
        )
        .route(
            "/agents/{agentId}/config/security/skill-scanner/whitelist",
            post(add_whitelist),
        )
        .route(
            "/agents/{agentId}/config/security/skill-scanner/whitelist/{skill_name}",
            delete(remove_whitelist),
        )
        .route(
            "/agents/{agentId}/config/security/allow-no-auth-hosts",
            get(get_allow_no_auth_hosts).put(update_allow_no_auth_hosts),
        )
}

pub fn file_preview_router() -> Router<AppState> {
    Router::new()
        .route("/agents/{agentId}/files/preview", get(agent_preview_file))
        .route("/files/preview/{*filepath}", get(preview_file))
        .route("/files/preview/{*filepath}", head(preview_file_head))
}

async fn get_heartbeat(
    State(state): State<AppState>,
    AxumPath(agent_id): AxumPath<String>,
) -> Response {
    match read_heartbeat_cells(&state.agent_cells, &agent_id).await {
        Ok(config) => (StatusCode::OK, Json(config)).into_response(),
        Err(e) => e.into_response(),
    }
}

async fn update_heartbeat(
    State(state): State<AppState>,
    AxumPath(agent_id): AxumPath<String>,
    Json(body): Json<dto::HeartbeatConfig>,
) -> Response {
    match write_heartbeat_cells(&state.agent_cells, &agent_id, body).await {
        Ok(config) => {
            let path = state
                .rua_root
                .join("workspaces")
                .join(&agent_id)
                .join("agent.json");
            crate::sync_publication::publish_agent_config_file_committed(
                &state,
                &agent_id,
                &path,
                "heartbeat agent config",
            )
            .await;
            (StatusCode::OK, Json(config)).into_response()
        }
        Err(e) => e.into_response(),
    }
}

async fn run_heartbeat_now(
    State(state): State<AppState>,
    AxumPath(agent_id): AxumPath<String>,
) -> Response {
    match trigger_manual_heartbeat_run(&state, agent_id).await {
        Ok(()) => (StatusCode::OK, Json(json!({ "started": true }))).into_response(),
        Err(e) => e.into_response(),
    }
}

async fn get_user_timezone(
    State(state): State<AppState>,
    AxumPath(_agent_id): AxumPath<String>,
) -> Response {
    let working_dir = match workspace_working_dir(&state).await {
        Ok(p) => p,
        Err(e) => return e.into_response(),
    };
    match read_user_timezone(&working_dir) {
        Ok(config) => (StatusCode::OK, Json(config)).into_response(),
        Err(e) => e.into_response(),
    }
}

async fn update_user_timezone(
    State(state): State<AppState>,
    AxumPath(_agent_id): AxumPath<String>,
    Json(body): Json<dto::UserTimezoneConfig>,
) -> Response {
    if body.timezone.trim().is_empty() {
        return ApiError::BadRequest("timezone is required".into()).into_response();
    }
    let working_dir = match workspace_working_dir(&state).await {
        Ok(p) => p,
        Err(e) => return e.into_response(),
    };
    match write_user_timezone(&working_dir, body) {
        Ok(config) => {
            let _ = crate::sync_publication::publish_sync_fact(
                &state,
                crate::sync_publication::SyncPublicationFact::RootUserTimezoneCommitted {
                    timezone: serde_json::Value::String(config.timezone.clone()),
                },
            )
            .await;
            (StatusCode::OK, Json(config)).into_response()
        }
        Err(e) => e.into_response(),
    }
}

async fn get_llm_routing(
    State(state): State<AppState>,
    AxumPath(agent_id): AxumPath<String>,
) -> Response {
    match read_llm_routing_cells(&state.agent_cells, &agent_id).await {
        Ok(config) => (StatusCode::OK, Json(config)).into_response(),
        Err(e) => e.into_response(),
    }
}

async fn update_llm_routing(
    State(state): State<AppState>,
    AxumPath(agent_id): AxumPath<String>,
    Json(body): Json<dto::LlmRoutingConfig>,
) -> Response {
    if !matches!(body.mode.as_str(), "local_first" | "cloud_first") {
        return ApiError::BadRequest("Invalid llm routing mode".into()).into_response();
    }
    match write_llm_routing_cells(&state.agent_cells, &agent_id, body).await {
        Ok(config) => {
            let path = state
                .rua_root
                .join("workspaces")
                .join(&agent_id)
                .join("agent.json");
            crate::sync_publication::publish_agent_config_file_committed(
                &state,
                &agent_id,
                &path,
                "llm routing agent config",
            )
            .await;
            (StatusCode::OK, Json(config)).into_response()
        }
        Err(e) => e.into_response(),
    }
}

pub(crate) async fn read_llm_routing_cells(
    agent_cells: &rua_agent::AgentCells,
    agent_id: &str,
) -> Result<dto::LlmRoutingConfig, ApiError> {
    let value: Option<serde_json::Value> = agent_cells
        .section::<serde_json::Value>(agent_id, "llm_routing")
        .read()
        .await
        .map_err(|e| ApiError::Internal(format!("failed to read llm_routing cell: {e}")))?;
    match value {
        None | Some(serde_json::Value::Null) => Ok(dto::LlmRoutingConfig::default()),
        Some(value) => serde_json::from_value(value).map_err(|e| {
            ApiError::Internal(format!(
                "failed to parse llm_routing cell for {agent_id}: {e}"
            ))
        }),
    }
}

pub(crate) async fn write_llm_routing_cells(
    agent_cells: &rua_agent::AgentCells,
    agent_id: &str,
    config: dto::LlmRoutingConfig,
) -> Result<dto::LlmRoutingConfig, ApiError> {
    agent_cells
        .section::<dto::LlmRoutingConfig>(agent_id, "llm_routing")
        .write(config.clone())
        .await
        .map_err(|e| ApiError::Internal(format!("failed to write llm_routing cell: {e}")))?;
    Ok(config)
}

use crate::endpoints::security::{
    approval_mode_to_dto, build_store, builtin_rules_payload, canonical_approval_mode,
    dto_to_skill_scanner, dto_to_tool_guard, get_allow_no_auth_hosts_from_store,
    get_file_guard_for_agent, map_store_error, publish_process_security_config,
    skill_scanner_to_dto, tool_guard_to_dto, update_allow_no_auth_hosts_in_store,
    update_file_guard_for_agent, validate_tool_guard_config,
};

async fn get_tool_guard(
    State(state): State<AppState>,
    AxumPath(agent_id): AxumPath<String>,
) -> Response {
    let store = state.security.config_store();
    match store.load(&agent_id) {
        Ok(cfg) => (StatusCode::OK, Json(tool_guard_to_dto(&cfg.tool_guard))).into_response(),
        Err(e) => map_store_error(e).into_response(),
    }
}

async fn update_tool_guard(
    State(state): State<AppState>,
    AxumPath(agent_id): AxumPath<String>,
    Json(body): Json<dto::ToolGuardConfig>,
) -> Response {
    if let Err(e) = validate_tool_guard_config(&body) {
        return e.into_response();
    }

    let store = state.security.config_store();
    let mut cfg = match store.load(&agent_id) {
        Ok(c) => c,
        Err(e) => return map_store_error(e).into_response(),
    };
    cfg.tool_guard = dto_to_tool_guard(&body, &cfg.tool_guard);
    if let Err(e) = state.security.save_and_invalidate(&agent_id, &cfg) {
        return map_store_error(e).into_response();
    }
    crate::sync_publication::publish_agent_security_committed(
        &state,
        &agent_id,
        serde_json::to_value(&cfg).unwrap_or_default(),
    )
    .await;
    (StatusCode::OK, Json(tool_guard_to_dto(&cfg.tool_guard))).into_response()
}

async fn get_builtin_rules(
    State(_state): State<AppState>,
    AxumPath(_agent_id): AxumPath<String>,
) -> Response {
    (StatusCode::OK, Json(builtin_rules_payload())).into_response()
}

async fn get_approval_mode(
    State(state): State<AppState>,
    AxumPath(agent_id): AxumPath<String>,
) -> Response {
    let store = state.security.config_store();
    match store.load(&agent_id) {
        Ok(cfg) => (
            StatusCode::OK,
            Json(approval_mode_to_dto(&cfg.tool_guard.approval_mode)),
        )
            .into_response(),
        Err(e) => map_store_error(e).into_response(),
    }
}

async fn update_approval_mode(
    State(state): State<AppState>,
    AxumPath(agent_id): AxumPath<String>,
    Json(body): Json<dto::ApprovalModeConfig>,
) -> Response {
    let Some(mode) = canonical_approval_mode(&body.approval_mode) else {
        return ApiError::Validation(vec![ValidationError {
            loc: vec!["body".to_string(), "approval_mode".to_string()],
            msg: "approval_mode must be one of ask_first, hands_off, just_do_it".to_string(),
            error_type: "value_error".to_string(),
        }])
        .into_response();
    };

    let store = state.security.config_store();
    let mut cfg = match store.load(&agent_id) {
        Ok(cfg) => cfg,
        Err(e) => return map_store_error(e).into_response(),
    };
    cfg.tool_guard.approval_mode = mode.to_string();
    if let Err(e) = state.security.save_and_invalidate(&agent_id, &cfg) {
        return map_store_error(e).into_response();
    }
    crate::sync_publication::publish_agent_security_committed(
        &state,
        &agent_id,
        serde_json::to_value(&cfg).unwrap_or_default(),
    )
    .await;
    (StatusCode::OK, Json(approval_mode_to_dto(mode))).into_response()
}

async fn get_file_guard(
    State(state): State<AppState>,
    AxumPath(agent_id): AxumPath<String>,
) -> Response {
    get_file_guard_for_agent(&state, &agent_id)
}

async fn get_allow_no_auth_hosts(
    State(state): State<AppState>,
    AxumPath(_agent_id): AxumPath<String>,
) -> Response {
    let store = match build_store(&state).await {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    match get_allow_no_auth_hosts_from_store(&store) {
        Ok(config) => (StatusCode::OK, Json(config)).into_response(),
        Err(e) => e.into_response(),
    }
}

async fn update_allow_no_auth_hosts(
    State(state): State<AppState>,
    AxumPath(_agent_id): AxumPath<String>,
    Json(body): Json<dto::AllowNoAuthHostsConfig>,
) -> Response {
    let store = match build_store(&state).await {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    match update_allow_no_auth_hosts_in_store(&store, body) {
        Ok(config) => (StatusCode::OK, Json(config)).into_response(),
        Err(e) => e.into_response(),
    }
}

async fn update_file_guard(
    State(state): State<AppState>,
    AxumPath(agent_id): AxumPath<String>,
    Json(body): Json<dto::FileGuardConfig>,
) -> Response {
    update_file_guard_for_agent(&state, &agent_id, body).await
}

async fn get_skill_scanner(
    State(state): State<AppState>,
    AxumPath(_agent_id): AxumPath<String>,
) -> Response {
    let store = match build_store(&state).await {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    match store.get_skill_scanner() {
        Ok(cfg) => (StatusCode::OK, Json(skill_scanner_to_dto(&cfg))).into_response(),
        Err(e) => map_store_error(e).into_response(),
    }
}

async fn update_skill_scanner(
    State(state): State<AppState>,
    AxumPath(_agent_id): AxumPath<String>,
    Json(body): Json<dto::SkillScannerConfig>,
) -> Response {
    let store = match build_store(&state).await {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    let existing = match store.get_skill_scanner() {
        Ok(c) => c,
        Err(e) => return map_store_error(e).into_response(),
    };
    let merged = dto_to_skill_scanner(&body, &existing);
    match store.update_skill_scanner(merged) {
        Ok(saved) => {
            publish_process_security_config(&state, &store).await;
            (StatusCode::OK, Json(skill_scanner_to_dto(&saved))).into_response()
        }
        Err(e) => map_store_error(e).into_response(),
    }
}

async fn get_blocked_history(
    State(state): State<AppState>,
    AxumPath(_agent_id): AxumPath<String>,
) -> Response {
    let store = match build_store(&state).await {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    match store.get_blocked_history() {
        Ok(values) => {
            let entries: Vec<dto::BlockedHistoryEntry> = values
                .into_iter()
                .filter_map(|v| serde_json::from_value(v).ok())
                .collect();
            (StatusCode::OK, Json(entries)).into_response()
        }
        Err(e) => map_store_error(e).into_response(),
    }
}

async fn clear_blocked_history(
    State(state): State<AppState>,
    AxumPath(_agent_id): AxumPath<String>,
) -> Response {
    let store = match build_store(&state).await {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    match store.clear_blocked_history() {
        Ok(()) => {
            let resp = dto::SuccessResponse::ok();
            (StatusCode::OK, Json(resp)).into_response()
        }
        Err(e) => map_store_error(e).into_response(),
    }
}

async fn delete_blocked_history_entry(
    State(state): State<AppState>,
    AxumPath((_agent_id, index)): AxumPath<(String, usize)>,
) -> Response {
    let store = match build_store(&state).await {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    match store.delete_blocked_history_entry(index) {
        Ok(()) => {
            let resp = dto::SuccessResponse::ok();
            (StatusCode::OK, Json(resp)).into_response()
        }
        Err(e) => map_store_error(e).into_response(),
    }
}

async fn add_whitelist(
    State(state): State<AppState>,
    AxumPath(_agent_id): AxumPath<String>,
    Json(body): Json<dto::SkillWhitelistEntry>,
) -> Response {
    if body.skill_name.is_empty() {
        return ApiError::BadRequest("skill_name is required".into()).into_response();
    }
    let store = match build_store(&state).await {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    let entry = rua_config::types::security::SkillScannerWhitelistEntry {
        skill_name: body.skill_name.clone(),
        content_hash: body.content_hash.clone(),
        added_at: String::new(),
        extra: body.extra.clone(),
    };
    match store.add_whitelist(entry) {
        Ok(saved) => {
            publish_process_security_config(&state, &store).await;
            let resp = dto::SkillWhitelistEntry {
                skill_name: saved.skill_name,
                content_hash: saved.content_hash,
                added_at: saved.added_at,
                extra: saved.extra,
            };
            (StatusCode::CREATED, Json(resp)).into_response()
        }
        Err(e) => map_store_error(e).into_response(),
    }
}

async fn remove_whitelist(
    State(state): State<AppState>,
    AxumPath((_agent_id, skill_name)): AxumPath<(String, String)>,
) -> Response {
    let store = match build_store(&state).await {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    match store.remove_whitelist(&skill_name) {
        Ok(()) => {
            publish_process_security_config(&state, &store).await;
            let resp = dto::SuccessResponse::ok();
            (StatusCode::OK, Json(resp)).into_response()
        }
        Err(e) => map_store_error(e).into_response(),
    }
}

fn guess_content_type(path: &Path) -> &'static str {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase);
    match extension.as_deref() {
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js" | "mjs") => "application/javascript; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("xml") => "application/xml; charset=utf-8",
        Some("txt" | "log" | "md" | "yaml" | "yml" | "toml" | "cfg" | "ini" | "conf") => {
            "text/plain; charset=utf-8"
        }
        Some("csv") => "text/csv; charset=utf-8",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("svg") => "image/svg+xml",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        Some("pdf") => "application/pdf",
        Some("zip") => "application/zip",
        Some("wasm") => "application/wasm",
        _ => "application/octet-stream",
    }
}

fn content_disposition(path: &Path, kind: &str) -> String {
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("download")
        .replace(['"', '\r', '\n'], "_");
    format!("{kind}; filename=\"{filename}\"")
}

fn preview_content_disposition(path: &Path) -> String {
    content_disposition(path, "attachment")
}

fn image_preview_content_disposition(path: &Path) -> String {
    content_disposition(path, "inline")
}

fn resolve_preview_path(filepath: &str) -> PathBuf {
    let p = PathBuf::from(filepath);
    if p.is_absolute() {
        p
    } else {
        PathBuf::from(format!("/{filepath}"))
    }
}

#[derive(Deserialize)]
struct AgentPreviewQuery {
    path: String,
}

fn expand_preview_tilde(source: &str) -> Option<PathBuf> {
    let home = rua_config::dirs::home_dir();
    if source == "~" {
        return Some(home);
    }
    source.strip_prefix("~/").map(|rest| home.join(rest))
}

async fn agent_preview_file(
    State(state): State<AppState>,
    AxumPath(agent_id): AxumPath<String>,
    Query(query): Query<AgentPreviewQuery>,
) -> Response {
    let source_text = query.path.strip_prefix("file://").unwrap_or(&query.path);
    let source = PathBuf::from(source_text);
    let path = if let Some(expanded) = expand_preview_tilde(source_text) {
        expanded
    } else if source.is_absolute() {
        source
    } else {
        let runtime = match state.registry.get_or_create(&agent_id).await {
            Ok(runtime) => runtime,
            Err(error) => {
                return ApiError::Unavailable(format!(
                    "workspace for '{agent_id}' unavailable: {error}"
                ))
                .into_response();
            }
        };
        runtime.workspace_dir().join(source)
    };

    if !path.exists() {
        return ApiError::NotFound(format!("File not found: {}", query.path)).into_response();
    }

    if !path.is_file() {
        return ApiError::BadRequest("Path is not a file".into()).into_response();
    }

    let content_type = guess_content_type(&path);
    if !content_type.starts_with("image/") {
        return ApiError::BadRequest("Path is not a supported image".into()).into_response();
    }

    match std::fs::read(&path) {
        Ok(bytes) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, content_type.to_string()),
                (
                    header::CONTENT_DISPOSITION,
                    image_preview_content_disposition(&path),
                ),
            ],
            bytes,
        )
            .into_response(),
        Err(e) => ApiError::Internal(format!("Failed to read file: {e}")).into_response(),
    }
}

async fn preview_file(AxumPath(filepath): AxumPath<String>) -> Response {
    let path = resolve_preview_path(&filepath);

    if !path.exists() {
        return ApiError::NotFound(format!("File not found: {filepath}")).into_response();
    }

    if !path.is_file() {
        return ApiError::BadRequest("Path is not a file".into()).into_response();
    }

    match std::fs::read(&path) {
        Ok(bytes) => {
            let content_type = guess_content_type(&path);
            (
                StatusCode::OK,
                [
                    (header::CONTENT_TYPE, content_type.to_string()),
                    (
                        header::CONTENT_DISPOSITION,
                        preview_content_disposition(&path),
                    ),
                ],
                bytes,
            )
                .into_response()
        }
        Err(e) => ApiError::Internal(format!("Failed to read file: {e}")).into_response(),
    }
}

async fn preview_file_head(AxumPath(filepath): AxumPath<String>) -> Response {
    let path = resolve_preview_path(&filepath);

    if !path.exists() {
        return StatusCode::NOT_FOUND.into_response();
    }

    if !path.is_file() {
        return StatusCode::BAD_REQUEST.into_response();
    }

    let content_type = guess_content_type(&path);
    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);

    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, content_type.to_string()),
            (header::CONTENT_LENGTH, size.to_string()),
            (
                header::CONTENT_DISPOSITION,
                preview_content_disposition(&path),
            ),
        ],
    )
        .into_response()
}
