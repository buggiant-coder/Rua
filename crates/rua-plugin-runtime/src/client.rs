use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use mcp_manager::client::McpClient;
use mcp_manager::error::McpError;
use mcp_manager::types::{McpCallContext, McpCallResult, McpInitializeResult, McpToolInfo};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use rua_core::runtime_authority::{
    ProcessShape, RuntimeAuthority, RuntimeLaunchRequest, ToolchainIntent,
};
use rua_mcp::transports::stdio::{StdioConfig, StdioMcpClient};

use crate::protocol::{
    InvokeCliParams, InvokeCliResult, InvokeHookParams, InvokeHookResult, InvokeSkillParams,
    InvokeSkillResult, METHOD_INVOKE_CLI, METHOD_INVOKE_HOOK, METHOD_INVOKE_SKILL,
};

#[derive(Debug, Clone)]
pub struct PluginClientConfig {
    pub plugin_id: String,
    pub entrypoint: String,
    pub working_dir: PathBuf,
    pub secret_dir: PathBuf,
    pub runtime_cache_dir: PathBuf,
    pub env: BTreeMap<String, String>,
    pub env_remove: BTreeSet<String>,
    pub startup_timeout: Duration,
    pub call_timeout: Duration,
    pub heartbeat_interval: Duration,

    pub command: Option<String>,

    pub args: Option<Vec<String>>,
}

