use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use rua_skills::{compute_content_hash, parse_skill_md, read_install_meta, write_install_meta};
use rua_skills_hub::resolve_bundle;

use crate::state::AppState;
use crate::sync_publication::{publish_skill_pool_committed, publish_workspace_skill_committed};

use super::skills_hub::shared_client;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/agents/{agentId}/skills/{name}/check-update",
            post(check_update_handler),
        )
        .route(
            "/agents/{agentId}/skills/{name}/update",
            post(execute_update_handler),
        )
        .route(
            "/skills/pool/{name}/check-update",
            post(check_pool_update_handler),
        )
        .route(
            "/skills/pool/{name}/update",
            post(execute_pool_update_handler),
        )
}

#[derive(Debug, Serialize)]
pub struct CheckUpdateResponse {
    pub has_update: bool,
    pub current_version: String,
    pub latest_version: String,
    pub local_modified: bool,
}

#[derive(Debug, Deserialize)]
pub struct ExecuteUpdateBody {
    #[serde(default)]
    pub force: bool,
}

#[derive(Debug, Serialize)]
pub struct ExecuteUpdateResponse {
    pub updated: bool,
    pub old_version: String,
    pub new_version: String,
}

enum UpdateManifestTarget {
    Workspace {
        workspace_dir: PathBuf,
        agent_id: String,
    },
    Pool(PathBuf),
}

pub async fn check_update_handler(
    State(state): State<AppState>,
    Path((agent_id, name)): Path<(String, String)>,
) -> Response {
    let workspace_dir = state.skill_service.workspace_dir_for(Some(&agent_id));
    let skill_dir = workspace_dir.join("skills").join(&name);
    check_update_for_dir(skill_dir).await
}

pub async fn check_pool_update_handler(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Response {
    let skill_dir = state.rua_root.join("skill_pool").join(&name);
    check_update_for_dir(skill_dir).await
}

async fn check_update_for_dir(skill_dir: PathBuf) -> Response {
    let meta = match read_install_meta(&skill_dir) {
        Some(m) => m,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"detail": "skill has no _meta.json (not hub-installed)"})),
            )
                .into_response();
        }
    };

    let client = match shared_client().await {
        Ok(c) => c,
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({"detail": format!("hub client error: {e}")})),
            )
                .into_response();
        }
    };

    let (bundle, _source_url) =
        match resolve_bundle(client.as_ref(), &meta.source_url, "", None).await {
            Ok(b) => b,
            Err(e) => {
                return (
                    StatusCode::BAD_GATEWAY,
                    Json(serde_json::json!({"detail": format!("source unreachable: {e}")})),
                )
                    .into_response();
            }
        };

    let latest_version = parse_skill_md(&bundle.content)
        .map(|p| p.version)
        .unwrap_or_default();

    let local_modified = match std::fs::read_to_string(skill_dir.join("SKILL.md")) {
        Ok(content) => compute_content_hash(&content) != meta.content_hash,
        Err(_) => true,
    };

    let has_update = !latest_version.is_empty()
        && !meta.installed_version.is_empty()
        && latest_version != meta.installed_version;

    let resp = CheckUpdateResponse {
        has_update,
        current_version: meta.installed_version,
        latest_version,
        local_modified,
    };
    (StatusCode::OK, Json(resp)).into_response()
}

pub async fn execute_update_handler(
    State(state): State<AppState>,
    Path((agent_id, name)): Path<(String, String)>,
    body: Option<Json<ExecuteUpdateBody>>,
) -> Response {
    let force = body.map(|b| b.force).unwrap_or(false);
    let workspace_dir = state.skill_service.workspace_dir_for(Some(&agent_id));
    let skill_dir = workspace_dir.join("skills").join(&name);
    execute_update_for_dir(
        &state,
        &name,
        skill_dir,
        Some(UpdateManifestTarget::Workspace {
            workspace_dir,
            agent_id,
        }),
        force,
    )
    .await
}

pub async fn execute_pool_update_handler(
    State(state): State<AppState>,
    Path(name): Path<String>,
    body: Option<Json<ExecuteUpdateBody>>,
) -> Response {
    let force = body.map(|b| b.force).unwrap_or(false);
    let pool_dir = state.rua_root.join("skill_pool");
    let skill_dir = pool_dir.join(&name);
    execute_update_for_dir(
        &state,
        &name,
        skill_dir,
        Some(UpdateManifestTarget::Pool(pool_dir)),
        force,
    )
    .await
}

