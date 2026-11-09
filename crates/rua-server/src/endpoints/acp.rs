use std::collections::{BTreeMap, HashMap};
use std::path::Path as FsPath;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post};
use axum::{Extension, Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::process::Command;

use rua_acp::client::AcpClientAdapter;
use rua_config::types::acp::default_builtin_runners;
use rua_config::types::{AcpAgentConfig, AcpConfig, ToolParseMode};
use rua_core::runtime_authority::{
    ProcessShape, RuntimeAuthority, RuntimeLaunchPlan, RuntimeLaunchRequest, ToolchainIntent,
};

use crate::errors::ApiError;
use crate::install_mirror::{detect_install_mirror_config_with_locale_values, InstallMirrorConfig};
use crate::middleware::AgentExtractorContext;
use crate::state::AppState;

const ACP_RUNNER_CHECK_TIMEOUT: Duration = Duration::from_secs(15);
const ACP_RUNNER_INSTALL_TIMEOUT: Duration = Duration::from_secs(300);
pub fn root_router() -> Router<AppState> {
    Router::new()
        .route("/acp", get(list_runners).post(create_runner))
        .route(
            "/acp/{runner_key}",
            get(get_runner).put(update_runner).delete(delete_runner),
        )
        .route("/acp/{runner_key}/check", get(check_runner))
        .route("/acp/{runner_key}/install", post(install_runner))
        .route(
            "/acp/{runner_key}/install/start",
            post(start_install_runner),
        )
        .route("/acp/install-tasks/{task_id}", get(get_install_task))
        .route("/acp/{runner_key}/toggle", patch(toggle_runner))
}

pub fn agent_scoped_router() -> Router<AppState> {
    Router::new()
        .route(
            "/agents/{agentId}/acp",
            get(list_runners_agent).post(create_runner_agent),
        )
        .route(
            "/agents/{agentId}/acp/{runner_key}",
            get(get_runner_agent)
                .put(update_runner_agent)
                .delete(delete_runner_agent),
        )
        .route(
            "/agents/{agentId}/acp/{runner_key}/check",
            get(check_runner_agent),
        )
        .route(
            "/agents/{agentId}/acp/{runner_key}/install",
            post(install_runner_agent),
        )
        .route(
            "/agents/{agentId}/acp/{runner_key}/install/start",
            post(start_install_runner_agent),
        )
        .route(
            "/agents/{agentId}/acp/install-tasks/{task_id}",
            get(get_install_task_agent),
        )
        .route(
            "/agents/{agentId}/acp/{runner_key}/toggle",
            patch(toggle_runner_agent),
        )
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpRunnerCreate {
    pub key: String,
    #[serde(flatten)]
    pub config: AcpAgentConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpRunnerUpdate {
    pub enabled: Option<bool>,
    pub command: Option<String>,
    pub args: Option<Vec<String>>,
    pub env: Option<HashMap<String, String>>,
    pub trusted: Option<bool>,
    pub tool_parse_mode: Option<ToolParseMode>,
    #[serde(default, deserialize_with = "deserialize_present_nullable")]
    pub stdio_buffer_limit_bytes: Option<Option<u64>>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

fn deserialize_present_nullable<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

impl AcpRunnerUpdate {
    fn apply_to(self, config: &mut AcpAgentConfig) {
        if let Some(enabled) = self.enabled {
            config.enabled = enabled;
        }
        if let Some(command) = self.command {
            config.command = command;
        }
        if let Some(args) = self.args {
            config.args = args;
        }
        if let Some(env) = self.env {
            config.env = env;
        }
        if let Some(trusted) = self.trusted {
            config.trusted = trusted;
        }
        if let Some(tool_parse_mode) = self.tool_parse_mode {
            config.tool_parse_mode = tool_parse_mode;
        }
        if let Some(stdio_buffer_limit_bytes) = self.stdio_buffer_limit_bytes {
            config.stdio_buffer_limit_bytes = stdio_buffer_limit_bytes;
        }
        config.extra.extend(self.extra);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpRunnerCheckResult {
    pub ok: bool,
    pub status: String,
    pub message: String,
    pub requires_auth: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpRunnerInstallResult {
    pub installed: bool,
    pub message: String,
    pub check_result: AcpRunnerCheckResult,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpRunnerInstallTask {
    pub task_id: String,
    pub agent_id: String,
    pub runner_key: String,
    pub status: String,
    pub stage: String,
    pub message: String,
    pub detail: Option<String>,
    pub installed: bool,
    pub check_result: Option<AcpRunnerCheckResult>,
    pub created_at: f64,
    pub updated_at: f64,
}

fn resolve_agent_id(agent_id: Option<&str>) -> &str {
    agent_id.unwrap_or("default")
}

fn is_builtin(key: &str) -> bool {
    default_builtin_runners().contains_key(key)
}

fn install_task_registry() -> &'static Mutex<HashMap<String, AcpRunnerInstallTask>> {
    static TASKS: OnceLock<Mutex<HashMap<String, AcpRunnerInstallTask>>> = OnceLock::new();
    TASKS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs_f64())
        .unwrap_or(0.0)
}

fn create_install_task(agent_id: &str, runner_key: &str) -> AcpRunnerInstallTask {
    let now = now_secs();
    let task = AcpRunnerInstallTask {
        task_id: uuid::Uuid::new_v4().to_string(),
        agent_id: agent_id.to_string(),
        runner_key: runner_key.to_string(),
        status: "pending".to_string(),
        stage: "queued".to_string(),
        message: format!("Queued ACP runner '{runner_key}' install"),
        detail: None,
        installed: false,
        check_result: None,
        created_at: now,
        updated_at: now,
    };
    install_task_registry()
        .lock()
        .expect("acp install task registry poisoned")
        .insert(task.task_id.clone(), task.clone());
    task
}

fn update_install_task(
    task_id: &str,
    status: &str,
    stage: &str,
    message: impl Into<String>,
    detail: Option<String>,
) {
    let mut tasks = install_task_registry()
        .lock()
        .expect("acp install task registry poisoned");
    if let Some(task) = tasks.get_mut(task_id) {
        task.status = status.to_string();
        task.stage = stage.to_string();
        task.message = message.into();
        task.detail = detail;
        task.updated_at = now_secs();
    }
}

fn complete_install_task(task_id: &str, result: AcpRunnerInstallResult) {
    let mut tasks = install_task_registry()
        .lock()
        .expect("acp install task registry poisoned");
    if let Some(task) = tasks.get_mut(task_id) {
        task.status = "completed".to_string();
        task.stage = "completed".to_string();
        task.message = result.message;
        task.detail = None;
        task.installed = result.installed;
        task.check_result = Some(result.check_result);
        task.updated_at = now_secs();
    }
}

fn fail_install_task(task_id: &str, error: ApiError) {
    let detail = error.to_string();
    let mut tasks = install_task_registry()
        .lock()
        .expect("acp install task registry poisoned");
    if let Some(task) = tasks.get_mut(task_id) {
        task.status = "failed".to_string();
        task.stage = "failed".to_string();
        task.message = "ACP runner install failed".to_string();
        task.detail = Some(detail);
        task.updated_at = now_secs();
    }
}

fn get_install_task_snapshot(agent_id: &str, task_id: &str) -> Option<AcpRunnerInstallTask> {
    let tasks = install_task_registry()
        .lock()
        .expect("acp install task registry poisoned");
    tasks
        .get(task_id)
        .filter(|task| task.agent_id == agent_id)
        .cloned()
}

fn acp_probe_cwd(state: &AppState, agent_id: &str) -> String {
    state
        .rua_root
        .join("workspaces")
        .join(agent_id)
        .to_string_lossy()
        .to_string()
}

async fn read_acp_config(state: &AppState, agent_id: &str) -> Result<AcpConfig, ApiError> {
    let cell = state.agent_cells.section::<AcpConfig>(agent_id, "acp");
    match cell.read().await {
        Ok(Some(mut cfg)) => {
            cfg.backfill_builtin_runners();
            Ok(cfg)
        }
        Ok(None) => {
            let mut cfg = AcpConfig::default();
            cfg.backfill_builtin_runners();
            Ok(cfg)
        }
        Err(e) => Err(ApiError::Internal(format!("acp cell read: {e}"))),
    }
}

async fn write_acp_config(
    state: &AppState,
    agent_id: &str,
    config: AcpConfig,
) -> Result<(), ApiError> {
    let cell = state.agent_cells.section::<AcpConfig>(agent_id, "acp");
    cell.write(config)
        .await
        .map_err(|e| ApiError::Internal(format!("acp cell write: {e}")))?;
    let path = state
        .rua_root
        .join("workspaces")
        .join(agent_id)
        .join("agent.json");
    crate::sync_publication::publish_agent_config_file_committed(
        state,
        agent_id,
        &path,
        "acp agent config",
    )
    .await;
    Ok(())
}

fn runner_response(key: &str, config: &AcpAgentConfig) -> serde_json::Value {
    let mut val = serde_json::to_value(config).unwrap_or_default();
    if let Some(obj) = val.as_object_mut() {
        obj.insert("key".to_string(), json!(key));
        obj.insert("is_builtin".to_string(), json!(is_builtin(key)));
    }
    val
}

async fn list_runners(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
) -> Response {
    list_runners_for(&state, Some(&ctx.agent_id)).await
}

async fn get_runner(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
    Path(runner_key): Path<String>,
) -> Response {
    get_runner_for(&state, Some(&ctx.agent_id), &runner_key).await
}

async fn check_runner(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
    Path(runner_key): Path<String>,
) -> Response {
    check_runner_for(&state, Some(&ctx.agent_id), &runner_key).await
}

async fn install_runner(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
    Path(runner_key): Path<String>,
) -> Response {
    install_runner_for(&state, Some(&ctx.agent_id), &runner_key).await
}

async fn start_install_runner(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
    Path(runner_key): Path<String>,
) -> Response {
    start_install_runner_for(state, Some(ctx.agent_id), runner_key).await
}

async fn get_install_task(
    Extension(ctx): Extension<AgentExtractorContext>,
    Path(task_id): Path<String>,
) -> Response {
    get_install_task_for(&ctx.agent_id, &task_id)
}

async fn create_runner(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
    Json(body): Json<AcpRunnerCreate>,
) -> Response {
    create_runner_for(&state, Some(&ctx.agent_id), body).await
}

async fn update_runner(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
    Path(runner_key): Path<String>,
    Json(body): Json<AcpRunnerUpdate>,
) -> Response {
    update_runner_for(&state, Some(&ctx.agent_id), &runner_key, body).await
}

async fn toggle_runner(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
    Path(runner_key): Path<String>,
    _body: axum::body::Bytes,
) -> Response {
    toggle_runner_for(&state, Some(&ctx.agent_id), &runner_key).await
}

async fn delete_runner(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
    Path(runner_key): Path<String>,
) -> Response {
    delete_runner_for(&state, Some(&ctx.agent_id), &runner_key).await
}

async fn list_runners_agent(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
) -> Response {
    list_runners_for(&state, Some(&agent_id)).await
}

async fn get_runner_agent(
    State(state): State<AppState>,
    Path((agent_id, runner_key)): Path<(String, String)>,
) -> Response {
    get_runner_for(&state, Some(&agent_id), &runner_key).await
}

async fn check_runner_agent(
    State(state): State<AppState>,
    Path((agent_id, runner_key)): Path<(String, String)>,
) -> Response {
    check_runner_for(&state, Some(&agent_id), &runner_key).await
}

async fn install_runner_agent(
    State(state): State<AppState>,
    Path((agent_id, runner_key)): Path<(String, String)>,
) -> Response {
    install_runner_for(&state, Some(&agent_id), &runner_key).await
}

async fn start_install_runner_agent(
    State(state): State<AppState>,
    Path((agent_id, runner_key)): Path<(String, String)>,
) -> Response {
    start_install_runner_for(state, Some(agent_id), runner_key).await
}

async fn get_install_task_agent(Path((agent_id, task_id)): Path<(String, String)>) -> Response {
    get_install_task_for(&agent_id, &task_id)
}

async fn create_runner_agent(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Json(body): Json<AcpRunnerCreate>,
) -> Response {
    create_runner_for(&state, Some(&agent_id), body).await
}

async fn update_runner_agent(
    State(state): State<AppState>,
    Path((agent_id, runner_key)): Path<(String, String)>,
    Json(body): Json<AcpRunnerUpdate>,
) -> Response {
    update_runner_for(&state, Some(&agent_id), &runner_key, body).await
}

async fn toggle_runner_agent(
    State(state): State<AppState>,
    Path((agent_id, runner_key)): Path<(String, String)>,
    _body: axum::body::Bytes,
) -> Response {
    toggle_runner_for(&state, Some(&agent_id), &runner_key).await
}

async fn delete_runner_agent(
    State(state): State<AppState>,
    Path((agent_id, runner_key)): Path<(String, String)>,
) -> Response {
    delete_runner_for(&state, Some(&agent_id), &runner_key).await
}

async fn list_runners_for(state: &AppState, agent_id: Option<&str>) -> Response {
    let aid = resolve_agent_id(agent_id);
    let config = match read_acp_config(state, aid).await {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };
    let mut runners = Vec::with_capacity(config.agents.len());
    for (key, runner) in &config.agents {
        runners.push(runner_response(key, runner));
    }
    (StatusCode::OK, Json(runners)).into_response()
}

async fn get_runner_for(state: &AppState, agent_id: Option<&str>, runner_key: &str) -> Response {
    let aid = resolve_agent_id(agent_id);
    let config = match read_acp_config(state, aid).await {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };
    match config.agents.get(runner_key) {
        Some(runner) => (StatusCode::OK, Json(runner_response(runner_key, runner))).into_response(),
        None => ApiError::NotFound(format!("ACP runner '{runner_key}' not found")).into_response(),
    }
}

async fn check_runner_for(state: &AppState, agent_id: Option<&str>, runner_key: &str) -> Response {
    let aid = resolve_agent_id(agent_id);
    let config = match read_acp_config(state, aid).await {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };

    let runner = match config.agents.get(runner_key) {
        Some(runner) => runner,
        None => {
            return ApiError::NotFound(format!("ACP runner '{runner_key}' not found"))
                .into_response();
        }
    };

    let cwd = acp_probe_cwd(state, aid);
    let result = check_runner_config(runner, &cwd, &state.rua_root).await;
    (StatusCode::OK, Json(result)).into_response()
}

async fn install_runner_for(
    state: &AppState,
    agent_id: Option<&str>,
    runner_key: &str,
) -> Response {
    let aid = resolve_agent_id(agent_id);
    match install_runner_core(state, aid, runner_key, &noop_install_progress()).await {
        Ok(result) => (StatusCode::OK, Json(result)).into_response(),
        Err(error) => error.into_response(),
    }
}

async fn start_install_runner_for(
    state: AppState,
    agent_id: Option<String>,
    runner_key: String,
) -> Response {
    let aid = agent_id.as_deref().unwrap_or("default").to_string();
    if let Err(error) = prepare_install_runner(&state, &aid, &runner_key).await {
        return error.into_response();
    }

    let task = create_install_task(&aid, &runner_key);
    let task_id = task.task_id.clone();
    let progress_task_id = task_id.clone();
    let progress = install_task_progress(progress_task_id);
    let worker_state = state.clone();
    let worker_agent_id = aid.clone();
    let worker_runner_key = runner_key.clone();
    tokio::spawn(async move {
        match install_runner_core(
            &worker_state,
            &worker_agent_id,
            &worker_runner_key,
            &progress,
        )
        .await
        {
            Ok(result) => complete_install_task(&task_id, result),
            Err(error) => fail_install_task(&task_id, error),
        }
    });

    (StatusCode::ACCEPTED, Json(task)).into_response()
}

fn get_install_task_for(agent_id: &str, task_id: &str) -> Response {
    match get_install_task_snapshot(agent_id, task_id) {
        Some(task) => (StatusCode::OK, Json(task)).into_response(),
        None => ApiError::NotFound(format!("ACP runner install task '{task_id}' not found"))
            .into_response(),
    }
}

type InstallProgress = std::sync::Arc<dyn Fn(&str, &str, Option<String>) + Send + Sync>;

fn noop_install_progress() -> InstallProgress {
    std::sync::Arc::new(|_, _, _| {})
}

fn install_task_progress(task_id: String) -> InstallProgress {
    std::sync::Arc::new(move |stage, message, detail| {
        update_install_task(&task_id, "running", stage, message, detail);
    })
}

fn emit_install_progress(
    progress: &InstallProgress,
    stage: &str,
    message: impl Into<String>,
    detail: Option<String>,
) {
    let message = message.into();
    progress(stage, &message, detail);
}

async fn prepare_install_runner(
    state: &AppState,
    aid: &str,
    runner_key: &str,
) -> Result<AcpAgentConfig, ApiError> {
    let config = match read_acp_config(state, aid).await {
        Ok(config) => config,
        Err(error) => return Err(error),
    };

    let runner = match config.agents.get(runner_key).cloned() {
        Some(runner) => runner,
        None => {
            return Err(ApiError::NotFound(format!(
                "ACP runner '{runner_key}' not found"
            )));
        }
    };

    if !is_builtin(runner_key) {
        return Err(ApiError::BadRequest(format!(
            "ACP runner install is only available for builtin runners; '{runner_key}' is custom"
        )));
    }

    if builtin_acp_runner_npm_package(runner_key).is_none() {
        return Err(ApiError::BadRequest(format!(
            "ACP runner '{runner_key}' has no install recipe"
        )));
    }

    Ok(runner)
}

async fn install_runner_core(
    state: &AppState,
    aid: &str,
    runner_key: &str,
    progress: &InstallProgress,
) -> Result<AcpRunnerInstallResult, ApiError> {
    emit_install_progress(
        progress,
        "validating",
        format!("Validating ACP runner '{runner_key}'"),
        None,
    );
    let runner = prepare_install_runner(state, aid, runner_key).await?;
    let package = builtin_acp_runner_npm_package(runner_key).expect("validated install recipe");

    let locale_signals = install_locale_signals(state);
    install_builtin_acp_runner_package(
        package,
        &state.rua_root,
        &state.cache_dir,
        &locale_signals,
        progress,
    )
    .await?;

    emit_install_progress(
        progress,
        "writing_config",
        format!("Writing ACP runner '{runner_key}' configuration"),
        None,
    );
    let mut latest_config = match read_acp_config(state, aid).await {
        Ok(config) => config,
        Err(error) => return Err(error),
    };
    let latest_runner = latest_config
        .agents
        .get(runner_key)
        .cloned()
        .unwrap_or(runner);
    let runner = canonicalize_installed_builtin_runner(runner_key, latest_runner);
    let runner_changed = match latest_config.agents.get(runner_key) {
        Some(current) => {
            runner.command != current.command
                || runner.args != current.args
                || runner.env != current.env
        }
        None => true,
    };
    if runner_changed {
        latest_config
            .agents
            .insert(runner_key.to_string(), runner.clone());
        write_acp_config(state, aid, latest_config).await?;
    }

    emit_install_progress(
        progress,
        "checking_runner",
        format!("Checking ACP runner '{runner_key}' availability"),
        None,
    );
    let cwd = acp_probe_cwd(state, aid);
    let check_result = check_runner_config(&runner, &cwd, &state.cache_dir).await;

    let message = if check_result.ok {
        format!("ACP runner '{runner_key}' installed and available")
    } else if check_result.requires_auth {
        format!("ACP runner '{runner_key}' installed; authentication is required")
    } else {
        format!(
            "ACP runner '{runner_key}' installed, but availability check failed: {}",
            check_result.message
        )
    };

    Ok(AcpRunnerInstallResult {
        installed: true,
        message,
        check_result,
    })
}

fn canonicalize_installed_builtin_runner(
    runner_key: &str,
    mut runner: AcpAgentConfig,
) -> AcpAgentConfig {
    if let Some(canonical) = default_builtin_runners().remove(runner_key) {
        runner.command = canonical.command;
        runner.args = canonical.args;
        runner.tool_parse_mode = canonical.tool_parse_mode;
        runner.stdio_buffer_limit_bytes = canonical.stdio_buffer_limit_bytes;
    }
    runner
}

async fn create_runner_for(
    state: &AppState,
    agent_id: Option<&str>,
    body: AcpRunnerCreate,
) -> Response {
    let aid = resolve_agent_id(agent_id);
    if body.key.is_empty() {
        return ApiError::BadRequest("key is required".into()).into_response();
    }

    let mut config = match read_acp_config(state, aid).await {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };

    if config.agents.contains_key(&body.key) {
        return ApiError::Conflict(format!("ACP runner '{}' already exists", body.key))
            .into_response();
    }

    config.agents.insert(body.key.clone(), body.config);
    if let Err(e) = write_acp_config(state, aid, config.clone()).await {
        return e.into_response();
    }

    let runner = config.agents.get(&body.key).unwrap();
    (
        StatusCode::CREATED,
        Json(runner_response(&body.key, runner)),
    )
        .into_response()
}

async fn update_runner_for(
    state: &AppState,
    agent_id: Option<&str>,
    runner_key: &str,
    body: AcpRunnerUpdate,
) -> Response {
    let aid = resolve_agent_id(agent_id);
    let mut config = match read_acp_config(state, aid).await {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };

    let mut runner = match config.agents.get(runner_key).cloned() {
        Some(runner) => runner,
        None => {
            return ApiError::NotFound(format!("ACP runner '{runner_key}' not found"))
                .into_response();
        }
    };
    body.apply_to(&mut runner);

    config.agents.insert(runner_key.to_string(), runner);
    if let Err(e) = write_acp_config(state, aid, config.clone()).await {
        return e.into_response();
    }

    let runner = config.agents.get(runner_key).unwrap();
    (StatusCode::OK, Json(runner_response(runner_key, runner))).into_response()
}

async fn toggle_runner_for(state: &AppState, agent_id: Option<&str>, runner_key: &str) -> Response {
    let aid = resolve_agent_id(agent_id);
    let mut config = match read_acp_config(state, aid).await {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };

    match config.agents.get_mut(runner_key) {
        Some(runner) => {
            runner.enabled = !runner.enabled;
        }
        None => {
            return ApiError::NotFound(format!("ACP runner '{runner_key}' not found"))
                .into_response();
        }
    }

    if let Err(e) = write_acp_config(state, aid, config.clone()).await {
        return e.into_response();
    }

    let runner = config.agents.get(runner_key).unwrap();
    (StatusCode::OK, Json(runner_response(runner_key, runner))).into_response()
}

async fn delete_runner_for(state: &AppState, agent_id: Option<&str>, runner_key: &str) -> Response {
    let aid = resolve_agent_id(agent_id);

    if is_builtin(runner_key) {
        return ApiError::Forbidden(format!(
            "Cannot delete builtin ACP runner '{runner_key}'. Use toggle to disable it instead."
        ))
        .into_response();
    }

    let mut config = match read_acp_config(state, aid).await {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };

    if config.agents.remove(runner_key).is_none() {
        return ApiError::NotFound(format!("ACP runner '{runner_key}' not found")).into_response();
    }

    if let Err(e) = write_acp_config(state, aid, config).await {
        return e.into_response();
    }

    (
        StatusCode::OK,
        Json(json!({
            "message": format!("ACP runner '{}' deleted successfully", runner_key)
        })),
    )
        .into_response()
}

pub fn builtin_acp_runner_npm_package(runner_key: &str) -> Option<&'static str> {
    match runner_key {
        "opencode" => Some("opencode-ai@1.18.3"),
        "qwen" => Some("@qwen-code/qwen-code@0.19.11"),
        "claude" => Some("@agentclientprotocol/claude-agent-acp@0.59.0"),
        "codex" => Some("@agentclientprotocol/codex-acp@1.1.4"),
        _ => None,
    }
}

fn install_locale_signals(state: &AppState) -> Vec<String> {
    let path = state.rua_root.join("settings.json");
    let Ok(bytes) = std::fs::read(path) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return Vec::new();
    };
    value
        .get("language")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .map(|value| vec![value.to_string()])
        .unwrap_or_default()
}

async fn install_builtin_acp_runner_package(
    package: &str,
    working_dir: &FsPath,
    cache_dir: &FsPath,
    locale_signals: &[String],
    progress: &InstallProgress,
) -> Result<(), ApiError> {
    emit_install_progress(
        progress,
        "resolving_mirror",
        "Resolving package install mirror",
        None,
    );
    let mirror = detect_install_mirror_config_with_locale_values(locale_signals).await;
    let authority = RuntimeAuthority::from_current_process(None, working_dir, cache_dir);
    emit_install_progress(
        progress,
        "installing_package",
        format!("Installing {package} with npm"),
        mirror.npm_registry.clone(),
    );
    let output = run_runtime_authority_command(
        authority.plan(
            ProcessShape::Install,
            ToolchainIntent::NodePackageManager,
            RuntimeLaunchRequest {
                args: vec![
                    "install".to_string(),
                    "-g".to_string(),
                    package.to_string(),
                    "--no-audit".to_string(),
                    "--no-fund".to_string(),
                ],
                env: linux_compat_install_env(&mirror),
                ..Default::default()
            },
        ),
        ACP_RUNNER_INSTALL_TIMEOUT,
        "npm install",
    )
    .await?;

    if !output.status.success() {
        return Err(ApiError::BadRequest(format!(
            "npm install -g {package} failed (exit {}): {}",
            output.status.code().unwrap_or(-1),
            process_output_detail(&output)
        )));
    }

    Ok(())
}

async fn run_runtime_authority_command(
    plan: RuntimeLaunchPlan,
    timeout: Duration,
    label: &str,
) -> Result<std::process::Output, ApiError> {
    let isolated = plan.runtime_kind == rua_core::runtime_layout::RuntimeKind::LinuxCompat;
    let process = plan
        .into_process()
        .map_err(|error| ApiError::Unavailable(error.to_string()))?;
    let command_display = process.command.to_string_lossy().to_string();
    let mut cmd = Command::new(process.command);
    cmd.args(process.args);
    if isolated {
        cmd.env_clear();
    }
    cmd.envs(process.env);
    cmd.kill_on_drop(true);
    #[cfg(unix)]
    unsafe {
        cmd.pre_exec(|| {
            if libc::setpgid(0, 0) == 0 {
                Ok(())
            } else {
                Err(std::io::Error::last_os_error())
            }
        });
    }
    if let Some(cwd) = process.cwd {
        cmd.current_dir(cwd);
    }

    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let child = cmd.spawn().map_err(|error| {
        ApiError::Unavailable(format!("failed to start ACP runner {label}: {error}"))
    })?;
    let mut group = InstallProcessGroup(child.id());
    let result = tokio::time::timeout(timeout, child.wait_with_output())
        .await
        .map_err(|_| {
            ApiError::Unavailable(format!(
                "ACP runner {label} timed out after {}s: {command_display}",
                timeout.as_secs()
            ))
        })?
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                ApiError::BadRequest(format!(
                    "ACP runner {label} command is not available in Runtime Authority launch plan: {command_display}"
                ))
            } else {
                ApiError::Internal(format!("failed to run ACP runner {label}: {error}"))
            }
        });
    if result.is_ok() {
        group.0 = None;
    }
    result
}

struct InstallProcessGroup(Option<u32>);

impl Drop for InstallProcessGroup {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(pid) = self.0 {
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
        }
    }
}

