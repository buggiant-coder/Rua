use std::sync::Arc;

use async_trait::async_trait;
use mcp_manager::manager::McpManager;
use mcp_manager::types::McpConfig;

use crate::error::WorkspaceError;
use crate::service_manager::ServiceLifecycle;

/// Startup and shutdown of the per-Agent MCP manager.
///
/// Server connections configured with `auto_connect` are established lazily by
/// the manager, so `start` applies the initial configuration and `stop` really
/// shuts every connection down. Both report partial failures of their own
/// reports, so a `Ready` capability means the load/shutdown actually succeeded
/// for every declared server. The observed state is owned by
/// [`crate::service_manager::ServiceManager`].
pub struct McpServiceAdapter {
    manager: Arc<dyn McpManager>,
    initial_config: Option<McpConfig>,
}

impl McpServiceAdapter {
    pub fn new(manager: Arc<dyn McpManager>, initial_config: Option<McpConfig>) -> Self {
        Self {
            manager,
            initial_config,
        }
    }
}

/// Renders the per-server failures of a report.
///
/// Only server IDs are reported: the accompanying messages can embed external
/// endpoints, commands, or credentials and must not reach health output.
fn failed_servers(
    errors: &std::collections::BTreeMap<mcp_manager::types::McpServerId, String>,
) -> String {
    let mut ids: Vec<&str> = errors.keys().map(|id| id.as_str()).collect();
    ids.sort_unstable();
    ids.join(", ")
}

#[async_trait]
impl ServiceLifecycle for McpServiceAdapter {
    async fn start(&self) -> Result<(), WorkspaceError> {
        let Some(config) = self.initial_config.clone() else {
            // The owner already applied its own configuration; querying the
            // manager is the only evidence that the assembled resource answers.
            return self
                .manager
                .list_servers()
                .await
                .map(|_| ())
                .map_err(|e| WorkspaceError::component_init_failed("mcp", e.to_string()));
        };

        // The load result is the startup result: a manager that could not apply
        // its configuration has not started, even when the call itself returned
        // `Ok`. Reporting `Ready` here would be the placeholder this lifecycle
        // exists to remove.
        let report = self
            .manager
            .load_config(config)
            .await
            .map_err(|e| WorkspaceError::component_init_failed("mcp", e.to_string()))?;

        if !report.errors.is_empty() {
            return Err(WorkspaceError::component_init_failed(
                "mcp",
                format!(
                    "{} of {} server(s) failed to load: {}",
                    report.errors.len(),
                    report.errors.len() + report.added.len() + report.updated.len(),
                    failed_servers(&report.errors)
                ),
            ));
        }

        tracing::info!(
            added = report.added.len(),
            removed = report.removed.len(),
            errors = report.errors.len(),
            "MCP manager loaded initial config",
        );
        Ok(())
    }

    async fn stop(&self) -> Result<(), WorkspaceError> {
        let report =
            self.manager.shutdown_all().await.map_err(|error| {
                WorkspaceError::component_shutdown_failed("mcp", error.to_string())
            })?;

        if !report.errors.is_empty() {
            return Err(WorkspaceError::component_shutdown_failed(
                "mcp",
                format!(
                    "{} server(s) failed to shut down: {}",
                    report.errors.len(),
                    failed_servers(&report.errors)
                ),
            ));
        }

        tracing::info!(
            stopped = report.success.len(),
            "MCP manager shut down all servers"
        );
        Ok(())
    }
}
