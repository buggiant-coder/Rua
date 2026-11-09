use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use futures::StreamExt;
use serde::Deserialize;
use std::convert::Infallible;

use std::sync::Arc;

use rua_workspace::WorkspaceRegistry;

use crate::endpoints::dto;
use crate::errors::ApiError;
use crate::state::AppState;
use crate::sync_publication::{
    publish_skill_pool_committed, publish_skill_pool_deleted, publish_workspace_skill_committed,
    publish_workspace_skill_deleted,
};

use super::skills_hub::{
    publish_completed_hub_install_once, spawn_hub_install_sync_publication_watcher,
};

fn skill_error_to_api(e: rua_skills::SkillError) -> ApiError {
    match e {
        rua_skills::SkillError::InvalidSkillName(msg) => ApiError::BadRequest(msg),
        rua_skills::SkillError::Conflict(msg) => ApiError::BadRequest(msg),
        other => ApiError::Internal(other.to_string()),
    }
}

#[derive(Debug, Default, Deserialize)]
struct SkillUploadQuery {
    #[serde(default)]
    enable: Option<bool>,
    #[serde(default)]
    target_name: Option<String>,
    #[serde(default)]
    rename_map: Option<String>,
    #[serde(default)]
    overwrite: bool,
}

impl SkillUploadQuery {
    fn into_options(self, default_enable: bool) -> Result<rua_skills::ImportOptions, ApiError> {
        let rename_map = match self.rename_map {
            Some(raw) if !raw.trim().is_empty() => serde_json::from_str(&raw).map_err(|error| {
                ApiError::BadRequest(format!("rename_map must be a JSON string map: {error}"))
            })?,
            _ => std::collections::BTreeMap::new(),
        };
        Ok(rua_skills::ImportOptions {
            enable: self.enable.unwrap_or(default_enable),
            target_name: self.target_name,
            rename_map,
            overwrite: self.overwrite,
        })
    }
}

fn schedule_skill_refresh(registry: Arc<dyn WorkspaceRegistry>, agent_id: &str) {
    let id = agent_id.to_string();
    tokio::spawn(async move {
        if let Some(runtime) = registry.get(&id).await {
            if let Some(rt) = runtime.react_runtime().await {
                rt.bump_prepared_generation();
            }
        }
    });
}

pub fn shared_router() -> Router<AppState> {
    Router::new()
        .route("/skills/workspaces", get(list_skill_workspaces))
        .route("/skills/hub/search", get(hub_search))
        .route("/skills/pool", get(list_pool))
        .route("/skills/pool/refresh", post(refresh_pool))
        .route("/skills/pool/builtin-sources", get(pool_builtin_sources))
        .route("/skills/pool/builtin-notice", get(pool_builtin_notice))
        .route("/skills/pool/create", post(pool_create))
        .route("/skills/pool/save", put(pool_save))
        .route("/skills/pool/upload-zip", post(pool_upload_zip))
        .route("/skills/pool/import", post(pool_import))
        .route("/skills/pool/upload", post(pool_upload))
        .route("/skills/pool/download", post(pool_download))
        .route("/skills/pool/import-builtin", post(pool_import_builtin))
        .route("/skills/pool/batch-delete", post(pool_batch_delete))
        .route("/skills/pool/{skill_name}/tags", put(pool_update_tags))
        .route(
            "/skills/pool/{skill_name}/update-builtin",
            post(pool_update_builtin),
        )
        .route("/skills/pool/{skill_name}", delete(pool_delete_skill))
        .route(
            "/skills/pool/{skill_name}/config",
            get(pool_get_config)
                .put(pool_update_config)
                .delete(pool_delete_config),
        )
        .route("/skills/preview-hub-skill", post(preview_hub_skill))
        .route("/skills/ai/optimize/stream", post(ai_optimize_stream))
}