async fn execute_update_for_dir(
    state: &AppState,
    skill_name: &str,
    skill_dir: PathBuf,
    manifest_target: Option<UpdateManifestTarget>,
    force: bool,
) -> Response {
    let meta = match read_install_meta(&skill_dir) {
        Some(m) => m,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"detail": "skill has no _meta.json (not hub-installed)"})),
            )
                .into_response();
        }
    };

    let local_modified = match std::fs::read_to_string(skill_dir.join("SKILL.md")) {
        Ok(content) => compute_content_hash(&content) != meta.content_hash,
        Err(_) => true,
    };

    if local_modified && !force {
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": "local_modified",
                "message": "Skill has local modifications. Use force=true to overwrite."
            })),
        )
            .into_response();
    }

    let client = match shared_client().await {
        Ok(c) => c,
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({"detail": format!("hub client error: {e}")})),
            )
                .into_response();
        }
    };

    let (bundle, _source_url) =
        match resolve_bundle(client.as_ref(), &meta.source_url, "", None).await {
            Ok(b) => b,
            Err(e) => {
                return (
                    StatusCode::BAD_GATEWAY,
                    Json(serde_json::json!({"detail": format!("source unreachable: {e}")})),
                )
                    .into_response();
            }
        };

    let new_version = parse_skill_md(&bundle.content)
        .map(|p| p.version)
        .unwrap_or_default();
    let old_version = meta.installed_version.clone();

    if let Err(e) = std::fs::write(skill_dir.join("SKILL.md"), &bundle.content) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"detail": format!("write SKILL.md: {e}")})),
        )
            .into_response();
    }

    if let Err(e) = write_subtree(&skill_dir.join("references"), &bundle.references) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"detail": format!("write references: {e}")})),
        )
            .into_response();
    }
    if let Err(e) = write_subtree(&skill_dir.join("scripts"), &bundle.scripts) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"detail": format!("write scripts: {e}")})),
        )
            .into_response();
    }
    if let Err(e) = write_subtree(&skill_dir, &bundle.extra_files) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"detail": format!("write extra_files: {e}")})),
        )
            .into_response();
    }

    let updated_meta = rua_skills::InstallMeta {
        installed_version: new_version.clone(),
        updated_at: Some(chrono::Utc::now().to_rfc3339()),
        content_hash: compute_content_hash(&bundle.content),
        ..meta
    };

    if let Err(e) = write_install_meta(&skill_dir, &updated_meta) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"detail": format!("write _meta.json: {e}")})),
        )
            .into_response();
    }

    if let Some(manifest_target) = manifest_target {
        match manifest_target {
            UpdateManifestTarget::Workspace {
                workspace_dir,
                agent_id,
            } => {
                if let Err(e) = rua_skills::reconcile_workspace_manifest(&workspace_dir) {
                    tracing::warn!("manifest reconcile after update failed: {e}");
                }
                publish_workspace_skill_committed(state, &agent_id, skill_name).await;
            }
            UpdateManifestTarget::Pool(pool_dir) => {
                let sigs = std::collections::BTreeMap::new();
                if let Err(e) = rua_skills::reconcile_pool_manifest_with(&pool_dir, &sigs) {
                    tracing::warn!("pool manifest reconcile after update failed: {e}");
                }
                publish_skill_pool_committed(state, skill_name).await;
            }
        }
    }

    let resp = ExecuteUpdateResponse {
        updated: true,
        old_version,
        new_version,
    };
    (StatusCode::OK, Json(resp)).into_response()
}

fn write_subtree(
    root: &std::path::Path,
    tree: &serde_json::Map<String, serde_json::Value>,
) -> std::io::Result<()> {
    if tree.is_empty() {
        return Ok(());
    }
    std::fs::create_dir_all(root)?;
    for (key, value) in tree {
        if key == "." || key == ".." || key.contains('/') || key.contains('\\') {
            continue;
        }
        let path = root.join(key);
        match value {
            serde_json::Value::Object(child) => write_subtree(&path, child)?,
            serde_json::Value::String(body) => {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&path, body)?;
            }
            _ => {}
        }
    }
    Ok(())
}