impl PluginClientConfig {
    pub fn new(plugin_id: impl Into<String>, entrypoint: impl Into<String>) -> Self {
        Self {
            plugin_id: plugin_id.into(),
            entrypoint: entrypoint.into(),
            working_dir: PathBuf::from("."),
            secret_dir: PathBuf::from("."),
            runtime_cache_dir: std::env::temp_dir().join("rua-runtime-cache"),
            env: BTreeMap::new(),
            env_remove: BTreeSet::new(),
            startup_timeout: Duration::from_secs(30),
            call_timeout: Duration::from_secs(60),
            heartbeat_interval: Duration::from_secs(30),
            command: None,
            args: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginHealth {
    Healthy,
    Unhealthy { reason: String },
    Stopped,
}

#[derive(Debug, thiserror::Error)]
pub enum PluginClientError {
    #[error("transport: {0}")]
    Transport(#[from] McpError),
    #[error("protocol: {message}")]
    Protocol { message: String },
    #[error("plugin not running")]
    NotRunning,
}

pub struct PluginClient {
    plugin_id: String,
    transport: Arc<StdioMcpClient>,
    call_timeout: Duration,
    health_tx: Arc<watch::Sender<PluginHealth>>,
    health_rx: watch::Receiver<PluginHealth>,
    heartbeat_handle: Option<JoinHandle<()>>,
}

impl PluginClient {
    pub async fn spawn(config: PluginClientConfig) -> Result<Self, PluginClientError> {
        let server_key = format!("plugin:{}", config.plugin_id);

        let command = config
            .command
            .clone()
            .unwrap_or_else(|| "python".to_string());
        let args = config.args.clone().unwrap_or_else(|| {
            vec![
                "-m".into(),
                config.entrypoint.clone(),
                "--plugin-id".into(),
                config.plugin_id.clone(),
                "--working-dir".into(),
                config.working_dir.to_string_lossy().into_owned(),
                "--secret-dir".into(),
                config.secret_dir.to_string_lossy().into_owned(),
            ]
        });

        let authority = RuntimeAuthority::from_current_process(
            None,
            &config.working_dir,
            &config.runtime_cache_dir,
        );
        let isolated = authority.inventory().default_external_runtime
            == rua_core::runtime_layout::RuntimeKind::LinuxCompat;
        let stdio_config = plugin_stdio_config_with_authority(
            StdioConfig {
                server_key,
                server_name: config.plugin_id.clone(),
                command,
                args,
                env: config.env.clone(),
                cwd: Some(config.working_dir.clone()),
                startup_timeout: config.startup_timeout,
                call_timeout: config.call_timeout,
                stderr_log_prefix: Some(format!("plugin:{}", config.plugin_id)),
            },
            &authority,
            config.env_remove.clone(),
        )
        .map_err(|error| PluginClientError::Protocol {
            message: error.to_string(),
        })?;

        let transport = StdioMcpClient::new(stdio_config);
        let transport = Arc::new(if isolated {
            transport.with_isolated_env()
        } else {
            transport.with_env_remove(config.env_remove.clone())
        });
        transport.start().await?;

        let (health_tx, health_rx) = watch::channel(PluginHealth::Healthy);
        let health_tx = Arc::new(health_tx);

        let mut client = Self {
            plugin_id: config.plugin_id,
            transport,
            call_timeout: config.call_timeout,
            health_tx,
            health_rx,
            heartbeat_handle: None,
        };

        client.start_heartbeat(config.heartbeat_interval);
        Ok(client)
    }

    pub fn plugin_id(&self) -> &str {
        &self.plugin_id
    }

    pub fn health(&self) -> PluginHealth {
        self.health_rx.borrow().clone()
    }

    pub fn health_watch(&self) -> watch::Receiver<PluginHealth> {
        self.health_rx.clone()
    }

    pub fn is_running(&self) -> bool {
        self.transport.is_running()
    }

    pub async fn initialize_result(&self) -> Result<McpInitializeResult, PluginClientError> {
        Ok(self.transport.initialize_result().await?)
    }

    pub async fn list_tools(&self) -> Result<Vec<McpToolInfo>, PluginClientError> {
        Ok(self.transport.list_tools().await?)
    }

    pub async fn call_tool(
        &self,
        name: &str,
        args: serde_json::Value,
    ) -> Result<McpCallResult, PluginClientError> {
        let ctx = McpCallContext {
            timeout: Some(self.call_timeout),
            ..Default::default()
        };
        Ok(self.transport.call_tool(name, args, ctx).await?)
    }

    pub async fn invoke_hook(
        &self,
        params: InvokeHookParams,
    ) -> Result<InvokeHookResult, PluginClientError> {
        let raw = serde_json::to_value(&params).map_err(|e| PluginClientError::Protocol {
            message: format!("serialize invoke_hook params: {e}"),
        })?;
        let result = self
            .transport
            .send_request(METHOD_INVOKE_HOOK, raw, self.call_timeout)
            .await?;
        serde_json::from_value(result).map_err(|e| PluginClientError::Protocol {
            message: format!("deserialize invoke_hook result: {e}"),
        })
    }

    pub async fn invoke_skill(
        &self,
        params: InvokeSkillParams,
    ) -> Result<InvokeSkillResult, PluginClientError> {
        let raw = serde_json::to_value(&params).map_err(|e| PluginClientError::Protocol {
            message: format!("serialize invoke_skill params: {e}"),
        })?;
        let result = self
            .transport
            .send_request(METHOD_INVOKE_SKILL, raw, self.call_timeout)
            .await?;
        serde_json::from_value(result).map_err(|e| PluginClientError::Protocol {
            message: format!("deserialize invoke_skill result: {e}"),
        })
    }

    pub async fn invoke_cli(
        &self,
        params: InvokeCliParams,
    ) -> Result<InvokeCliResult, PluginClientError> {
        let raw = serde_json::to_value(&params).map_err(|e| PluginClientError::Protocol {
            message: format!("serialize invoke_cli params: {e}"),
        })?;
        let result = self
            .transport
            .send_request(METHOD_INVOKE_CLI, raw, self.call_timeout)
            .await?;
        serde_json::from_value(result).map_err(|e| PluginClientError::Protocol {
            message: format!("deserialize invoke_cli result: {e}"),
        })
    }

    pub async fn shutdown(&mut self) -> Result<(), PluginClientError> {
        if let Some(handle) = self.heartbeat_handle.take() {
            handle.abort();
            let _ = handle.await;
        }
        let _ = self.health_tx.send(PluginHealth::Stopped);
        self.transport.request_shutdown().await?;
        Ok(())
    }

    fn start_heartbeat(&mut self, interval: Duration) {
        let transport = Arc::clone(&self.transport);
        let health_tx = Arc::clone(&self.health_tx);
        let plugin_id = self.plugin_id.clone();
        let shutdown_notify = self.transport.shutdown_requested();
        let heartbeat_timeout = Duration::from_secs(10);

        self.heartbeat_handle = Some(tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(interval) => {}
                    _ = shutdown_notify.notified() => break,
                }

                if !transport.is_running() {
                    tracing::warn!(plugin_id = %plugin_id, "heartbeat: transport stopped");
                    let _ = health_tx.send(PluginHealth::Unhealthy {
                        reason: "transport stopped".into(),
                    });
                    break;
                }

                match tokio::time::timeout(heartbeat_timeout, transport.ping()).await {
                    Ok(Ok(())) => {
                        let _ = health_tx.send(PluginHealth::Healthy);
                    }
                    Ok(Err(e)) => {
                        tracing::warn!(
                            plugin_id = %plugin_id,
                            error = %e,
                            "heartbeat: ping failed",
                        );
                        let _ = health_tx.send(PluginHealth::Unhealthy {
                            reason: format!("ping failed: {e}"),
                        });
                    }
                    Err(_) => {
                        tracing::warn!(plugin_id = %plugin_id, "heartbeat: timeout");
                        let _ = health_tx.send(PluginHealth::Unhealthy {
                            reason: "heartbeat timeout".into(),
                        });
                    }
                }
            }
        }));
    }
}

