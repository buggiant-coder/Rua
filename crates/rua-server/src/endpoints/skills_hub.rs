use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use axum::extract::{Path as AxumPath, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tokio::sync::OnceCell;

use rua_skills::pool_service::SkillPoolService;
use rua_skills::{SkillBundleWrite, SkillService};
use rua_skills_hub::bundle::{BundleInstaller, BundlePoolInstaller};
use rua_skills_hub::client::HubClient;
use rua_skills_hub::types::{HubError, HubInstallResult, HubSkillResult};
use rua_skills_hub::{
    install_skill_from_hub, install_skill_to_pool_from_hub, resolve_bundle_source,
    search_hub_skills,
};

use crate::endpoints::dto;
use crate::services::hub_install_task::DEFAULT_INSTALL_TIMEOUT;
use crate::state::AppState;
use crate::sync_publication::{publish_skill_pool_committed, publish_workspace_skill_committed};

pub fn register_hub_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/agents/{agentId}/skills/hub/install",
            post(hub_install_handler),
        )
        .route("/skills/hub/install_pool", post(hub_install_pool_handler))
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct HubSearchQuery {
    #[serde(default)]
    pub q: Option<String>,
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HubInstallBody {
    pub url: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub enable: Option<bool>,
    #[serde(default)]
    pub target_name: Option<String>,
    #[serde(default)]
    pub cancel_id: Option<String>,
    #[serde(default)]
    pub installed_from: Option<String>,
    #[serde(default)]
    pub market_meta: Option<Value>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct HubSearchHit {
    pub slug: String,
    pub name: String,
    pub description: String,
    pub version: String,
    pub source_url: String,
}

impl From<HubSkillResult> for HubSearchHit {
    fn from(value: HubSkillResult) -> Self {
        Self {
            slug: value.slug,
            name: value.name,
            description: value.description,
            version: value.version,
            source_url: value.source_url,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct HubInstallResponse {
    pub installed: bool,
    pub name: String,
    pub enabled: bool,
    pub source_url: String,
}

impl From<HubInstallResult> for HubInstallResponse {
    fn from(value: HubInstallResult) -> Self {
        Self {
            installed: true,
            name: value.name,
            enabled: value.enabled,
            source_url: value.source_url,
        }
    }
}

pub async fn hub_search_handler(Query(query): Query<HubSearchQuery>) -> Response {
    let q = query.q.unwrap_or_default();
    let limit = query.limit.unwrap_or(20);
    let client = match shared_client().await {
        Ok(c) => c,
        Err(e) => return hub_error_to_response(e),
    };
    match search_hub_skills(client.as_ref(), &q, limit).await {
        Ok(results) => {
            let hits: Vec<HubSearchHit> = results.into_iter().map(HubSearchHit::from).collect();
            (StatusCode::OK, Json(hits)).into_response()
        }
        Err(e) => hub_error_to_response(e),
    }
}

pub async fn hub_install_handler(
    State(state): State<AppState>,
    AxumPath(agent_id): AxumPath<String>,
    Json(body): Json<HubInstallBody>,
) -> Response {
    let workspace_dir = state.skill_service.workspace_dir_for(Some(&agent_id));
    let installer = match WorkspaceBundleInstaller::new(&workspace_dir) {
        Ok(i) => i,
        Err(e) => return hub_error_to_response(e),
    };
    let client = match shared_client().await {
        Ok(c) => c,
        Err(e) => return hub_error_to_response(e),
    };
    let version = body.version.clone().unwrap_or_default();
    let enable = body.enable.unwrap_or(false);
    let target_name = body.target_name.as_deref();
    let meta = build_install_meta_from_body(
        &body.url,
        &version,
        body.installed_from.as_deref(),
        body.market_meta.clone(),
    );
    match install_skill_from_hub(
        client.as_ref(),
        &installer,
        &body.url,
        &version,
        enable,
        target_name,
        None,
        Some(meta),
    )
    .await
    {
        Ok(result) => {
            publish_workspace_skill_committed(&state, &agent_id, &result.name).await;
            (StatusCode::OK, Json(HubInstallResponse::from(result))).into_response()
        }
        Err(e) => hub_error_to_response(e),
    }
}

pub async fn hub_install_pool_handler(
    State(state): State<AppState>,
    Json(body): Json<HubInstallBody>,
) -> Response {
    let pool_dir = state.rua_root.join("skill_pool");
    let installer = PoolBundleInstaller::new(pool_dir);
    let client = match shared_client().await {
        Ok(c) => c,
        Err(e) => return hub_error_to_response(e),
    };
    let version = body.version.clone().unwrap_or_default();
    let target_name = body.target_name.as_deref();
    let meta = build_install_meta_from_body(
        &body.url,
        &version,
        body.installed_from.as_deref(),
        body.market_meta.clone(),
    );
    match install_skill_to_pool_from_hub(
        client.as_ref(),
        &installer,
        &body.url,
        &version,
        target_name,
        None,
        Some(meta),
    )
    .await
    {
        Ok(result) => {
            publish_skill_pool_committed(&state, &result.name).await;
            (StatusCode::OK, Json(HubInstallResponse::from(result))).into_response()
        }
        Err(e) => hub_error_to_response(e),
    }
}

pub(crate) fn spawn_hub_install_sync_publication_watcher(
    state: AppState,
    agent_id: String,
    task_id: String,
) {
    tokio::spawn(async move {
        let deadline = Instant::now() + DEFAULT_INSTALL_TIMEOUT + Duration::from_secs(5);
        loop {
            tokio::time::sleep(Duration::from_millis(250)).await;
            let status = match state
                .skill_service
                .hub_install_status(&agent_id, &task_id)
                .await
            {
                Ok(status) => status,
                Err(e) => {
                    tracing::warn!(
                        task_id,
                        error = %e,
                        "[sync-v4] hub install sync watcher could not read status"
                    );
                    return;
                }
            };
            match status.status.as_str() {
                "completed" => {
                    publish_completed_hub_install_once(&state, &agent_id, &task_id, &status).await;
                    return;
                }
                "failed" | "cancelled" => return,
                _ if Instant::now() >= deadline => {
                    tracing::warn!(
                        task_id,
                        "[sync-v4] hub install sync watcher timed out before terminal status"
                    );
                    return;
                }
                _ => {}
            }
        }
    });
}

pub(crate) async fn publish_completed_hub_install_once(
    state: &AppState,
    agent_id: &str,
    task_id: &str,
    status: &dto::SkillHubInstallStatus,
) {
    if status.status != "completed" {
        return;
    }
    let Some(skill_name) = status
        .extra
        .get("result")
        .and_then(|result| result.get("name"))
        .and_then(|name| name.as_str())
    else {
        return;
    };
    if state
        .skill_service
        .mark_hub_install_sync_published(agent_id, task_id)
    {
        publish_workspace_skill_committed(state, agent_id, skill_name).await;

        if let Some(runtime) = state.registry.get(agent_id).await {
            if let Some(rt) = runtime.react_runtime().await {
                rt.bump_prepared_generation();
            }
        }
    }
}

const STATUS_CLIENT_CLOSED_REQUEST: u16 = 499;

pub fn hub_error_to_response(err: HubError) -> Response {
    let (status, message) = match err {
        HubError::Cancelled => (STATUS_CLIENT_CLOSED_REQUEST, "cancelled".to_string()),
        HubError::RateLimit => (
            StatusCode::TOO_MANY_REQUESTS.as_u16(),
            "hub rate limit exceeded".to_string(),
        ),
        HubError::ZipLimit => (
            StatusCode::PAYLOAD_TOO_LARGE.as_u16(),
            "bundle exceeded the size or entry-count limits".to_string(),
        ),
        HubError::UnsupportedSource(src) => (
            StatusCode::BAD_REQUEST.as_u16(),
            format!("source not supported: {src}"),
        ),
        HubError::Http { status, body } => (status, body),
        HubError::InvalidBundle(msg) => (
            StatusCode::UNPROCESSABLE_ENTITY.as_u16(),
            format!("invalid bundle: {msg}"),
        ),
        HubError::Transport(msg) => (
            StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
            format!("transport error: {msg}"),
        ),
        HubError::Installer(msg) => (
            StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
            format!("installer error: {msg}"),
        ),
    };

    let status = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let body = serde_json::json!({ "detail": message });
    (status, Json(body)).into_response()
}

fn client_cell() -> &'static OnceCell<Arc<HubClient>> {
    static CELL: OnceCell<Arc<HubClient>> = OnceCell::const_new();
    &CELL
}

pub(crate) async fn shared_client() -> Result<Arc<HubClient>, HubError> {
    client_cell()
        .get_or_try_init(|| async {
            HubClient::new()
                .map(Arc::new)
                .map_err(|e| HubError::Transport(e.to_string()))
        })
        .await
        .cloned()
}

pub struct WorkspaceBundleInstaller {
    service: SkillService,
}

impl WorkspaceBundleInstaller {
    pub fn new(workspace_dir: impl Into<PathBuf>) -> Result<Self, HubError> {
        let workspace_dir = workspace_dir.into();
        let service =
            SkillService::new(&workspace_dir).map_err(|e| HubError::Installer(e.to_string()))?;
        Ok(Self { service })
    }
}

#[async_trait]
impl BundleInstaller for WorkspaceBundleInstaller {
    async fn create_skill(
        &self,
        name: &str,
        content: &str,
        references: &Map<String, Value>,
        scripts: &Map<String, Value>,
        extra_files: &Map<String, Value>,
        install_meta: Option<&rua_skills::InstallMeta>,
    ) -> Result<String, HubError> {
        let created = self
            .service
            .create_skill_bundle(
                SkillBundleWrite {
                    name,
                    content,
                    references,
                    scripts,
                    extra_files,
                    install_meta,
                },
                false,
            )
            .map_err(|e| HubError::Installer(e.to_string()))?;
        let final_name = created.ok_or_else(|| {
            HubError::Installer(format!("skill '{name}' already exists in workspace"))
        })?;
        Ok(final_name)
    }

    async fn enable_skill(&self, name: &str) -> Result<bool, HubError> {
        self.service
            .enable_skill(name)
            .map_err(|e| HubError::Installer(e.to_string()))
    }
}

pub struct PoolBundleInstaller {
    pool_dir: PathBuf,
}

impl PoolBundleInstaller {
    pub fn new(pool_dir: impl Into<PathBuf>) -> Self {
        Self {
            pool_dir: pool_dir.into(),
        }
    }
}

#[async_trait]
impl BundlePoolInstaller for PoolBundleInstaller {
    async fn create_pool_skill(
        &self,
        name: &str,
        content: &str,
        references: &Map<String, Value>,
        scripts: &Map<String, Value>,
        extra_files: &Map<String, Value>,
        install_meta: Option<&rua_skills::InstallMeta>,
    ) -> Result<String, HubError> {
        let service = SkillPoolService::new_in(&self.pool_dir)
            .map_err(|e| HubError::Installer(e.to_string()))?;
        service
            .create_skill_bundle(SkillBundleWrite {
                name,
                content,
                references,
                scripts,
                extra_files,
                install_meta,
            })
            .map_err(|e| HubError::Installer(e.to_string()))?
            .ok_or_else(|| HubError::Installer(format!("pool skill '{name}' already exists")))
    }
}

pub(crate) fn build_install_meta_from_body(
    url: &str,
    version: &str,
    installed_from: Option<&str>,
    market_meta: Option<serde_json::Value>,
) -> rua_skills::InstallMeta {
    let provider = installed_from
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            resolve_bundle_source(url)
                .map(|s| s.provider_key().to_string())
                .unwrap_or_else(|_| "url".to_string())
        });
    rua_skills::InstallMeta {
        installed_from: provider,
        source_url: url.to_string(),
        installed_at: chrono::Utc::now().to_rfc3339(),
        updated_at: None,
        installed_version: version.to_string(),
        content_hash: String::new(),
        market_meta,
    }
}