pub fn agent_scoped_router() -> Router<AppState> {
    Router::new()
        .route(
            "/agents/{agentId}/skills",
            get(agent_list_skills).post(agent_create_skill),
        )
        .route(
            "/agents/{agentId}/skills/refresh",
            post(agent_refresh_skills),
        )
        .route(
            "/agents/{agentId}/skills/hub/install/start",
            post(agent_hub_install_start),
        )
        .route(
            "/agents/{agentId}/skills/hub/install/status/{task_id}",
            get(agent_hub_install_status),
        )
        .route(
            "/agents/{agentId}/skills/hub/install/cancel/{task_id}",
            post(agent_hub_install_cancel),
        )
        .route("/agents/{agentId}/skills/upload", post(agent_upload_skill))
        .route(
            "/agents/{agentId}/skills/batch-delete",
            post(agent_batch_delete_skills),
        )
        .route(
            "/agents/{agentId}/skills/batch-disable",
            post(agent_batch_disable_skills),
        )
        .route(
            "/agents/{agentId}/skills/batch-enable",
            post(agent_batch_enable_skills),
        )
        .route("/agents/{agentId}/skills/save", put(agent_save_skill))
        .route(
            "/agents/{agentId}/skills/{skill_name}/enable",
            post(agent_enable_skill),
        )
        .route(
            "/agents/{agentId}/skills/{skill_name}/disable",
            post(agent_disable_skill),
        )
        .route(
            "/agents/{agentId}/skills/{skill_name}",
            get(agent_get_skill).delete(agent_delete_skill),
        )
        .route(
            "/agents/{agentId}/skills/{skill_name}/files/{*file_path}",
            get(agent_get_skill_file),
        )
        .route(
            "/agents/{agentId}/skills/{skill_name}/channels",
            put(agent_update_skill_channels),
        )
        .route(
            "/agents/{agentId}/skills/{skill_name}/config",
            get(agent_get_skill_config)
                .put(agent_update_skill_config)
                .delete(agent_delete_skill_config),
        )
}

