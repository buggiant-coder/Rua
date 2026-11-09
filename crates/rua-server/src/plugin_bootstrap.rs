use std::sync::Arc;

use crate::state::AppState;

pub(crate) fn load_persisted_plugins(state: &AppState) {
    let plugins = state.plugins.clone();
    let plugin_registry = Arc::clone(&state.plugin_registry);

    let _ = std::thread::Builder::new()
        .name("plugin-bootstrap".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("failed to build plugin bootstrap runtime");
            rt.block_on(async move {
                let records = plugins.list().await;
                let active: Vec<_> = records.into_iter().filter(|r| r.enabled).collect();
                if active.is_empty() {
                    return;
                }
                tracing::info!(count = active.len(), "loading persisted plugins at startup");

                for record in &active {
                    let source_path = record
                        .extra
                        .get("source_path")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if source_path.is_empty() {
                        tracing::warn!(
                            plugin_id = %record.id,
                            "no source_path in persisted record, skipping startup load"
                        );
                        continue;
                    }

                    let path = std::path::PathBuf::from(source_path);
                    let plugin = match rua_plugin_runtime::PluginLoader::load(&path) {
                        Ok(p) => p,
                        Err(e) => {
                            tracing::warn!(
                                plugin_id = %record.id,
                                source = %source_path,
                                error = %e,
                                "failed to load plugin at startup, skipping"
                            );
                            continue;
                        }
                    };

                    let plugin_id = plugin.manifest.id.clone();
                    let manifest_meta = plugin.manifest.meta.clone();

                    if let Err(e) = plugin_registry.add(plugin) {
                        tracing::warn!(
                            plugin_id = %plugin_id,
                            error = %e,
                            "failed to add plugin to registry at startup, skipping"
                        );
                        continue;
                    }

                    if let Some(tools_arr) = manifest_meta.get("tools").and_then(|v| v.as_array()) {
                        for tool_val in tools_arr {
                            let tool_json = tool_val.to_string();
                            match serde_json::from_str::<rua_plugin_runtime::ToolSpec>(&tool_json) {
                                Ok(spec) => {
                                    if let Err(e) = plugin_registry.register_tool(&plugin_id, spec)
                                    {
                                        tracing::warn!(
                                            plugin_id = %plugin_id,
                                            error = %e,
                                            "failed to register tool at startup"
                                        );
                                    }
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        plugin_id = %plugin_id,
                                        error = %e,
                                        "failed to parse tool spec at startup"
                                    );
                                }
                            }
                        }
                    }

                    if let Some(hooks_arr) = manifest_meta.get("hooks").and_then(|v| v.as_array()) {
                        for hook_val in hooks_arr {
                            let hook_json = hook_val.to_string();
                            match serde_json::from_str::<rua_plugin_runtime::HookSpec>(&hook_json) {
                                Ok(spec) => {
                                    if let Err(e) =
                                        plugin_registry.register_hook(&plugin_id, spec).await
                                    {
                                        tracing::warn!(
                                            plugin_id = %plugin_id,
                                            error = %e,
                                            "failed to register hook at startup"
                                        );
                                    }
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        plugin_id = %plugin_id,
                                        error = %e,
                                        "failed to parse hook spec at startup"
                                    );
                                }
                            }
                        }
                    }
                }

                if let Err(e) = plugin_registry
                    .dispatch_hook(rua_plugin_runtime::HookPoint::Startup, "{}")
                    .await
                {
                    tracing::warn!(
                        error = %e,
                        "Startup hook dispatch failed at cold start, continuing"
                    );
                }

                tracing::info!("plugin startup hooks dispatched");
            });
        });
}