fn linux_compat_install_env(mirror: &InstallMirrorConfig) -> BTreeMap<String, String> {
    let mut env = BTreeMap::from([
        ("npm_config_audit".to_string(), "false".to_string()),
        ("npm_config_fund".to_string(), "false".to_string()),
        (
            "npm_config_update_notifier".to_string(),
            "false".to_string(),
        ),
    ]);
    if let Some(npm_registry) = mirror.npm_registry.as_deref() {
        env.insert("npm_config_registry".to_string(), npm_registry.to_string());
        env.insert("NPM_CONFIG_REGISTRY".to_string(), npm_registry.to_string());
    }
    if let Some(node_dist_base) = mirror.node_dist_base_url.as_deref() {
        env.insert("npm_config_disturl".to_string(), node_dist_base.to_string());
        env.insert("NPM_CONFIG_DISTURL".to_string(), node_dist_base.to_string());
    }
    if let Some(pypi_index) = mirror.pypi_index_url.as_deref() {
        env.insert("PIP_INDEX_URL".to_string(), pypi_index.to_string());
        env.insert("UV_DEFAULT_INDEX".to_string(), pypi_index.to_string());
        env.insert("UV_INDEX_URL".to_string(), pypi_index.to_string());
    }
    env
}

fn process_output_detail(output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let detail = if !stderr.is_empty() { stderr } else { stdout };
    truncate_for_response(&detail, 4000)
}