async fn list_skill_workspaces(State(state): State<AppState>) -> Response {
    match state.skill_service.list_workspaces().await {
        Ok(resp) => (StatusCode::OK, Json(resp)).into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn hub_search(
    State(state): State<AppState>,
    Query(query): Query<dto::SkillHubSearchQuery>,
) -> Response {
    match state.skill_service.hub_search(query).await {
        Ok(results) => (StatusCode::OK, Json(results)).into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn list_pool(State(state): State<AppState>) -> Response {
    match state.skill_service.list_pool().await {
        Ok(pool) => (StatusCode::OK, Json(pool)).into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn refresh_pool(State(state): State<AppState>) -> Response {
    match state.skill_service.refresh_pool().await {
        Ok(resp) => (StatusCode::OK, Json(resp)).into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn pool_builtin_sources(State(state): State<AppState>) -> Response {
    let pool_dir = state.rua_root.join("skill_pool");
    match rua_skills::list_builtin_import_candidates_in(&pool_dir) {
        Ok(candidates) => (StatusCode::OK, Json(candidates)).into_response(),
        Err(e) => skill_error_to_api(e).into_response(),
    }
}

async fn pool_builtin_notice(State(state): State<AppState>) -> Response {
    match state.skill_service.pool_builtin_notice().await {
        Ok(notice) => (StatusCode::OK, Json(notice)).into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn pool_update_tags(
    State(state): State<AppState>,
    Path(skill_name): Path<String>,
    Json(body): Json<dto::SkillPoolTagsRequest>,
) -> Response {
    match state
        .skill_service
        .pool_update_tags(&skill_name, body)
        .await
    {
        Ok(()) => {
            publish_skill_pool_committed(&state, &skill_name).await;
            (StatusCode::OK, Json(dto::SuccessResponse::ok())).into_response()
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn pool_create(
    State(state): State<AppState>,
    Json(body): Json<dto::SkillPoolCreateRequest>,
) -> Response {
    match state.skill_service.pool_create(body).await {
        Ok(entry) => {
            publish_skill_pool_committed(&state, &entry.id).await;
            (StatusCode::CREATED, Json(entry)).into_response()
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn pool_save(
    State(state): State<AppState>,
    Json(body): Json<dto::SkillPoolSaveRequest>,
) -> Response {
    match state.skill_service.pool_save(body).await {
        Ok(entry) => {
            publish_skill_pool_committed(&state, &entry.id).await;
            (StatusCode::OK, Json(entry)).into_response()
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn pool_upload_zip(
    State(state): State<AppState>,
    Query(query): Query<SkillUploadQuery>,
    body: bytes::Bytes,
) -> Response {
    let options = match query.into_options(false) {
        Ok(options) => options,
        Err(error) => return error.into_response(),
    };
    match state.skill_service.pool_upload_zip(body, options).await {
        Ok(resp) => {
            for name in &resp.imported {
                publish_skill_pool_committed(&state, name).await;
            }
            (StatusCode::OK, Json(resp)).into_response()
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn pool_import(
    State(state): State<AppState>,
    Json(body): Json<dto::SkillPoolImportRequest>,
) -> Response {
    match state.skill_service.pool_import(body).await {
        Ok(resp) => (StatusCode::OK, Json(resp)).into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn pool_upload(State(state): State<AppState>, body: bytes::Bytes) -> Response {
    match state.skill_service.pool_upload(body).await {
        Ok(resp) => {
            if let Some(name) = resp.extra.get("name").and_then(|v| v.as_str()) {
                publish_skill_pool_committed(&state, name).await;
            }
            (StatusCode::OK, Json(resp)).into_response()
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn pool_download(
    State(state): State<AppState>,
    Json(body): Json<dto::SkillPoolDownloadRequest>,
) -> Response {
    match state.skill_service.pool_download(body).await {
        Ok(data) => (StatusCode::OK, data).into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn pool_import_builtin(
    State(state): State<AppState>,
    Json(body): Json<dto::SkillPoolImportBuiltinRequest>,
) -> Response {
    let pool_dir = state.rua_root.join("skill_pool");
    let names = if body.skill_names.is_empty() {
        None
    } else {
        Some(body.skill_names.as_slice())
    };
    match rua_skills::import_builtin_skills_in(&pool_dir, names, body.overwrite_conflicts) {
        Ok(report) => {
            if !report.conflicts.is_empty() && !body.overwrite_conflicts {
                return (StatusCode::CONFLICT, Json(&report)).into_response();
            }
            for name in report.imported.iter().chain(report.updated.iter()) {
                publish_skill_pool_committed(&state, name).await;
            }
            (StatusCode::OK, Json(report)).into_response()
        }
        Err(e) => skill_error_to_api(e).into_response(),
    }
}

async fn pool_update_builtin(
    State(state): State<AppState>,
    Path(skill_name): Path<String>,
) -> Response {
    let pool_dir = state.rua_root.join("skill_pool");
    match rua_skills::update_single_builtin_in(&pool_dir, &skill_name) {
        Ok(candidate) => {
            publish_skill_pool_committed(&state, &skill_name).await;
            (StatusCode::OK, Json(candidate)).into_response()
        }
        Err(e) => skill_error_to_api(e).into_response(),
    }
}

async fn pool_delete_skill(
    State(state): State<AppState>,
    Path(skill_name): Path<String>,
) -> Response {
    match state.skill_service.pool_delete(&skill_name).await {
        Ok(()) => {
            publish_skill_pool_deleted(&state, &skill_name).await;
            (StatusCode::OK, Json(dto::SuccessResponse::ok())).into_response()
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn pool_get_config(
    State(state): State<AppState>,
    Path(skill_name): Path<String>,
) -> Response {
    match state.skill_service.pool_get_config(&skill_name).await {
        Ok(config) => (StatusCode::OK, Json(config)).into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn pool_update_config(
    State(state): State<AppState>,
    Path(skill_name): Path<String>,
    Json(body): Json<dto::SkillPoolConfig>,
) -> Response {
    let config_value = body
        .inner
        .get("config")
        .cloned()
        .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
    let _ = config_value;
    match state
        .skill_service
        .pool_update_config(&skill_name, body)
        .await
    {
        Ok(config) => {
            publish_skill_pool_committed(&state, &skill_name).await;
            (StatusCode::OK, Json(config)).into_response()
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn pool_delete_config(
    State(state): State<AppState>,
    Path(skill_name): Path<String>,
) -> Response {
    match state.skill_service.pool_delete_config(&skill_name).await {
        Ok(()) => {
            publish_skill_pool_committed(&state, &skill_name).await;
            (StatusCode::OK, Json(dto::SuccessResponse::ok())).into_response()
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn pool_batch_delete(
    State(state): State<AppState>,
    Json(body): Json<dto::BatchDeleteRequest>,
) -> Response {
    let names = body.names.clone();
    match state.skill_service.pool_batch_delete(body).await {
        Ok(()) => {
            for name in &names {
                publish_skill_pool_deleted(&state, name).await;
            }
            (StatusCode::OK, Json(dto::SuccessResponse::ok())).into_response()
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn ai_optimize_stream(
    State(state): State<AppState>,
    Json(body): Json<dto::SkillOptimizeRequest>,
) -> Response {
    let event_stream = state.skill_service.ai_optimize_stream(body);

    let sse_stream = event_stream.map(|result| -> std::result::Result<Event, Infallible> {
        match result {
            Ok(event) => {
                let data = serde_json::to_string(&event).unwrap_or_default();
                Ok(Event::default().data(data))
            }
            Err(e) => {
                let error_event = dto::SkillOptimizeEvent::Error { detail: e };
                let data = serde_json::to_string(&error_event).unwrap_or_default();
                Ok(Event::default().data(data))
            }
        }
    });

    Sse::new(sse_stream)
        .keep_alive(
            axum::response::sse::KeepAlive::new()
                .interval(std::time::Duration::from_secs(15))
                .text("keepalive"),
        )
        .into_response()
}

async fn preview_hub_skill(body: bytes::Bytes) -> Response {
    use std::io::{Cursor, Read};

    let cursor = Cursor::new(body.as_ref());
    let mut archive = match zip::ZipArchive::new(cursor) {
        Ok(a) => a,
        Err(e) => {
            return ApiError::BadRequest(format!("Invalid zip file: {e}")).into_response();
        }
    };

    let manifest: serde_json::Value = match archive.by_name("manifest.json") {
        Ok(mut file) => {
            let mut buf = String::new();
            let _ = file.read_to_string(&mut buf);
            serde_json::from_str(&buf).unwrap_or_default()
        }
        Err(_) => serde_json::Value::Null,
    };

    let files: Vec<String> = (0..archive.len())
        .filter_map(|i| archive.by_index(i).ok().map(|f| f.name().to_string()))
        .collect();

    let name = manifest.get("name").and_then(|v| v.as_str()).unwrap_or("");
    let description = manifest
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let response = serde_json::json!({
        "name": name,
        "description": description,
        "files": files,
        "manifest": manifest,
    });

    (StatusCode::OK, Json(response)).into_response()
}

async fn agent_list_skills(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
) -> Response {
    match state.skill_service.list_skills(Some(&agent_id)).await {
        Ok(skills) => (StatusCode::OK, Json(skills)).into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn agent_create_skill(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Json(body): Json<dto::SkillCreate>,
) -> Response {
    match state
        .skill_service
        .create_skill(Some(&agent_id), body)
        .await
    {
        Ok(skill) => {
            publish_workspace_skill_committed(&state, &agent_id, &skill.id).await;
            schedule_skill_refresh(state.registry.clone(), &agent_id);
            (StatusCode::CREATED, Json(skill)).into_response()
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn agent_refresh_skills(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
) -> Response {
    match state.skill_service.refresh_skills(Some(&agent_id)).await {
        Ok(resp) => (StatusCode::OK, Json(resp)).into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn agent_hub_install_start(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Json(body): Json<dto::SkillHubInstallStartRequest>,
) -> Response {
    match state.skill_service.hub_install_start(&agent_id, body).await {
        Ok(status) => {
            spawn_hub_install_sync_publication_watcher(
                state.clone(),
                agent_id,
                status.task_id.clone(),
            );
            (StatusCode::OK, Json(status)).into_response()
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn agent_hub_install_status(
    State(state): State<AppState>,
    Path((agent_id, task_id)): Path<(String, String)>,
) -> Response {
    match state
        .skill_service
        .hub_install_status(&agent_id, &task_id)
        .await
    {
        Ok(status) => {
            publish_completed_hub_install_once(&state, &agent_id, &task_id, &status).await;
            (StatusCode::OK, Json(status)).into_response()
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn agent_hub_install_cancel(
    State(state): State<AppState>,
    Path((agent_id, task_id)): Path<(String, String)>,
) -> Response {
    match state
        .skill_service
        .hub_install_cancel(&agent_id, &task_id)
        .await
    {
        Ok(()) => (StatusCode::OK, Json(dto::SuccessResponse::ok())).into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn agent_upload_skill(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Query(query): Query<SkillUploadQuery>,
    body: bytes::Bytes,
) -> Response {
    let options = match query.into_options(true) {
        Ok(options) => options,
        Err(error) => return error.into_response(),
    };
    match state
        .skill_service
        .upload_skill(Some(&agent_id), body, options)
        .await
    {
        Ok(skill) => {
            for name in &skill.imported {
                publish_workspace_skill_committed(&state, &agent_id, name).await;
            }
            (StatusCode::OK, Json(skill)).into_response()
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn agent_batch_delete_skills(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Json(body): Json<dto::BatchDeleteRequest>,
) -> Response {
    let names = body.names.clone();
    match state
        .skill_service
        .batch_delete(Some(&agent_id), body)
        .await
    {
        Ok(()) => {
            for name in &names {
                publish_workspace_skill_deleted(&state, &agent_id, name).await;
            }
            (StatusCode::OK, Json(dto::SuccessResponse::ok())).into_response()
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn agent_batch_disable_skills(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Json(body): Json<dto::BatchToggleBody>,
) -> Response {
    let names = match body {
        dto::BatchToggleBody::Array(names) => names,
        dto::BatchToggleBody::Object(req) => req.names,
    };
    let mut results = serde_json::Map::new();
    let mut any_changed = false;
    for name in &names {
        match state
            .skill_service
            .toggle_skill(Some(&agent_id), name, false)
            .await
        {
            Ok((_, changed)) => {
                any_changed |= changed;
                if changed {
                    publish_workspace_skill_committed(&state, &agent_id, name).await;
                }
                results.insert(
                    name.clone(),
                    serde_json::json!({
                        "disabled": true,
                        "success": true,
                        "updated_workspaces": [&agent_id],
                    }),
                );
            }
            Err(e) => {
                results.insert(
                    name.clone(),
                    serde_json::json!({
                        "success": false,
                        "reason": e.to_string(),
                    }),
                );
            }
        }
    }
    if any_changed {
        schedule_skill_refresh(state.registry.clone(), &agent_id);
    }
    (
        StatusCode::OK,
        Json(serde_json::json!({ "results": results })),
    )
        .into_response()
}

async fn agent_batch_enable_skills(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Json(body): Json<dto::BatchToggleBody>,
) -> Response {
    let names = match body {
        dto::BatchToggleBody::Array(names) => names,
        dto::BatchToggleBody::Object(req) => req.names,
    };
    let mut results = serde_json::Map::new();
    let mut any_changed = false;
    for name in &names {
        match state
            .skill_service
            .toggle_skill(Some(&agent_id), name, true)
            .await
        {
            Ok((_, changed)) => {
                any_changed |= changed;
                if changed {
                    publish_workspace_skill_committed(&state, &agent_id, name).await;
                }
                results.insert(
                    name.clone(),
                    serde_json::json!({
                        "enabled": true,
                        "success": true,
                        "updated_workspaces": [&agent_id],
                        "failed": [],
                        "reason": null,
                    }),
                );
            }
            Err(e) => {
                results.insert(
                    name.clone(),
                    serde_json::json!({
                        "success": false,
                        "reason": e.to_string(),
                    }),
                );
            }
        }
    }
    if any_changed {
        schedule_skill_refresh(state.registry.clone(), &agent_id);
    }
    (
        StatusCode::OK,
        Json(serde_json::json!({ "results": results })),
    )
        .into_response()
}

async fn agent_save_skill(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Json(body): Json<dto::SkillSaveRequest>,
) -> Response {
    match state.skill_service.save_skill(Some(&agent_id), body).await {
        Ok((skill, changed)) => {
            if changed {
                publish_workspace_skill_committed(&state, &agent_id, &skill.id).await;
                schedule_skill_refresh(state.registry.clone(), &agent_id);
            }
            (StatusCode::OK, Json(skill)).into_response()
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn agent_enable_skill(
    State(state): State<AppState>,
    Path((agent_id, skill_name)): Path<(String, String)>,
) -> Response {
    match state
        .skill_service
        .toggle_skill(Some(&agent_id), &skill_name, true)
        .await
    {
        Ok((_skill, changed)) => {
            if changed {
                publish_workspace_skill_committed(&state, &agent_id, &skill_name).await;
                schedule_skill_refresh(state.registry.clone(), &agent_id);
            }
            let resp = serde_json::json!({
                "enabled": true,
                "success": true,
                "updated_workspaces": [&agent_id],
                "failed": [],
                "reason": null,
            });
            (StatusCode::OK, Json(resp)).into_response()
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn agent_disable_skill(
    State(state): State<AppState>,
    Path((agent_id, skill_name)): Path<(String, String)>,
) -> Response {
    match state
        .skill_service
        .toggle_skill(Some(&agent_id), &skill_name, false)
        .await
    {
        Ok((_skill, changed)) => {
            if changed {
                publish_workspace_skill_committed(&state, &agent_id, &skill_name).await;
                schedule_skill_refresh(state.registry.clone(), &agent_id);
            }
            let resp = serde_json::json!({
                "disabled": true,
                "success": true,
                "updated_workspaces": [&agent_id],
            });
            (StatusCode::OK, Json(resp)).into_response()
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn agent_get_skill(
    State(state): State<AppState>,
    Path((agent_id, skill_name)): Path<(String, String)>,
) -> Response {
    match state
        .skill_service
        .get_skill(Some(&agent_id), &skill_name)
        .await
    {
        Ok(Some(skill)) => (StatusCode::OK, Json(skill)).into_response(),
        Ok(None) => ApiError::NotFound(format!("Skill '{skill_name}' not found")).into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn agent_delete_skill(
    State(state): State<AppState>,
    Path((agent_id, skill_name)): Path<(String, String)>,
) -> Response {
    match state
        .skill_service
        .delete_skill(Some(&agent_id), &skill_name)
        .await
    {
        Ok(()) => {
            publish_workspace_skill_deleted(&state, &agent_id, &skill_name).await;

            schedule_skill_refresh(state.registry.clone(), &agent_id);
            (StatusCode::OK, Json(dto::DeletedResponse::ok())).into_response()
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn agent_get_skill_file(
    State(state): State<AppState>,
    Path((agent_id, skill_name, file_path)): Path<(String, String, String)>,
) -> Response {
    match state
        .skill_service
        .get_skill_file(Some(&agent_id), &skill_name, &file_path)
        .await
    {
        Ok(Some(content)) => (StatusCode::OK, Json(content)).into_response(),
        Ok(None) => ApiError::NotFound(format!(
            "File '{file_path}' not found in skill '{skill_name}'"
        ))
        .into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn agent_update_skill_channels(
    State(state): State<AppState>,
    Path((agent_id, skill_name)): Path<(String, String)>,
    Json(body): Json<dto::SkillChannelsRequest>,
) -> Response {
    match state
        .skill_service
        .update_skill_channels(Some(&agent_id), &skill_name, body)
        .await
    {
        Ok(()) => {
            publish_workspace_skill_committed(&state, &agent_id, &skill_name).await;
            schedule_skill_refresh(state.registry.clone(), &agent_id);
            (StatusCode::OK, Json(dto::SuccessResponse::ok())).into_response()
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn agent_get_skill_config(
    State(state): State<AppState>,
    Path((agent_id, skill_name)): Path<(String, String)>,
) -> Response {
    match state
        .skill_service
        .get_skill_config(Some(&agent_id), &skill_name)
        .await
    {
        Ok(config) => (StatusCode::OK, Json(config)).into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn agent_update_skill_config(
    State(state): State<AppState>,
    Path((agent_id, skill_name)): Path<(String, String)>,
    Json(body): Json<dto::SkillConfig>,
) -> Response {
    match state
        .skill_service
        .update_skill_config(Some(&agent_id), &skill_name, body)
        .await
    {
        Ok(config) => {
            publish_workspace_skill_committed(&state, &agent_id, &skill_name).await;
            (StatusCode::OK, Json(config)).into_response()
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn agent_delete_skill_config(
    State(state): State<AppState>,
    Path((agent_id, skill_name)): Path<(String, String)>,
) -> Response {
    match state
        .skill_service
        .delete_skill_config(Some(&agent_id), &skill_name)
        .await
    {
        Ok(()) => {
            publish_workspace_skill_committed(&state, &agent_id, &skill_name).await;
            (StatusCode::OK, Json(dto::SuccessResponse::ok())).into_response()
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}
