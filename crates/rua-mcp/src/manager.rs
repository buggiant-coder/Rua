use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, error, info, warn};

use rua_core::runtime_authority::{
    ProcessShape, RuntimeAuthority, RuntimeLaunchRequest, ToolchainIntent,
};

const MAX_RETRIES: u32 = 3;
const RETRY_BASE_DELAY_MS: u64 = 200;

const MAX_CONNECT_RETRIES: u32 = 3;
const CONNECT_RETRY_DELAY: Duration = Duration::from_secs(1);

const FIRE_FORGET_RETRY_INTERVAL: Duration = Duration::from_secs(30);
const FIRE_FORGET_MAX_ATTEMPTS: u32 = 3;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", content = "error")]
pub enum McpClientStatus {
    Connecting,
    Ready,
    Failed(String),
}

use mcp_manager::client::McpClient;
use mcp_manager::error::{McpError, McpResult};
use mcp_manager::manager::McpManager;
use mcp_manager::status::{McpConnectionState, McpServerStatus};
use mcp_manager::types::{
    AutoConnectPolicy, McpCallContext, McpCallResult, McpConfig, McpReloadReport, McpServerConfig,
    McpServerId, McpShutdownReport, McpToolInfo, McpTransportConfig, ToolFilter,
};

use crate::config::McpConfigLoader;
use crate::mobile_policy::{McpMobilePolicy, MobilePolicyDecision};
use crate::transports::http::{HttpConfig, HttpMcpClient};
use crate::transports::sse::{SseConfig, SseMcpClient};
use crate::transports::stdio::{StdioConfig, StdioMcpClient};

pub struct ServerEntry {
    pub config: McpServerConfig,
    pub client: Box<dyn McpClient>,
    pub stop: tokio_util::sync::CancellationToken,
    pub status: Arc<RwLock<McpClientStatus>>,
}

pub struct McpManagerImpl {
    pub servers: Arc<RwLock<BTreeMap<McpServerId, ServerEntry>>>,
    lifetime: tokio_util::sync::CancellationToken,
    current_config: Arc<RwLock<McpConfig>>,
    config_update: Mutex<()>,
    calls_suspended: std::sync::atomic::AtomicBool,
    unapplied_errors: RwLock<BTreeMap<McpServerId, String>>,
    bundled_browser: Option<crate::bundled_browser::BundledBrowser>,
    bundled_computer: Option<crate::bundled_computer::BundledComputer>,
    mobile_policy: McpMobilePolicy,

    startup_timeout: Duration,

    #[allow(dead_code)]
    call_timeout: Duration,

    retry_interval: Duration,
    runtime_workspace_dir: PathBuf,
    runtime_cache_dir: PathBuf,
    runtime_host_profile: Option<String>,
}

fn headers_with_oauth(
    headers: &BTreeMap<String, String>,
    config: &McpServerConfig,
) -> BTreeMap<String, String> {
    let mut result = headers.clone();
    if let Some(env_var) = config
        .extra
        .get("bearer_token_env_var")
        .and_then(serde_json::Value::as_str)
        .filter(|name| !name.is_empty())
    {
        match std::env::var(env_var) {
            Ok(token) if !token.is_empty() => {
                result.insert("Authorization".to_string(), format!("Bearer {token}"));
            }
            Ok(_) | Err(_) => {
                warn!(
                    server = %config.name,
                    env_var,
                    "MCP bearer-token environment variable is not set"
                );
            }
        }
    }
    if let Some(oauth) = &config.oauth {
        result.retain(|name, _| !name.eq_ignore_ascii_case("authorization"));
        if oauth.has_valid_access_token() {
            result.insert(
                "Authorization".to_string(),
                format!("Bearer {}", oauth.access_token),
            );
        } else if !oauth.access_token.is_empty() {
            warn!(
                server = %config.name,
                "OAuth token for MCP server is expired; skipping Authorization header"
            );
        }
    }
    result
}