fn truncate_for_response(value: &str, max_chars: usize) -> String {
    let mut iter = value.chars();
    let truncated: String = iter.by_ref().take(max_chars).collect();
    if iter.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}

async fn check_runner_config(
    runner: &AcpAgentConfig,
    cwd: &str,
    runtime_cache_dir: &FsPath,
) -> AcpRunnerCheckResult {
    probe_runner_config(runner, cwd, runtime_cache_dir).await
}

async fn probe_runner_config(
    runner: &AcpAgentConfig,
    cwd: &str,
    runtime_cache_dir: &FsPath,
) -> AcpRunnerCheckResult {
    if runner.command.trim().is_empty() {
        return AcpRunnerCheckResult::failure("ACP runner command is required".to_string());
    }

    let mut check_runner = runner.clone();
    check_runner.enabled = true;

    match AcpClientAdapter::probe_initialize_for_runtime_with_timeout(
        &check_runner,
        FsPath::new(cwd),
        runtime_cache_dir,
        ACP_RUNNER_CHECK_TIMEOUT,
    )
    .await
    {
        Ok(()) => AcpRunnerCheckResult {
            ok: true,
            status: "ok".to_string(),
            message: "ACP runner is available".to_string(),
            requires_auth: false,
        },
        Err(err) => AcpRunnerCheckResult::from_error(err.to_string()),
    }
}

impl AcpRunnerCheckResult {
    fn failure(message: String) -> Self {
        let requires_auth = is_auth_error(&message);
        let message = if requires_auth {
            format!("ACP runner is not authenticated. Authenticate it first: {message}")
        } else if message.starts_with("ACP ") {
            message
        } else {
            format!("ACP runner check failed: {message}")
        };
        Self {
            ok: false,
            status: "unavailable".to_string(),
            message,
            requires_auth,
        }
    }

    fn from_error(message: String) -> Self {
        Self::failure(message)
    }
}

fn is_auth_error(message: &str) -> bool {
    let normalized = message.to_ascii_lowercase();
    normalized.contains("auth")
        || normalized.contains("authenticate")
        || normalized.contains("login")
        || normalized.contains("api key")
        || normalized.contains("api_key")
        || normalized.contains("credential")
}