#[doc(hidden)]
pub fn plugin_stdio_config_with_runtime_authority(
    stdio_config: StdioConfig,
    plugin_config: &PluginClientConfig,
) -> rua_core::Result<StdioConfig> {
    let authority = RuntimeAuthority::from_current_process(
        None,
        &plugin_config.working_dir,
        &plugin_config.runtime_cache_dir,
    );
    plugin_stdio_config_with_authority(stdio_config, &authority, plugin_config.env_remove.clone())
}

#[doc(hidden)]
pub fn plugin_stdio_config_with_authority(
    config: StdioConfig,
    authority: &RuntimeAuthority,
    env_remove: BTreeSet<String>,
) -> rua_core::Result<StdioConfig> {
    let plan = authority.plan(
        ProcessShape::Stdio,
        plugin_toolchain_intent(&config.command),
        RuntimeLaunchRequest {
            args: config.args.clone(),
            env: config.env.clone(),
            cwd: config.cwd.clone(),
            env_remove,
            ..Default::default()
        },
    );

    let process = plan.into_process()?;
    Ok(StdioConfig {
        command: process.command.to_string_lossy().into_owned(),
        args: process.args,
        env: process.env,
        cwd: process.cwd,
        ..config
    })
}

fn plugin_toolchain_intent(command: &str) -> ToolchainIntent {
    let command = command.trim();
    if std::path::Path::new(command).components().count() > 1 {
        return ToolchainIntent::GenericCommand {
            command: command.to_string(),
        };
    }

    let name = command.to_ascii_lowercase();
    let name = name.trim_end_matches(".cmd").trim_end_matches(".bat");
    match name {
        "python" | "python3" => ToolchainIntent::Python,
        "uv" => ToolchainIntent::Uv,
        "node" => ToolchainIntent::Node,
        "npm" => ToolchainIntent::Npm,
        "npx" => ToolchainIntent::Npx,
        _ => ToolchainIntent::GenericCommand {
            command: command.to_string(),
        },
    }
}

impl Drop for PluginClient {
    fn drop(&mut self) {
        if let Some(handle) = self.heartbeat_handle.take() {
            handle.abort();
        }
        let _ = self.health_tx.send(PluginHealth::Stopped);
    }
}
