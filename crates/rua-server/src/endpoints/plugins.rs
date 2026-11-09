use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::errors::ApiError;
use crate::state::AppState;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginSummary {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
    #[serde(default)]
    pub provider_count: usize,
    #[serde(default)]
    pub hook_count: usize,
    #[serde(default)]
    pub command_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginDetail {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
    #[serde(default)]
    pub provider_count: usize,
    #[serde(default)]
    pub hook_count: usize,
    #[serde(default)]
    pub command_count: usize,
    #[serde(default)]
    pub entry_point: String,
    #[serde(default)]
    pub source_path: String,
    #[serde(default)]
    pub min_rua_version: String,
    #[serde(default)]
    pub meta: HashMap<String, serde_json::Value>,
    #[serde(default)]
    pub providers: Vec<String>,
    #[serde(default)]
    pub startup_hooks: Vec<String>,
    #[serde(default)]
    pub shutdown_hooks: Vec<String>,
    #[serde(default)]
    pub commands: Vec<String>,
    #[serde(default)]
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallPluginRequest {
    pub source: String,
    #[serde(default)]
    pub force: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallPluginResponse {
    pub plugin_id: String,
    pub plugin_name: String,
    pub source_path: String,
    #[serde(default = "default_true")]
    pub installed: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UninstallPluginResponse {
    pub plugin_id: String,
    #[serde(default = "default_true")]
    pub removed: bool,
    #[serde(default = "default_true")]
    pub requires_restart: bool,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/plugins", get(list_plugins))
        .route("/plugins/install", post(install_plugin))
        .route(
            "/plugins/{pluginId}",
            get(get_plugin).delete(uninstall_plugin),
        )
}

fn record_to_summary(r: &rua_plugin::PluginRecord) -> PluginSummary {
    PluginSummary {
        id: r.id.clone(),
        name: r.name.clone(),
        version: r.version.clone(),
        description: r.description.clone(),
        author: r
            .extra
            .get("author")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        enabled: r.enabled,
        source_url: r.source_url.clone(),
        provider_count: 0,
        hook_count: 0,
        command_count: 0,
    }
}

fn record_to_detail(r: &rua_plugin::PluginRecord) -> PluginDetail {
    let summary = record_to_summary(r);
    let meta_map = r
        .extra
        .get("meta")
        .and_then(|v| serde_json::from_value::<HashMap<String, serde_json::Value>>(v.clone()).ok())
        .unwrap_or_default();

    let (startup_hooks, shutdown_hooks) = extract_hook_names(&meta_map);

    PluginDetail {
        id: summary.id,
        name: summary.name,
        version: summary.version,
        description: summary.description,
        author: summary.author,
        enabled: summary.enabled,
        source_url: summary.source_url,
        provider_count: summary.provider_count,
        hook_count: summary.hook_count,
        command_count: summary.command_count,
        entry_point: r
            .extra
            .get("entry_point")
            .and_then(|v| v.as_str())
            .unwrap_or("plugin.wasm")
            .to_string(),
        source_path: r
            .extra
            .get("source_path")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        min_rua_version: r
            .extra
            .get("min_rua_version")
            .and_then(|v| v.as_str())
            .unwrap_or("0.1.0")
            .to_string(),
        meta: meta_map,
        providers: Vec::new(),
        startup_hooks,
        shutdown_hooks,
        commands: Vec::new(),
        diagnostics: Vec::new(),
    }
}

fn extract_hook_names(meta: &HashMap<String, serde_json::Value>) -> (Vec<String>, Vec<String>) {
    let mut startup = Vec::new();
    let mut shutdown = Vec::new();
    if let Some(hooks_arr) = meta.get("hooks").and_then(|v| v.as_array()) {
        for hook_val in hooks_arr {
            let point = hook_val
                .get("hook_point")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let name = hook_val
                .get("hook_name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if name.is_empty() {
                continue;
            }
            match point {
                "startup" => startup.push(name),
                "shutdown" => shutdown.push(name),
                _ => {}
            }
        }
    }
    (startup, shutdown)
}

async fn list_plugins(State(state): State<AppState>) -> Response {
    let records = state.plugins.list().await;
    let plugins: Vec<PluginSummary> = records.iter().map(record_to_summary).collect();
    (StatusCode::OK, Json(plugins)).into_response()
}

async fn get_plugin(State(state): State<AppState>, Path(plugin_id): Path<String>) -> Response {
    match state.plugins.get(&plugin_id).await {
        Some(record) => (StatusCode::OK, Json(record_to_detail(&record))).into_response(),
        None => ApiError::NotFound(format!("Plugin '{plugin_id}' not found")).into_response(),
    }
}

async fn install_plugin(
    State(state): State<AppState>,
    Json(body): Json<InstallPluginRequest>,
) -> Response {
    super::plugins_errors::install_plugin_checked(&state, body).await
}

async fn uninstall_plugin(
    State(state): State<AppState>,
    Path(plugin_id): Path<String>,
) -> Response {
    if state.plugins.get(&plugin_id).await.is_none() {
        return ApiError::NotFound(format!("Plugin '{plugin_id}' not found")).into_response();
    }

    state
        .product_hooks
        .mark_plugin_unavailable(&plugin_id, "Plugin uninstalled")
        .await;

    let mut errors: Vec<String> = Vec::new();

    for decl in state.plugin_registry.get_manifest_providers(&plugin_id) {
        state.provider_manager.unregister_plugin_provider(&decl.id);
    }

    let pool_dir = state.rua_root.join("skill_pool");
    if let Ok(pool) = rua_skills::SkillPoolService::new_in(&pool_dir) {
        let registrar = rua_plugin_runtime::SkillRegistrar::new(&state.plugin_registry, &pool)
            .with_skill_registry(&state.plugin_skill_registry);
        if let Err(e) =
            rua_plugin_runtime::skill_registry::remove_plugin_skills(&registrar, &plugin_id)
        {
            errors.push(format!("remove_plugin_skills: {e}"));
        }
    } else {
        state.plugin_registry.untrack_plugin_skills(&plugin_id);
        state.plugin_skill_registry.unregister_plugin(&plugin_id);
    }

    state.plugin_registry.remove_plugin_hooks(&plugin_id).await;

    state.plugin_registry.remove_plugin_tools(&plugin_id);

    if let Err(e) = state.plugin_registry.remove(&plugin_id) {
        errors.push(format!("registry remove: {e}"));
    }

    if let Err(e) = state.plugins.uninstall(&plugin_id).await {
        errors.push(format!("uninstall persistence: {e}"));
    }

    if errors.is_empty() {
        let resp = UninstallPluginResponse {
            plugin_id,
            removed: true,
            requires_restart: false,
        };
        (StatusCode::OK, Json(resp)).into_response()
    } else {
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "plugin_id": plugin_id,
                "removed": false,
                "errors": errors,
            })),
        )
            .into_response()
    }
}