#[doc(hidden)]
pub fn stdio_config_with_runtime_authority(
    config: StdioConfig,
    authority: &RuntimeAuthority,
) -> rua_core::Result<StdioConfig> {
    let plan = authority.plan(
        ProcessShape::Stdio,
        mcp_stdio_toolchain_intent(&config.command),
        RuntimeLaunchRequest {
            args: config.args.clone(),
            env: config.env.clone(),
            cwd: normalized_cwd(config.cwd.as_deref()),
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

fn normalized_cwd(cwd: Option<&Path>) -> Option<PathBuf> {
    cwd.filter(|path| !path.as_os_str().is_empty())
        .map(Path::to_path_buf)
}

fn mcp_stdio_toolchain_intent(command: &str) -> ToolchainIntent {
    let command = command.trim();
    if Path::new(command).components().count() > 1 {
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

impl McpManagerImpl {
    pub fn new(mobile_policy: McpMobilePolicy) -> Self {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let runtime_host_profile = if mobile_policy.profile().is_mobile() {
            Some("mobile-apk".to_string())
        } else {
            None
        };
        Self {
            servers: Arc::new(RwLock::new(BTreeMap::new())),
            lifetime: tokio_util::sync::CancellationToken::new(),
            current_config: Arc::new(RwLock::new(McpConfig::default())),
            config_update: Mutex::new(()),
            calls_suspended: std::sync::atomic::AtomicBool::new(false),
            unapplied_errors: RwLock::new(BTreeMap::new()),
            bundled_browser: None,
            bundled_computer: None,
            mobile_policy,
            startup_timeout: Duration::from_secs(30),
            call_timeout: Duration::from_secs(60),
            retry_interval: FIRE_FORGET_RETRY_INTERVAL,
            runtime_workspace_dir: cwd.clone(),
            runtime_cache_dir: rua_config::dirs::Paths::new(None).cache_dir,
            runtime_host_profile,
        }
    }

    pub fn with_startup_timeout(mut self, timeout: Duration) -> Self {
        self.startup_timeout = timeout;
        self
    }

    pub fn with_retry_interval(mut self, interval: Duration) -> Self {
        self.retry_interval = interval;
        self
    }

    pub fn with_runtime_dirs(
        mut self,
        workspace_dir: impl Into<PathBuf>,
        cache_dir: impl Into<PathBuf>,
    ) -> Self {
        self.runtime_workspace_dir = workspace_dir.into();
        self.runtime_cache_dir = cache_dir.into();
        self
    }

    pub fn with_bundled_browser(
        mut self,
        settings: crate::bundled_browser::BundledBrowser,
    ) -> Self {
        self.bundled_browser = Some(settings);
        self
    }

    pub fn with_bundled_computer(
        mut self,
        settings: crate::bundled_computer::BundledComputer,
    ) -> Self {
        self.bundled_computer = Some(settings);
        self
    }

    fn product_defaults(&self, config: McpConfig) -> McpConfig {
        let config = match &self.bundled_browser {
            Some(browser) => browser.current(&self.runtime_authority()).defaults(config),
            None => config,
        };
        match &self.bundled_computer {
            Some(computer) => computer.defaults(config),
            None => config,
        }
    }

    fn runtime_authority(&self) -> RuntimeAuthority {
        RuntimeAuthority::from_current_process(
            self.runtime_host_profile.as_deref(),
            &self.runtime_workspace_dir,
            &self.runtime_cache_dir,
        )
    }

    fn create_client(
        &self,
        id: &McpServerId,
        config: &McpServerConfig,
    ) -> Result<Box<dyn McpClient>, String> {
        if id.as_str() == crate::bundled_computer::SERVER_ID
            && crate::bundled_computer::is_product_computer(config)
        {
            if let Some(settings) = &self.bundled_computer {
                return Ok(Box::new(
                    crate::bundled_computer::BundledComputerClient::new(settings.clone()),
                ));
            }
        }
        if id.as_str() == crate::bundled_browser::SERVER_ID
            && crate::bundled_browser::is_product_browser(config)
        {
            if let Some(settings) = &self.bundled_browser {
                return Ok(Box::new(crate::bundled_browser::BundledBrowserClient::new(
                    settings.current(&self.runtime_authority()),
                )));
            }
        }
        Ok(match &config.transport {
            McpTransportConfig::Stdio {
                command,
                args,
                env,
                cwd,
                use_proot: _,
            } => {
                let stdio_config = StdioConfig {
                    server_key: id.to_string(),
                    server_name: config.name.clone(),
                    command: command.clone(),
                    args: args.clone(),
                    env: env.clone(),
                    cwd: cwd.clone(),
                    startup_timeout: self.startup_timeout,
                    call_timeout: Duration::from_secs(config.call_timeout_secs),
                    stderr_log_prefix: None,
                };
                let authority = self.runtime_authority();
                // A disabled configuration is portable stored intent, not an admitted launch.
                // Enabling it reloads the client and validates local executable material.
                let stdio_config = if config.enabled {
                    stdio_config_with_runtime_authority(stdio_config, &authority)
                        .map_err(|error| error.to_string())?
                } else {
                    stdio_config
                };
                let client = StdioMcpClient::new(stdio_config);
                Box::new(
                    if authority.inventory().default_external_runtime
                        == rua_core::runtime_layout::RuntimeKind::LinuxCompat
                    {
                        client.with_isolated_env()
                    } else {
                        client
                    },
                )
            }
            McpTransportConfig::StreamableHttp {
                url,
                headers,
                timeout_secs,
            } => {
                let allow_private_ip = config
                    .extra
                    .get("allow_private_ip")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                Box::new(HttpMcpClient::new(HttpConfig {
                    server_key: id.to_string(),
                    server_name: config.name.clone(),
                    url: url.clone(),
                    headers: headers_with_oauth(headers, config),
                    timeout: Duration::from_secs(*timeout_secs),
                    call_timeout: Duration::from_secs(config.call_timeout_secs),
                    allow_private_ip,
                }))
            }
            McpTransportConfig::Sse {
                url,
                headers,
                reconnect,
            } => {
                let allow_private_ip = config
                    .extra
                    .get("allow_private_ip")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                Box::new(SseMcpClient::new(SseConfig {
                    server_key: id.to_string(),
                    server_name: config.name.clone(),
                    url: url.clone(),
                    headers: headers_with_oauth(headers, config),
                    reconnect: reconnect.clone(),
                    call_timeout: Duration::from_secs(config.call_timeout_secs),
                    allow_private_ip,
                }))
            }
        })
    }

    async fn add_server(&self, id: &McpServerId, config: &McpServerConfig) -> Result<bool, String> {
        let client = self.create_client(id, config)?;

        let should_connect = if config.enabled {
            let decision = self.mobile_policy.check_auto_connect(id, config);
            match decision {
                MobilePolicyDecision::Allow => !matches!(
                    config.auto_connect,
                    AutoConnectPolicy::Manual | AutoConnectPolicy::LazyOnFirstUse
                ),
                MobilePolicyDecision::DeferToLazy { reason } => {
                    info!(server = %id, %reason, "deferring MCP auto-connect on mobile");
                    false
                }
                MobilePolicyDecision::Deny { reason } => {
                    info!(server = %id, %reason, "MCP server denied on mobile");
                    false
                }
            }
        } else {
            false
        };

        let initial_status = if should_connect {
            McpClientStatus::Connecting
        } else {
            McpClientStatus::Ready
        };
        let status = Arc::new(RwLock::new(initial_status));

        let stop = self.lifetime.child_token();
        let mut servers = self.servers.write().await;
        servers.insert(
            id.clone(),
            ServerEntry {
                config: config.clone(),
                client,
                stop: stop.clone(),
                status: Arc::clone(&status),
            },
        );

        if should_connect {
            let servers_ref = Arc::clone(&self.servers);
            let server_id = id.clone();
            let status_ref = Arc::clone(&status);
            let retry_interval = self.retry_interval;

            tokio::spawn(async move {
                tokio::select! {
                    biased;
                    _=stop.cancelled()=>{},
                    _=Self::connect_with_retry(servers_ref, server_id, status_ref, retry_interval)=>{},
                }
            });
        }

        Ok(should_connect)
    }

    async fn connect_with_retry(
        servers: Arc<RwLock<BTreeMap<McpServerId, ServerEntry>>>,
        server_id: McpServerId,
        status: Arc<RwLock<McpClientStatus>>,
        retry_interval: Duration,
    ) {
        let mut attempt: u32 = 0;
        loop {
            attempt += 1;
            let connect_result = {
                let guard = servers.read().await;
                match guard.get(&server_id) {
                    Some(entry) => Some(entry.client.connect().await),
                    None => None,
                }
            };

            match connect_result {
                None => {
                    debug!(server = %server_id, "server removed during connect_with_retry, stopping");
                    return;
                }
                Some(Ok(())) => {
                    info!(server = %server_id, attempt, "MCP server connected (fire-and-forget)");
                    *status.write().await = McpClientStatus::Ready;
                    return;
                }
                Some(Err(e)) if attempt >= FIRE_FORGET_MAX_ATTEMPTS => {
                    info!(
                        server = %server_id,
                        attempts = attempt,
                        error = %e,
                        "MCP fire-and-forget gave up after max attempts (toggle client to retry)"
                    );
                    *status.write().await =
                        McpClientStatus::Failed(format!("after {attempt} attempts: {e}"));
                    return;
                }
                Some(Err(e)) => {
                    warn!(
                        server = %server_id,
                        attempt,
                        max = FIRE_FORGET_MAX_ATTEMPTS,
                        error = %e,
                        "fire-and-forget connect failed, retrying in 30s"
                    );
                    *status.write().await = McpClientStatus::Failed(e.to_string());
                    tokio::time::sleep(retry_interval).await;
                    *status.write().await = McpClientStatus::Connecting;
                }
            }
        }
    }

    async fn remove_server(&self, id: &McpServerId) -> Result<(), String> {
        let servers = self.servers.read().await;
        if let Some(entry) = servers.get(id) {
            entry.stop.cancel();
            if let Err(e) = entry.client.shutdown().await {
                warn!(server = %id, error = %e, "error shutting down MCP server");
                return Err(e.to_string());
            }
            info!(server = %id, "MCP server removed and shut down");
        }
        drop(servers);
        self.servers.write().await.remove(id);

        Ok(())
    }
}

#[async_trait]
impl McpManager for McpManagerImpl {
    async fn suspend_new_calls(&self) -> McpResult<()> {
        self.calls_suspended
            .store(true, std::sync::atomic::Ordering::Release);
        Ok(())
    }

    async fn load_config(&self, config: McpConfig) -> McpResult<McpReloadReport> {
        self.reload(config).await
    }

    async fn configuration(&self) -> McpResult<McpConfig> {
        Ok(self.current_config.read().await.clone())
    }

    async fn list_servers(&self) -> McpResult<Vec<McpServerStatus>> {
        let unapplied = self.unapplied_errors.read().await.clone();
        let servers = self.servers.read().await;
        let mut statuses: Vec<McpServerStatus> = Vec::with_capacity(servers.len());

        for (id, entry) in servers.iter() {
            let mut server_status = entry.client.status();
            server_status.key = id.to_string();

            let client_status = entry.status.read().await;
            match &*client_status {
                _ if (self.bundled_browser.is_some()
                    && crate::bundled_browser::is_product_browser(&entry.config))
                    || (self.bundled_computer.is_some()
                        && crate::bundled_computer::is_product_computer(&entry.config)) => {}
                McpClientStatus::Connecting => {
                    server_status.state = McpConnectionState::Connecting;
                }
                McpClientStatus::Failed(err) => {
                    server_status.state = McpConnectionState::Failed;
                    server_status.last_error = Some(err.clone());
                }
                McpClientStatus::Ready => {}
            }

            if self.mobile_policy.profile().is_mobile() {
                let decision = self
                    .mobile_policy
                    .check_on_demand_start(id, &entry.config, 0);
                if let MobilePolicyDecision::Deny { reason } = decision {
                    server_status.mobile_unavailable_reason = Some(reason);
                }
            }

            server_status.enabled = entry.config.enabled;
            if !entry.config.enabled {
                server_status.state = McpConnectionState::Disabled;
            }
            if unapplied.contains_key(id)
                || self
                    .calls_suspended
                    .load(std::sync::atomic::Ordering::Acquire)
            {
                server_status.state = McpConnectionState::Failed;
                server_status.last_error =
                    Some(unapplied.get(id).cloned().unwrap_or_else(|| {
                        "Saved MCP configuration has not been applied".to_string()
                    }));
            }

            statuses.push(server_status);
        }

        drop(servers);
        let config = self.current_config.read().await;
        for (id, message) in &unapplied {
            if statuses.iter().any(|status| status.key == id.as_str()) {
                continue;
            }
            if let Some(entry) = config.clients.get(id) {
                let mut status = McpServerStatus::not_connected(
                    id.to_string(),
                    entry.name.clone(),
                    entry.transport.transport_type(),
                );
                status.enabled = entry.enabled;
                status.state = McpConnectionState::Failed;
                status.last_error = Some(message.clone());
                statuses.push(status);
            }
        }
        statuses.sort_by(|a, b| a.key.cmp(&b.key));
        Ok(statuses)
    }

    async fn list_tools(&self, filter: ToolFilter) -> McpResult<Vec<McpToolInfo>> {
        if self
            .calls_suspended
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return Err(McpError::Internal {
                message: "Saved MCP configuration is unavailable; new calls are suspended".into(),
            });
        }
        let unapplied = self.unapplied_errors.read().await.clone();
        if let Some(message) = filter.server_id.as_ref().and_then(|id| unapplied.get(id)) {
            return Err(McpError::Internal {
                message: message.clone(),
            });
        }
        let servers = self.servers.read().await;
        let mut all_tools = Vec::new();

        for (id, entry) in servers.iter() {
            if unapplied.contains_key(id) {
                if filter.server_id.as_ref() == Some(id) {
                    return Err(McpError::Internal {
                        message: "Saved MCP configuration has not been applied".to_string(),
                    });
                }
                continue;
            }
            if let Some(ref filter_id) = filter.server_id {
                if id != filter_id {
                    continue;
                }
            }

            if !entry.config.enabled {
                continue;
            }

            {
                let client_status = entry.status.read().await;
                if *client_status == McpClientStatus::Connecting {
                    if filter.server_id.is_none() {
                        continue;
                    }
                    drop(client_status);

                    let deadline = tokio::time::Instant::now() + self.startup_timeout;
                    loop {
                        if *entry.status.read().await != McpClientStatus::Connecting {
                            break;
                        }
                        if tokio::time::Instant::now() >= deadline {
                            return Err(McpError::CallTimeout {
                                server: id.to_string(),
                                tool: "tools/list".into(),
                                timeout_secs: self.startup_timeout.as_secs(),
                            });
                        }
                        tokio::select! {
                            biased;
                            _=entry.stop.cancelled()=>return Err(McpError::Cancelled {server:id.to_string()}),
                            _=tokio::time::sleep(Duration::from_millis(50))=>{},
                        }
                    }
                }
            }

            if !entry.client.status().state.is_available() {
                if matches!(entry.config.auto_connect, AutoConnectPolicy::Manual) {
                    continue;
                }
                debug!(server = %id, "lazy-connecting MCP server for tool listing");
                let connected = tokio::select! {
                    biased;
                    _=entry.stop.cancelled()=>return Err(McpError::Cancelled {server:id.to_string()}),
                    result=entry.client.connect()=>result,
                };
                if let Err(e) = connected {
                    warn!(server = %id, error = %e, "lazy connect failed for tool listing");
                    if filter.server_id.is_some() {
                        *entry.status.write().await = McpClientStatus::Failed(e.to_string());
                        return Err(e);
                    }
                    continue;
                }
                *entry.status.write().await = McpClientStatus::Ready;
            }

            let listed = tokio::select! {
                biased;
                _=entry.stop.cancelled()=>return Err(McpError::Cancelled {server:id.to_string()}),
                result=entry.client.list_tools()=>result,
            };
            match listed {
                Ok(tools) => {
                    for tool in tools {
                        if !filter.include_disabled && entry.config.is_tool_disabled(&tool.name) {
                            continue;
                        }
                        if let Some(ref prefix) = filter.name_prefix {
                            if !tool.name.starts_with(prefix.as_str()) {
                                continue;
                            }
                        }
                        all_tools.push(tool);
                    }
                }
                Err(e) => {
                    warn!(server = %id, error = %e, "failed to list tools from MCP server");
                    if filter.server_id.is_some() {
                        return Err(e);
                    }
                }
            }
        }

        if filter.server_id.is_some() && all_tools.is_empty() {
            if let Some(server_id) = filter.server_id {
                if !servers.contains_key(&server_id) {
                    return Err(McpError::ServerNotFound {
                        server: server_id.to_string(),
                    });
                }
            }
        }

        Ok(all_tools)
    }

    async fn call_tool(
        &self,
        server: &McpServerId,
        tool: &str,
        args: serde_json::Value,
        ctx: McpCallContext,
    ) -> McpResult<McpCallResult> {
        if self
            .calls_suspended
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return Err(McpError::Internal {
                message: "Saved MCP configuration is unavailable; new calls are suspended".into(),
            });
        }
        if let Some(message) = self.unapplied_errors.read().await.get(server) {
            return Err(McpError::Internal {
                message: message.clone(),
            });
        }
        let servers = tokio::select! {
            biased;
            _=self.lifetime.cancelled()=>return Err(McpError::Cancelled {server:server.to_string()}),
            servers=self.servers.read()=>servers,
        };

        let entry = servers
            .get(server)
            .ok_or_else(|| McpError::ServerNotFound {
                server: server.to_string(),
            })?;

        // Top-level enablement belongs to the effective Chat tool snapshot,
        // not the transport. Composite calls must also respect leaf controls.
        if tool == crate::computer_control::INTERACT
            && self.bundled_computer.is_some()
            && crate::bundled_computer::is_product_computer(&entry.config)
        {
            let captured = ctx
                .metadata
                .get(rua_core::tool::EFFECTIVE_DISABLED_TOOLS_METADATA_KEY)
                .map(|value| serde_json::from_value::<Vec<String>>(value.clone()))
                .transpose()
                .map_err(|error| McpError::ConfigInvalid {
                    message: format!("Invalid captured tool disablements: {error}"),
                })?;
            if captured.is_none() && ctx.metadata.contains_key("chat_configuration_epoch") {
                return Err(McpError::PermissionDenied {
                    message: "Computer input requires captured Chat tool restrictions".into(),
                });
            }
            for (name, _) in crate::computer_control::action_calls(&args)? {
                let disabled = match &captured {
                    Some(disabled) => disabled.contains(&format!("mcp__{server}__{name}")),
                    None => entry.config.is_tool_disabled(&name),
                };
                if disabled {
                    return Err(McpError::PermissionDenied {
                        message: format!("Computer sequence action '{name}' is disabled"),
                    });
                }
            }
        }
        if ctx.cancel.as_ref().is_some_and(|rx| *rx.borrow()) {
            return Err(McpError::Cancelled {
                server: server.to_string(),
            });
        }
        if !entry.config.enabled {
            return Err(McpError::PermissionDenied {
                message: format!("MCP server '{server}' is disabled"),
            });
        }

        if self.mobile_policy.profile().is_mobile() {
            let subprocess_count = servers
                .values()
                .filter(|e| {
                    e.config.transport.is_subprocess() && e.client.status().state.is_available()
                })
                .count();

            let decision =
                self.mobile_policy
                    .check_on_demand_start(server, &entry.config, subprocess_count);

            if let MobilePolicyDecision::Deny { reason } = decision {
                return Err(McpError::MobilePolicyDenied {
                    server: server.to_string(),
                    reason,
                });
            }
        }

        if (self.bundled_browser.is_some()
            && crate::bundled_browser::is_product_browser(&entry.config))
            || (self.bundled_computer.is_some()
                && crate::bundled_computer::is_product_computer(&entry.config))
        {
            // The client owns one total budget including connection/resource waits.
            // Lifecycle cancellation drops its owned in-flight guard; never replay.
            return tokio::select! {
                biased;
                _=entry.stop.cancelled()=>Err(McpError::Cancelled {server:server.to_string()}),
                result=entry.client.call_tool(tool,args,ctx)=>result,
            };
        }
        tokio::select! {
            biased;
            _=entry.stop.cancelled()=>Err(McpError::Cancelled {server:server.to_string()}),
            result=async {
        if !entry.client.status().state.is_available() {
            entry.client.connect().await?;
        }
        let mut last_err = None;
        for attempt in 0..=MAX_RETRIES {
            match entry
                .client
                .call_tool(tool, args.clone(), ctx.clone())
                .await
            {
                Ok(result) => {
                    return Ok(result);
                }
                // A stdio failure may arrive after a side effect. Reconnect only for a new call.
                Err(e) if e.is_retryable() && !entry.config.transport.is_subprocess() && attempt < MAX_RETRIES => {
                    let delay = Duration::from_millis(RETRY_BASE_DELAY_MS * 2u64.pow(attempt));
                    warn!(
                        server = %server,
                        tool = %tool,
                        attempt = attempt + 1,
                        delay_ms = delay.as_millis() as u64,
                        error = %e,
                        "retrying MCP tool call after transient error"
                    );
                    tokio::time::sleep(delay).await;

                    if matches!(
                        e,
                        McpError::TransportClosed { .. } | McpError::ServerCrashed { .. }
                    ) {
                        let _ = entry.client.shutdown().await;
                        if let Err(reconnect_err) = entry.client.connect().await {
                            warn!(server = %server, error = %reconnect_err, "reconnect failed during retry");
                        }
                    }

                    last_err = Some(e);
                }
                Err(e) => {
                    return Err(e);
                }
            }
        }

        Err(last_err.unwrap_or_else(|| McpError::Internal {
            message: "retry loop exhausted without error".into(),
        }))
            } => result,
        }
    }

    async fn reload(&self, new_config: McpConfig) -> McpResult<McpReloadReport> {
        let _update = self.config_update.lock().await;
        let new_config = self.product_defaults(new_config);
        let old_config = {
            let current = self.current_config.read().await;
            current.clone()
        };

        let (mut added, mut removed, mut updated, mut unchanged) =
            McpConfigLoader::diff_configs(&old_config, &new_config);
        let unapplied = self.unapplied_errors.read().await.clone();

        let servers = self.servers.read().await;
        for id in unapplied.keys() {
            if !new_config.clients.contains_key(id)
                && servers.contains_key(id)
                && !removed.contains(id)
            {
                removed.push(id.clone());
            }
        }
        let mut retry = Vec::new();
        for id in &unchanged {
            match servers.get(id) {
                None => added.push(id.clone()),
                Some(entry)
                    if unapplied.contains_key(id)
                        || matches!(*entry.status.read().await, McpClientStatus::Failed(_)) =>
                {
                    updated.push(id.clone());
                }
                Some(_) => continue,
            }
            retry.push(id.clone());
        }
        drop(servers);
        unchanged.retain(|id| !retry.contains(id));

        let mut report = McpReloadReport {
            added: added.clone(),
            removed: removed.clone(),
            updated: updated.clone(),
            unchanged: unchanged.clone(),
            errors: BTreeMap::new(),
        };
        self.unapplied_errors.write().await.extend(
            added
                .iter()
                .chain(&updated)
                .chain(&removed)
                .cloned()
                .map(|id| {
                    (
                        id,
                        "Saved MCP configuration has not been applied".to_string(),
                    )
                }),
        );

        for id in &removed {
            if let Err(e) = self.remove_server(id).await {
                report.errors.insert(id.clone(), e);
            }
        }

        for id in &updated {
            if let Err(e) = self.remove_server(id).await {
                report
                    .errors
                    .insert(id.clone(), format!("shutdown error: {e}"));
                continue;
            }
            if let Some(config) = new_config.clients.get(id) {
                if let Err(e) = self.add_server(id, config).await {
                    report
                        .errors
                        .insert(id.clone(), format!("restart error: {e}"));
                }
            }
        }

        for id in &added {
            if let Some(config) = new_config.clients.get(id) {
                if let Err(e) = self.add_server(id, config).await {
                    report.errors.insert(id.clone(), e);
                }
            }
        }

        let mut current = self.current_config.write().await;
        *current = new_config;
        self.calls_suspended
            .store(false, std::sync::atomic::Ordering::Release);
        {
            let mut unapplied = self.unapplied_errors.write().await;
            for id in added.iter().chain(&updated).chain(&removed) {
                if let Some(message) = report.errors.get(id) {
                    unapplied.insert(id.clone(), message.clone());
                } else {
                    unapplied.remove(id);
                }
            }
        }

        info!(
            added = report.added.len(),
            removed = report.removed.len(),
            updated = report.updated.len(),
            unchanged = report.unchanged.len(),
            errors = report.errors.len(),
            "MCP config reloaded"
        );

        Ok(report)
    }

    async fn replace_client(
        &self,
        server_id: &McpServerId,
        config: McpServerConfig,
    ) -> McpResult<()> {
        let _update = self.config_update.lock().await;
        let mut replacement = McpConfig::default();
        replacement.clients.insert(server_id.clone(), config);
        let config = self
            .product_defaults(replacement)
            .clients
            .remove(server_id)
            .expect("replacement contains the requested server");
        self.remove_server(server_id)
            .await
            .map_err(|e| McpError::Internal {
                message: format!("failed to remove old server '{}': {}", server_id, e),
            })?;

        self.add_server(server_id, &config)
            .await
            .map_err(|e| McpError::Internal {
                message: format!("failed to add replacement server '{}': {}", server_id, e),
            })?;

        {
            let mut current = self.current_config.write().await;
            current.clients.insert(server_id.clone(), config);
        }

        self.unapplied_errors.write().await.remove(server_id);
        info!(server = %server_id, "MCP server client replaced");
        Ok(())
    }

    async fn invalidate_tool_cache(&self, server: &McpServerId) -> McpResult<()> {
        let servers = self.servers.read().await;
        let entry = servers
            .get(server)
            .ok_or_else(|| McpError::ServerNotFound {
                server: server.to_string(),
            })?;
        entry.client.invalidate_tool_cache().await;
        debug!(server = %server, "invalidated MCP tool cache");
        Ok(())
    }

    async fn connect_server(&self, server: &McpServerId) -> McpResult<()> {
        debug!(server = %server, "connecting to MCP server");
        if let Some(message) = self.unapplied_errors.read().await.get(server) {
            return Err(McpError::Internal {
                message: message.clone(),
            });
        }
        let servers = self.servers.read().await;

        let entry = servers
            .get(server)
            .ok_or_else(|| McpError::ServerNotFound {
                server: server.to_string(),
            })?;

        if !entry.config.enabled {
            return Err(McpError::PermissionDenied {
                message: format!("MCP server '{server}' is disabled"),
            });
        }

        if self.mobile_policy.profile().is_mobile() {
            let subprocess_count = servers
                .values()
                .filter(|e| {
                    e.config.transport.is_subprocess() && e.client.status().state.is_available()
                })
                .count();

            let decision =
                self.mobile_policy
                    .check_on_demand_start(server, &entry.config, subprocess_count);

            if let MobilePolicyDecision::Deny { reason } = decision {
                return Err(McpError::MobilePolicyDenied {
                    server: server.to_string(),
                    reason,
                });
            }
        }

        let mut last_err = None;
        for attempt in 1..=MAX_CONNECT_RETRIES {
            let connected = tokio::select! {
                biased;
                _=entry.stop.cancelled()=>return Err(McpError::Cancelled {server:server.to_string()}),
                result=entry.client.connect()=>result,
            };
            match connected {
                Ok(()) => {
                    *entry.status.write().await = McpClientStatus::Ready;
                    debug!(server = %server, "MCP server connected successfully");
                    return Ok(());
                }
                Err(e) if e.is_config_error() => return Err(e),
                Err(e) => {
                    warn!(
                        server = %server,
                        attempt = attempt,
                        max = MAX_CONNECT_RETRIES,
                        error = %e,
                        "MCP server {} connect failed (attempt {}/{}): {}",
                        server, attempt, MAX_CONNECT_RETRIES, e
                    );
                    last_err = Some(e);
                    if attempt < MAX_CONNECT_RETRIES {
                        tokio::select! {
                            biased;
                            _=entry.stop.cancelled()=>return Err(McpError::Cancelled {server:server.to_string()}),
                            _=tokio::time::sleep(CONNECT_RETRY_DELAY)=>{},
                        }
                    }
                }
            }
        }

        Err(last_err.unwrap_or_else(|| McpError::Internal {
            message: "connect retry loop exhausted without error".into(),
        }))
    }

    async fn disconnect_server(&self, server: &McpServerId) -> McpResult<()> {
        debug!(server = %server, "disconnecting MCP server");
        let _update = self.config_update.lock().await;
        {
            let servers = self.servers.read().await;
            let entry = servers
                .get(server)
                .ok_or_else(|| McpError::ServerNotFound {
                    server: server.to_string(),
                })?;
            // Wake in-flight work before waiting for its read lock. Dropping an
            // input future records its unknown outcome instead of replaying it.
            entry.stop.cancel();
        }
        let mut servers = self.servers.write().await;
        let entry = servers
            .get_mut(server)
            .ok_or_else(|| McpError::ServerNotFound {
                server: server.to_string(),
            })?;
        let result = entry.client.shutdown().await;
        entry.stop = self.lifetime.child_token();
        *entry.status.write().await = McpClientStatus::Ready;
        result
    }

    async fn shutdown_all(&self) -> McpResult<McpShutdownReport> {
        self.lifetime.cancel();
        let _update = self.config_update.lock().await;
        *self.current_config.write().await = McpConfig::default();
        let mut report = McpShutdownReport::default();
        for entry in self.servers.read().await.values() {
            entry.stop.cancel();
        }
        let mut servers = self.servers.write().await;

        let ids: Vec<McpServerId> = servers.keys().cloned().collect();

        for id in ids {
            if let Some(entry) = servers.remove(&id) {
                debug!(server = %id, "shutting down MCP server");
                match entry.client.shutdown().await {
                    Ok(()) => {
                        debug!(server = %id, "MCP server shut down");
                        report.success.push(id);
                    }
                    Err(e) => {
                        error!(server = %id, error = %e, "error shutting down MCP server");
                        report.errors.insert(id, e.to_string());
                    }
                }
            }
        }

        info!(
            success = report.success.len(),
            errors = report.errors.len(),
            "all MCP servers shut down"
        );

        Ok(report)
    }
}

impl Drop for McpManagerImpl {
    fn drop(&mut self) {
        self.lifetime.cancel();
    }
}
