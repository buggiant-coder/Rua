use crate::state::AppState;

#[derive(Debug, Clone, serde::Serialize)]
pub struct McpApplication {
    pub changed: bool,
    pub status: &'static str,
    pub saved: bool,
    pub failed_servers: Vec<String>,
}

pub async fn apply_current(state: &AppState, agent_id: &str) -> McpApplication {
    let Some(runtime) = state.registry.get(agent_id).await else {
        return McpApplication {
            changed: false,
            status: "deferred",
            saved: true,
            failed_servers: Vec::new(),
        };
    };
    let result = apply_loaded(
        &runtime,
        &state.agent_mcp_cells,
        state.device_key.as_bytes(),
        agent_id,
    )
    .await;
    if result.changed {
        state.mcp_tools_cache.invalidate_agent(agent_id).await;
    }
    result
}

pub async fn apply_loaded(
    runtime: &rua_workspace::WorkspaceRuntime,
    cells: &rua_mcp::AgentMcpCells,
    device_key: &[u8; 32],
    agent_id: &str,
) -> McpApplication {
    match runtime
        .apply_mcp_configuration(|| async {
            let value = cells.cell(agent_id).read_value().await.map_err(|_| {
                rua_workspace::WorkspaceError::internal("Saved MCP configuration could not be read")
            })?;
            match value {
                Some(value) => {
                    rua_mcp::McpClientStore::decrypt_mcp_config(&value, Some(device_key)).map_err(
                        |_| {
                            rua_workspace::WorkspaceError::internal(
                                "Saved MCP configuration could not be decoded",
                            )
                        },
                    )
                }
                None => Ok(mcp_manager::types::McpConfig::default()),
            }
        })
        .await
    {
        Ok(report) => {
            let changed = !report.added.is_empty()
                || !report.removed.is_empty()
                || !report.updated.is_empty()
                || !report.errors.is_empty();
            McpApplication {
                changed,
                status: if report.errors.is_empty() {
                    "applied"
                } else {
                    "failed"
                },
                saved: true,
                failed_servers: report.errors.keys().map(ToString::to_string).collect(),
            }
        }
        Err(_) => McpApplication {
            changed: true,
            status: "failed",
            saved: true,
            failed_servers: Vec::new(),
        },
    }
}

pub async fn reload_from_disk(state: &AppState, agent_id: &str) {
    let application = apply_current(state, agent_id).await;
    if application.status == "failed" {
        tracing::warn!(agent_id, "saved MCP configuration could not be applied");
    }
}
