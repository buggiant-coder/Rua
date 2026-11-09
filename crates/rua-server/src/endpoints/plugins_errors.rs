use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::state::AppState;

use super::plugins::InstallPluginRequest;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PluginInstallError {
    ManifestParseFailed {
        detail: String,
    },
    WasmLoadFailed {
        detail: String,
    },
    RegistryAddFailed {
        detail: String,
    },
    ToolRegisterFailed {
        tool: String,
        reason: String,
    },
    SkillSourceConflict {
        skill_name: String,
        existing: String,
    },
    HookRegisterFailed {
        hook_name: String,
        detail: String,
    },
    DownloadFailed {
        url: String,
        detail: String,
    },
    DownloadTooLarge {
        url: String,
        size: u64,
        max: u64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginInstallResult {
    pub plugin_id: String,
    pub plugin_name: String,
    pub source_path: String,

    pub status: String,

    pub installed: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<PluginInstallError>,
}

pub async fn install_plugin_checked(state: &AppState, body: InstallPluginRequest) -> Response {
    let is_url = body.source.starts_with("http://") || body.source.starts_with("https://");
    let _temp_dir: Option<tempfile::TempDir>;

    let plugin = if is_url {
        let td = match tempfile::TempDir::new() {
            Ok(td) => td,
            Err(e) => {
                return fail_response(
                    String::new(),
                    String::new(),
                    &body.source,
                    vec![PluginInstallError::DownloadFailed {
                        url: body.source.clone(),
                        detail: format!("failed to create temp dir: {e}"),
                    }],
                );
            }
        };
        let target = td.path().to_path_buf();
        let url = body.source.clone();
        match rua_plugin_runtime::PluginLoader::from_url(&url, &target).await {
            Ok(p) => {
                _temp_dir = Some(td);
                p
            }
            Err(e) => {
                let error = classify_load_error(&e);
                return fail_response(String::new(), String::new(), &body.source, vec![error]);
            }
        }
    } else {
        _temp_dir = None;
        let load_source = body.source.clone();
        let load_result = tokio::task::spawn_blocking(move || {
            rua_plugin_runtime::PluginLoader::load(std::path::Path::new(&load_source))
        })
        .await;
        match load_result {
            Ok(Ok(p)) => p,
            Ok(Err(e)) => {
                let error = classify_load_error(&e);
                return fail_response(String::new(), String::new(), &body.source, vec![error]);
            }
            Err(e) => {
                return fail_response(
                    String::new(),
                    String::new(),
                    &body.source,
                    vec![PluginInstallError::WasmLoadFailed {
                        detail: format!("Task panicked: {e}"),
                    }],
                );
            }
        }
    };

    let plugin_id = plugin.manifest.id.clone();
    let plugin_name = plugin.manifest.name.clone();
    let plugin_version = plugin.manifest.version.clone();
    let manifest_author = plugin.manifest.author.clone();
    let manifest_entry_point = plugin.manifest.entry_point.clone();
    let manifest_min_rua_version = plugin.manifest.min_rua_version.clone();
    let manifest_meta = plugin.manifest.meta.clone();
    let manifest_providers = plugin.manifest.providers.clone();

    if body.force {
        let _ = state.plugins.uninstall(&plugin_id).await;
        let _ = state.plugin_registry.remove(&plugin_id);
        state.plugin_registry.remove_plugin_tools(&plugin_id);
        state.plugin_registry.untrack_plugin_skills(&plugin_id);
        state.plugin_registry.remove_plugin_hooks(&plugin_id).await;
    }

    if let Err(e) = state.plugin_registry.add(plugin) {
        return fail_response(
            plugin_id,
            plugin_name,
            &body.source,
            vec![PluginInstallError::RegistryAddFailed {
                detail: e.to_string(),
            }],
        );
    }

    let mut errors: Vec<PluginInstallError> = Vec::new();

    collect_tool_errors(
        &state.plugin_registry,
        &plugin_id,
        &manifest_meta,
        &mut errors,
    );
    collect_hook_errors(
        &state.plugin_registry,
        &plugin_id,
        &manifest_meta,
        &mut errors,
    )
    .await;

    register_plugin_providers(state, &manifest_providers).await;

    let status_str = if errors.is_empty() {
        "installed"
    } else {
        "partial"
    };

    let mut extra = serde_json::Map::new();
    if !manifest_author.is_empty() {
        extra.insert("author".into(), serde_json::Value::String(manifest_author));
    }
    extra.insert(
        "entry_point".into(),
        serde_json::Value::String(manifest_entry_point),
    );
    extra.insert(
        "source_path".into(),
        serde_json::Value::String(body.source.clone()),
    );
    extra.insert(
        "min_rua_version".into(),
        serde_json::Value::String(manifest_min_rua_version),
    );
    if !manifest_meta.is_empty() {
        extra.insert(
            "meta".into(),
            serde_json::to_value(&manifest_meta).unwrap_or_default(),
        );
    }
    extra.insert(
        "status".into(),
        serde_json::Value::String(status_str.into()),
    );

    let source_url = if is_url {
        Some(body.source.clone())
    } else {
        None
    };

    let req = rua_plugin::InstallRequest {
        plugin_id: plugin_id.clone(),
        version: Some(plugin_version),
        source_url,
        extra,
    };

    match state.plugins.install(req).await {
        Ok(record) => {
            if let Err(error) = crate::services::plugin_hook_definitions::register_from_record(
                &state.product_hooks,
                &record,
            )
            .await
            {
                errors.push(PluginInstallError::HookRegisterFailed {
                    hook_name: "hook_definitions".to_string(),
                    detail: error,
                });
            }

            if let Err(e) = state
                .plugin_registry
                .dispatch_hook(rua_plugin_runtime::HookPoint::Startup, "{}")
                .await
            {
                tracing::warn!(
                    plugin_id = %plugin_id,
                    error = %e,
                    "Startup hook dispatch failed after install, continuing"
                );
            }

            let http_status = if errors.is_empty() {
                StatusCode::OK
            } else {
                StatusCode::MULTI_STATUS
            };
            let result_status = if errors.is_empty() {
                "installed"
            } else {
                "partial"
            };
            let result = PluginInstallResult {
                plugin_id,
                plugin_name,
                source_path: body.source,
                status: result_status.into(),
                installed: true,
                errors,
            };
            (http_status, Json(result)).into_response()
        }
        Err(e) => {
            let _ = state.plugin_registry.remove(&plugin_id);
            state.plugin_registry.remove_plugin_tools(&plugin_id);

            fail_response(
                plugin_id,
                plugin_name,
                &body.source,
                vec![PluginInstallError::RegistryAddFailed {
                    detail: format!("Persist failed, rolled back: {e}"),
                }],
            )
        }
    }
}

fn classify_load_error(e: &rua_plugin_runtime::PluginLoadError) -> PluginInstallError {
    use rua_plugin_runtime::PluginLoadError;
    match e {
        PluginLoadError::Manifest(_) => PluginInstallError::ManifestParseFailed {
            detail: e.to_string(),
        },
        PluginLoadError::DownloadFailed { url, detail } => PluginInstallError::DownloadFailed {
            url: url.clone(),
            detail: detail.clone(),
        },
        PluginLoadError::DownloadTooLarge { url, size, max } => {
            PluginInstallError::DownloadTooLarge {
                url: url.clone(),
                size: *size,
                max: *max,
            }
        }
        PluginLoadError::ZipSlip { entry } => PluginInstallError::DownloadFailed {
            url: String::new(),
            detail: format!("zip-slip: entry '{entry}' escapes target directory"),
        },
        PluginLoadError::ZipError { detail } => PluginInstallError::DownloadFailed {
            url: String::new(),
            detail: format!("zip extraction failed: {detail}"),
        },
        _ => PluginInstallError::WasmLoadFailed {
            detail: e.to_string(),
        },
    }
}

fn fail_response(
    plugin_id: String,
    plugin_name: String,
    source_path: &str,
    errors: Vec<PluginInstallError>,
) -> Response {
    let result = PluginInstallResult {
        plugin_id,
        plugin_name,
        source_path: source_path.to_string(),
        status: "failed".into(),
        installed: false,
        errors,
    };
    (StatusCode::UNPROCESSABLE_ENTITY, Json(result)).into_response()
}

fn collect_tool_errors(
    registry: &rua_plugin_runtime::PluginRegistry,
    plugin_id: &str,
    meta: &HashMap<String, serde_json::Value>,
    errors: &mut Vec<PluginInstallError>,
) {
    let tools_arr = match meta.get("tools").and_then(|v| v.as_array()) {
        Some(arr) => arr,
        None => return,
    };

    for tool_val in tools_arr {
        let tool_json = tool_val.to_string();
        let tool_name = tool_val
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("<unknown>")
            .to_string();

        match serde_json::from_str::<rua_plugin_runtime::ToolSpec>(&tool_json) {
            Ok(spec) => {
                if let Err(e) = registry.register_tool(plugin_id, spec) {
                    errors.push(PluginInstallError::ToolRegisterFailed {
                        tool: tool_name,
                        reason: e.to_string(),
                    });
                }
            }
            Err(e) => {
                errors.push(PluginInstallError::ToolRegisterFailed {
                    tool: tool_name,
                    reason: e.to_string(),
                });
            }
        }
    }
}

async fn collect_hook_errors(
    registry: &rua_plugin_runtime::PluginRegistry,
    plugin_id: &str,
    meta: &HashMap<String, serde_json::Value>,
    errors: &mut Vec<PluginInstallError>,
) {
    let hooks_arr = match meta.get("hooks").and_then(|v| v.as_array()) {
        Some(arr) => arr,
        None => return,
    };

    for hook_val in hooks_arr {
        let hook_json = hook_val.to_string();
        let hook_name = hook_val
            .get("hook_name")
            .and_then(|v| v.as_str())
            .unwrap_or("<unknown>")
            .to_string();

        match serde_json::from_str::<rua_plugin_runtime::HookSpec>(&hook_json) {
            Ok(spec) => {
                if let Err(e) = registry.register_hook(plugin_id, spec).await {
                    errors.push(PluginInstallError::HookRegisterFailed {
                        hook_name,
                        detail: e.to_string(),
                    });
                }
            }
            Err(e) => {
                errors.push(PluginInstallError::HookRegisterFailed {
                    hook_name,
                    detail: e.to_string(),
                });
            }
        }
    }
}

async fn register_plugin_providers(
    state: &AppState,
    providers: &[rua_plugin_runtime::PluginProviderDecl],
) {
    for decl in providers {
        let spec = rua_provider::manager::PluginProviderSpec {
            id: decl.id.clone(),
            label: decl.label.clone(),
            base_url: decl.base_url.clone(),
            chat_model: decl.chat_model.clone(),
            preset_models: decl.preset_models.clone(),
            require_api_key: decl.require_api_key,
        };
        if let Err(e) = state.provider_manager.register_plugin_provider(&spec).await {
            tracing::warn!(
                provider_id = %decl.id,
                error = %e,
                "failed to register plugin provider, skipping"
            );
        }
    }
}
