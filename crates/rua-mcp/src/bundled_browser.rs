//! Product-owned agent-browser integration (ADR-0054).
//! The driver is independent; Rua owns its identity, launch policy and lifecycle.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use mcp_manager::client::McpClient;
use mcp_manager::error::{McpError, McpResult};
use mcp_manager::status::{McpConnectionState, McpServerStatus};
use mcp_manager::types::{
    AutoConnectPolicy, McpCallContext, McpCallResult, McpConfig, McpContent, McpInitializeResult,
    McpServerConfig, McpServerId, McpToolInfo, McpTransportConfig,
};
use rua_core::runtime_authority::{RuntimeAuthority, RuntimeCapability};
use rua_core::ChatAddress;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

use crate::call_scope::CallScope;
use crate::tool_adapter::McpToolAdapter;
use crate::transports::stdio::{StdioConfig, StdioMcpClient};

pub const SERVER_ID: &str = "rua_browser";
pub const DRIVER_VERSION: &str = "0.39.4";
const CALL_TIMEOUT: Duration = Duration::from_secs(40);
const CLOSE_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Debug, Clone)]
pub struct BundledBrowser {
    pub driver: PathBuf,
    pub data_dir: PathBuf,
    pub browser: RuntimeCapability,
    pub preferences_path: Option<PathBuf>,
}

impl BundledBrowser {
    /// Resolve the installed driver beside Rua, with an explicit development override.
    /// Driver discovery never depends on the viewer or searches PATH.
    pub fn from_runtime(authority: &RuntimeAuthority, cache_dir: &Path) -> Option<Self> {
        if !matches!(
            authority.inventory().os_family.as_str(),
            "macos" | "windows"
        ) || authority.inventory().host_profile != "desktop"
        {
            return None;
        }
        let driver = std::env::var_os("RUA_BUNDLED_AGENT_BROWSER")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::current_exe()
                    .ok()
                    .and_then(|exe| exe.canonicalize().ok())
                    .map(|exe| {
                        exe.with_file_name(if cfg!(windows) {
                            "agent-browser.exe"
                        } else {
                            "agent-browser"
                        })
                    })
            })?;
        Some(
            Self {
                driver,
                data_dir: cache_dir.join("desktop-automation/browser"),
                browser: authority.system_chromium(),
                preferences_path: Some(rua_config::desktop_automation::preferences_path(cache_dir)),
            }
            .current(authority),
        )
    }

    pub fn current(&self, authority: &RuntimeAuthority) -> Self {
        let mut current = self.clone();
        if let Some(path) = &self.preferences_path {
            match rua_config::desktop_automation::DesktopAutomationPreferences::load(path) {
                Ok(preferences) => {
                    current.browser =
                        authority.selected_chromium(preferences.browser_path.as_deref())
                }
                Err(error) => {
                    current.browser.available = false;
                    current.browser.diagnostics =
                        vec![format!("Cannot read local browser preference: {error}")];
                }
            }
        }
        current
    }

    pub fn defaults(&self, mut config: McpConfig) -> McpConfig {
        let id = McpServerId::new(SERVER_ID);
        if config
            .clients
            .get(&id)
            .is_some_and(|entry| !is_product_browser(entry))
        {
            // A pre-existing third-party registration keeps its identity and transport.
            return config;
        }
        // This reserved product identity keeps user enablement and tool restrictions.
        // Resolved paths are runtime-only and never written to the portable MCP store.
        let entry = config.clients.entry(id).or_insert_with(|| McpServerConfig {
            name: "Browser Use".into(),
            description: "Rua-owned Chromium browser sessions".into(),
            enabled: true,
            transport: McpTransportConfig::Stdio {
                command: String::new(),
                args: vec![],
                env: BTreeMap::new(),
                cwd: None,
                use_proot: false,
            },
            auto_connect: AutoConnectPolicy::LazyOnFirstUse,
            call_timeout_secs: 60,
            max_concurrency: 1,
            oauth: None,
            disabled_tools: vec![],
            extra: BTreeMap::from([("rua_builtin".into(), json!("browser"))]),
        });
        entry.transport = McpTransportConfig::Stdio {
            command: self.driver.to_string_lossy().into_owned(),
            args: vec!["mcp".into(), "--tools".into(), "core".into()],
            env: BTreeMap::new(),
            cwd: None,
            use_proot: false,
        };
        entry.extra.insert(
            "rua_browser_revision".into(),
            json!(format!(
                "{:x}",
                Sha256::digest(serde_json::to_vec(&self.browser).expect("browser serializes"))
            )),
        );
        config
    }
}

pub fn is_product_browser(config: &McpServerConfig) -> bool {
    config.extra.get("rua_builtin").and_then(Value::as_str) == Some("browser")
}

struct BrowserState {
    sessions: BTreeSet<String>,
    transports: BTreeMap<String, StdioMcpClient>,
}

pub struct BundledBrowserClient {
    settings: BundledBrowser,
    transport: StdioMcpClient,
    verified: std::sync::atomic::AtomicBool,
    environment: BTreeMap<String, String>,
    socket_dir: PathBuf,
    state: Mutex<BrowserState>,
    fault: std::sync::RwLock<Option<String>>,
}

impl BundledBrowserClient {
    pub fn new(settings: BundledBrowser) -> Self {
        // macOS sockaddr_un is only 104 bytes; its normal TMPDIR is too long.
        let temp = if cfg!(target_os = "macos") {
            PathBuf::from("/tmp")
        } else {
            std::env::temp_dir()
        };
        let socket_dir = temp.join(format!(
            "rua-b-{}",
            &uuid::Uuid::new_v4().simple().to_string()[..16]
        ));
        let mut environment: BTreeMap<String, String> = std::env::vars()
            .filter(|(key, _)| {
                matches!(
                    key.as_str(),
                    "HOME"
                        | "USERPROFILE"
                        | "SYSTEMROOT"
                        | "SystemRoot"
                        | "WINDIR"
                        | "TEMP"
                        | "TMP"
                        | "TMPDIR"
                        | "LANG"
                        | "LC_ALL"
                )
            })
            .collect();
        environment.insert(
            "AGENT_BROWSER_CONFIG".into(),
            socket_dir
                .join("driver.json")
                .to_string_lossy()
                .into_owned(),
        );
        environment.insert(
            "AGENT_BROWSER_SOCKET_DIR".into(),
            socket_dir.to_string_lossy().into_owned(),
        );
        environment.insert(
            "RUA_BROWSER_OWNER_FILE".into(),
            socket_dir.join("owner").to_string_lossy().into_owned(),
        );
        environment.insert(
            "RUA_BROWSER_OWNER_PID".into(),
            std::process::id().to_string(),
        );
        environment.insert("AGENT_BROWSER_NO_WEBMCP".into(), "1".into());
        environment.insert("AGENT_BROWSER_NO_AUTO_DIALOG".into(), "1".into());
        environment.insert("AGENT_BROWSER_IDLE_TIMEOUT_MS".into(), "900000".into());
        if let Some(path) = &settings.browser.path {
            environment.insert(
                "AGENT_BROWSER_EXECUTABLE_PATH".into(),
                path.to_string_lossy().into_owned(),
            );
        }
        let transport =
            Self::profile_transport(&settings, &environment, &socket_dir.join("probe-profile"));
        Self {
            settings,
            fault: std::sync::RwLock::new(None),
            transport,
            verified: std::sync::atomic::AtomicBool::new(false),
            environment,
            socket_dir,
            state: Mutex::new(BrowserState {
                sessions: BTreeSet::new(),
                transports: BTreeMap::new(),
            }),
        }
    }

    fn profile_transport(
        settings: &BundledBrowser,
        environment: &BTreeMap<String, String>,
        profile: &Path,
    ) -> StdioMcpClient {
        let mut environment = environment.clone();
        environment.insert(
            "AGENT_BROWSER_PROFILE".into(),
            profile.to_string_lossy().into_owned(),
        );
        StdioMcpClient::new(StdioConfig {
            server_key: SERVER_ID.into(),
            server_name: "Browser Use".into(),
            command: settings.driver.to_string_lossy().into_owned(),
            args: vec!["mcp".into(), "--tools".into(), "core".into()],
            env: environment.clone(),
            cwd: Some(settings.data_dir.clone()),
            startup_timeout: Duration::from_secs(10),
            call_timeout: CALL_TIMEOUT,
            stderr_log_prefix: Some("Browser Use".into()),
        })
        .with_isolated_env()
    }

    pub fn socket_dir(&self) -> &Path {
        &self.socket_dir
    }

    fn unavailable(&self) -> Option<String> {
        if let Ok(error) = std::env::var("RUA_DESKTOP_AUTOMATION_ERROR") {
            return Some(error);
        }
        if !rua_core::runtime_authority::browser::executable_file(&self.settings.driver) {
            return Some("Bundled agent-browser is missing or not executable. Repair the Rua Desktop installation.".into());
        }
        if !self.settings.browser.available {
            return Some(self.settings.browser.diagnostics.join("; "));
        }
        None
    }

    async fn connect_locked(&self) -> McpResult<()> {
        if let Some(message) = self.unavailable() {
            return Err(McpError::ConfigInvalid { message });
        }
        if self.transport.status().state.is_available()
            && self.verified.load(std::sync::atomic::Ordering::Acquire)
        {
            return Ok(());
        }
        self.verified
            .store(false, std::sync::atomic::Ordering::Release);
        private_directory(&self.settings.data_dir)?;
        private_directory(&self.socket_dir)?;
        rua_config::atomic_write(&self.socket_dir.join("driver.json"), b"{}\n")
            .map_err(io_error)?;
        let contract = tokio::time::timeout(
            Duration::from_secs(5),
            tokio::process::Command::new(&self.settings.driver)
                .env_clear()
                .envs(&self.environment)
                .arg("--rua-automation-contract")
                .stdin(std::process::Stdio::null())
                .kill_on_drop(true)
                .output(),
        )
        .await
        .map_err(|_| {
            io_error(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "driver contract probe timed out",
            ))
        })?
        .map_err(io_error)?;
        if !contract.status.success() || contract.stdout != b"rua-agent-browser/1\n" {
            return Err(McpError::ConfigInvalid {message:"Bundled browser adapter is missing or mismatched. Repair this Rua installation.".into()});
        }
        rua_config::atomic_write(&self.socket_dir.join("owner"), b"owned\n").map_err(io_error)?;
        self.transport.connect().await?;
        let init = self.transport.initialize_result().await?;
        if init.server_version != DRIVER_VERSION {
            self.transport.shutdown().await?;
            return Err(McpError::ConfigInvalid {
                message: format!(
                    "Expected agent-browser {DRIVER_VERSION}, received {}",
                    init.server_version
                ),
            });
        }
        self.close_session("probe").await?;
        let _ = std::fs::remove_file(self.socket_dir.join("probe.stop"));
        let probe_profile = self.socket_dir.join("probe-profile");
        let mut probe_guard = BrowserCall {
            stop_file: self.socket_dir.join("probe.stop"),
            settled: false,
        };
        let result = self
            .transport
            .call_tool(
                "agent_browser_open",
                json!({
                    "url":"about:blank", "session":"probe", "timeoutMs":10000,
                }),
                McpCallContext {
                    timeout: Some(Duration::from_secs(12)),
                    ..Default::default()
                },
            )
            .await;
        let closed = self.close_session("probe").await;
        probe_guard.settled = closed.is_ok();
        let _ = std::fs::remove_dir_all(probe_profile);
        match (result, closed) {
            (Ok(result), Ok(())) if !result.is_error => {
                self.verified
                    .store(true, std::sync::atomic::Ordering::Release);
                Ok(())
            }
            (result, cleanup) => {
                let _ = self.transport.shutdown().await;
                let message = match result {
                    Ok(result) => result.text(),
                    Err(error) => error.to_string(),
                };
                let diagnostic = if message.contains("-32601")
                    || message.to_ascii_lowercase().contains("method not found")
                    || message.to_ascii_lowercase().contains("unknown method")
                    || message.to_ascii_lowercase().contains("protocol error")
                {
                    "Browser is incompatible with the required Chromium CDP protocol; select an up-to-date Chromium browser"
                } else {
                    "Selected browser could not start a dedicated Chromium session; check that it runs normally, then select its executable or recheck"
                };
                Err(McpError::ConfigInvalid {
                    message: format!("{diagnostic}: {message}; cleanup: {cleanup:?}"),
                })
            }
        }
    }

    async fn close_session(&self, session: &str) -> McpResult<()> {
        rua_config::atomic_write(&self.socket_dir.join(format!("{session}.stop")), b"stop\n")
            .map_err(io_error)?;
        let deadline = tokio::time::Instant::now() + CLOSE_TIMEOUT;
        // The patched daemon cancels its command before releasing the browser.
        // Wait for acknowledgement; never kill a process named by an untrusted PID file.
        let pid_file = self.socket_dir.join(format!("{session}.pid"));
        while pid_file.exists() {
            if let Some(pid) = std::fs::read_to_string(&pid_file)
                .ok()
                .and_then(|p| p.trim().parse::<u32>().ok())
                .filter(|p| *p > 0)
            {
                if process_exited(pid) {
                    let _ = std::fs::remove_file(&pid_file);
                    let _ = std::fs::remove_file(self.socket_dir.join(format!("{session}.sock")));
                    let _ = std::fs::remove_file(self.socket_dir.join(format!("{session}.port")));
                    break;
                }
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(McpError::ProtocolError {
                    server: SERVER_ID.into(),
                    message: "Owned browser did not acknowledge Stop; session remains blocked"
                        .into(),
                });
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        Ok(())
    }
}

fn process_exited(pid: u32) -> bool {
    #[cfg(unix)]
    {
        i32::try_from(pid).is_ok_and(|pid| unsafe { libc::kill(pid, 0) } != 0
            && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH))
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::{
            Foundation::{CloseHandle, GetLastError, ERROR_INVALID_PARAMETER, STILL_ACTIVE},
            System::Threading::{
                GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
            },
        };
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                return GetLastError() == ERROR_INVALID_PARAMETER;
            }
            let mut code = 0;
            let exited = GetExitCodeProcess(handle, &mut code) != 0 && code != STILL_ACTIVE as u32;
            CloseHandle(handle);
            exited
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
        false
    }
}

fn io_error(error: std::io::Error) -> McpError {
    McpError::Internal {
        message: format!("Browser Use: {error}"),
    }
}

fn private_directory(path: &Path) -> McpResult<()> {
    std::fs::create_dir_all(path).map_err(io_error)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).map_err(io_error)?;
    }
    Ok(())
}

/// Hash a canonical full address, never a model-supplied or transport session name.
pub fn session_key(ctx: &McpCallContext) -> McpResult<String> {
    let invalid = |message: &str| McpError::PermissionDenied {
        message: message.into(),
    };
    if ctx
        .metadata
        .get("host_profile")
        .and_then(Value::as_str)
        .is_some_and(|host| host != "desktop")
    {
        return Err(invalid(
            "Browser Use must run on its Desktop execution Device",
        ));
    }
    let device = ctx
        .metadata
        .get("device_id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| invalid("Browser Use requires the execution Device identity"))?;
    let address: ChatAddress = serde_json::from_value(
        ctx.metadata
            .get("security_chat_address")
            .cloned()
            .ok_or_else(|| invalid("Browser Use requires a trusted full Chat Address"))?,
    )
    .map_err(|_| invalid("Invalid Browser Use Chat Address"))?;
    if ctx.agent_id.is_empty()
        || address.chat_id().as_str().is_empty()
        || address
            .agent_id()
            .is_some_and(|agent| agent.as_str() != ctx.agent_id)
    {
        return Err(invalid(
            "Browser Use owner does not match the executing Agent",
        ));
    }
    let identity = serde_json::to_vec(&(device, &ctx.agent_id, address.identity_key()))
        .expect("identity serializes");
    Ok(format!("{:x}", Sha256::digest(identity)))
}

fn properties(name: &str) -> Option<&'static [&'static str]> {
    Some(match name {
        "agent_browser_open" => &["url"],
        "agent_browser_read" => &["outline", "filter"],
        "agent_browser_snapshot" => &["interactive", "compact", "depth", "selector", "includeUrls"],
        "agent_browser_get_text" | "agent_browser_click" => &["selector"],
        "agent_browser_fill" => &["selector", "text"],
        "agent_browser_scroll" => &["direction", "amount", "selector"],
        "agent_browser_screenshot" => &["selector", "fullPage", "annotate"],
        "agent_browser_get_title" | "agent_browser_get_url" | "agent_browser_close" => &[],
        _ => return None,
    })
}

fn restricted_tool(mut tool: McpToolInfo) -> Option<McpToolInfo> {
    let allowed = properties(&tool.name)?;
    if let Some(props) = tool
        .input_schema
        .get_mut("properties")
        .and_then(Value::as_object_mut)
    {
        props.retain(|key, _| allowed.contains(&key.as_str()));
    }
    tool.input_schema["additionalProperties"] = json!(false);
    if let Some(required) = tool
        .input_schema
        .get_mut("required")
        .and_then(Value::as_array_mut)
    {
        required.retain(|key| key.as_str().is_some_and(|key| allowed.contains(&key)));
    }
    tool.description = match tool.name.as_str() {
        "agent_browser_open" => "Open an HTTP(S) page in this Chat's dedicated system Chromium browser. Rua owns the session; observe the page after navigation.".into(),
        "agent_browser_read" => "Read text from the current Chat's active page.".into(),
        "agent_browser_close" => "Close only this Chat's dedicated browser session.".into(),
        "agent_browser_click" => "Click one observed element in this Chat's browser. Observe again after page changes; never reuse stale refs or repeat an uncertain submission.".into(),
        "agent_browser_fill" => "Clear and fill one observed input in this Chat's browser. Observe the result before submitting.".into(),
        "agent_browser_scroll" => "Scroll the current Chat's page, then observe the newly visible content.".into(),
        "agent_browser_screenshot" => "Capture this Chat's browser as a PNG image. Requires a model configured for image input; text-only models should use read or snapshot.".into(),
        _ => tool.description,
    };
    tool.raw["inputSchema"] = tool.input_schema.clone();
    tool.raw["description"] = json!(tool.description);
    Some(tool)
}

fn validate_arguments(name: &str, args: &Value) -> McpResult<()> {
    let allowed = properties(name).ok_or_else(|| McpError::ToolNotFound {
        server: SERVER_ID.into(),
        tool: name.into(),
    })?;
    let error = |message: String| McpError::SchemaValidationFailed {
        server: SERVER_ID.into(),
        tool: name.into(),
        message,
    };
    let object = args
        .as_object()
        .ok_or_else(|| error("arguments must be an object".into()))?;
    for (key, value) in object {
        if !allowed.contains(&key.as_str()) {
            return Err(error(format!(
                "argument '{key}' is not exposed by Browser Use"
            )));
        }
        // Upstream converts fields to CLI tokens; reject flag-shaped positional values.
        if value
            .as_str()
            .is_some_and(|value| value.trim_start().starts_with('-'))
        {
            return Err(error(format!(
                "argument '{key}' cannot be a command-line option"
            )));
        }
    }
    if name == "agent_browser_scroll" {
        if args
            .get("direction")
            .is_some_and(|value| !matches!(value.as_str(), Some("up" | "down" | "left" | "right")))
        {
            return Err(error("direction must be up, down, left or right".into()));
        }
        if args
            .get("amount")
            .is_some_and(|value| value.as_u64().is_none_or(|amount| amount > 100_000))
        {
            return Err(error(
                "scroll amount must be between 0 and 100000 pixels".into(),
            ));
        }
    }
    if name == "agent_browser_open" {
        if let Some(url) = args.get("url") {
            let valid = url.as_str().is_some_and(|url| {
                reqwest::Url::parse(url).is_ok_and(|url| {
                    matches!(url.scheme(), "http" | "https") || url.as_str() == "about:blank"
                })
            });
            if !valid {
                return Err(error(
                    "url must be an absolute HTTP(S) URL or about:blank".into(),
                ));
            }
        }
    }
    Ok(())
}

#[async_trait]
impl McpClient for BundledBrowserClient {
    async fn connect(&self) -> McpResult<()> {
        let scope = CallScope::new(
            &McpCallContext::default(),
            SERVER_ID,
            "connect",
            CALL_TIMEOUT,
        );
        let _state = scope.wait(self.state.lock()).await?;
        if let Err(error) = scope
            .wait(self.connect_locked())
            .await
            .and_then(|result| result)
        {
            *self.fault.write().expect("browser fault") = Some(error.to_string());
            return Err(error);
        }
        *self.fault.write().expect("browser fault") = None;
        Ok(())
    }

    async fn initialize_result(&self) -> McpResult<McpInitializeResult> {
        self.transport.initialize_result().await
    }

    async fn list_tools(&self) -> McpResult<Vec<McpToolInfo>> {
        Ok(self
            .transport
            .list_tools()
            .await?
            .into_iter()
            .filter_map(restricted_tool)
            .collect())
    }

    async fn call_tool(
        &self,
        name: &str,
        mut args: Value,
        ctx: McpCallContext,
    ) -> McpResult<McpCallResult> {
        validate_arguments(name, &args)?;
        let key = session_key(&ctx)?;
        if name == "agent_browser_screenshot"
            && ctx
                .metadata
                .get(rua_core::tool::MODEL_SUPPORTS_IMAGE_METADATA_KEY)
                .and_then(Value::as_bool)
                != Some(true)
        {
            return Err(McpError::ConfigInvalid {
                message: "Browser screenshot requires a model configured for image input. Use page read/snapshot for text, or select a vision-capable model for this Chat.".into(),
            });
        }
        let scope = CallScope::new(&ctx, SERVER_ID, name, CALL_TIMEOUT);
        let mut state = scope.wait(self.state.lock()).await?;
        // Manager disconnect cancels all calls from the old entry generation.
        // A new explicit call may reopen its own session, never replay old input.
        if ctx.cancel.as_ref().is_some_and(|rx| *rx.borrow()) {
            return Err(McpError::Cancelled {
                server: SERVER_ID.into(),
            });
        }
        scope.wait(self.connect_locked()).await??;
        *self.fault.write().expect("browser fault") = None;
        let tool = scope
            .wait(self.list_tools())
            .await??
            .into_iter()
            .find(|tool| tool.name == name)
            .ok_or_else(|| McpError::ToolNotFound {
                server: SERVER_ID.into(),
                tool: name.into(),
            })?;
        McpToolAdapter::validate_args(&tool.input_schema, &args).map_err(|message| {
            McpError::SchemaValidationFailed {
                server: SERVER_ID.into(),
                tool: name.into(),
                message,
            }
        })?;
        // A shortened live socket name keeps macOS paths bounded; persistent profiles use the full digest.
        let session = &key[..24];
        let stop_file = self.socket_dir.join(format!("{session}.stop"));
        if stop_file.exists() {
            scope.wait(self.close_session(session)).await??;
            if matches!(
                name,
                "agent_browser_click" | "agent_browser_fill" | "agent_browser_scroll"
            ) {
                return Err(McpError::ConfigInvalid {message:"Previous browser operation was interrupted. Open/observe this Chat's page again before new input.".into()});
            }
            std::fs::remove_file(&stop_file).map_err(io_error)?;
        }
        let profile = self.settings.data_dir.join("profiles").join(&key);
        private_directory(&profile)?;
        args["session"] = json!(session);
        if !state.transports.contains_key(session) {
            let transport = Self::profile_transport(&self.settings, &self.environment, &profile);
            scope.wait(transport.connect()).await??;
            state.transports.insert(session.into(), transport);
        }
        args["timeoutMs"] = json!(scope.remaining().as_millis().max(1) as u64);
        let screenshot = if name == "agent_browser_screenshot" {
            let directory = self.socket_dir.join("screenshots");
            private_directory(&directory)?;
            let path = directory.join(format!("{}.png", uuid::Uuid::new_v4().simple()));
            args["path"] = json!(path);
            Some(path)
        } else {
            None
        };
        scope.check()?;
        state.sessions.insert(session.into());
        let mut in_flight = BrowserCall {
            stop_file,
            settled: false,
        };
        let mut result = state
            .transports
            .get(session)
            .expect("connected Chat transport")
            .call_tool(
                name,
                args,
                McpCallContext {
                    timeout: Some(scope.remaining()),
                    ..ctx
                },
            )
            .await;
        if let Some(path) = screenshot {
            if let Ok(result) = &mut result {
                for content in &mut result.content {
                    if let McpContent::Text { text } = content {
                        *text = text.replace(
                            path.to_string_lossy().as_ref(),
                            "Current Chat browser screenshot",
                        );
                    }
                }
                if !result.is_error
                    && !result
                        .content
                        .iter()
                        .any(|content| matches!(content, McpContent::Image { .. }))
                {
                    result.is_error = true;
                    result.content.push(McpContent::Text { text: "Screenshot did not produce an inline image (it may exceed the driver image limit). Try a smaller visible area; no image was delivered to the model.".into() });
                }
            }
            let _ = std::fs::remove_file(path);
        }
        // A daemon error response is a completed request (for example a stale
        // element). Missing response/CLI timeout cannot prove input completion.
        if result.as_ref().is_ok_and(|result| {
            result
                .structured_content
                .as_ref()
                .is_some_and(|value| value["response"].is_object())
        }) {
            in_flight.settled = true;
        }
        if !in_flight.settled {
            drop(in_flight);
            if let Some(transport) = state.transports.remove(session) {
                let _ = transport.shutdown().await;
            }
            let cleanup = self.close_session(session).await;
            if cleanup.is_ok() {
                state.sessions.remove(session);
            }
            let reason = match &result {
                Ok(result) => result.text(),
                Err(error) => error.to_string(),
            };
            *self.fault.write().expect("browser fault") = Some(reason.clone());
            return Ok(McpCallResult {content:vec![McpContent::Text {text:format!("Browser operation interrupted: {reason}. Submitted action outcome may be unknown; no action was replayed. Observe the page again before a new input. Cleanup: {cleanup:?}")}],structured_content:Some(json!({"interrupted":true,"outcome_unknown":true,"requires_new_observation":true})),is_error:true});
        }
        match &result {
            Ok(result) if name == "agent_browser_close" && !result.is_error => {
                let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
                while self.socket_dir.join(format!("{session}.pid")).exists()
                    && tokio::time::Instant::now() < deadline
                {
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
                if !self.socket_dir.join(format!("{session}.pid")).exists() {
                    state.sessions.remove(session);
                    if let Some(transport) = state.transports.remove(session) {
                        transport.shutdown().await?;
                    }
                }
            }
            _ => {}
        }
        result
    }

    async fn invalidate_tool_cache(&self) {
        self.transport.invalidate_tool_cache().await;
    }
    async fn ping(&self) -> McpResult<()> {
        self.transport.ping().await
    }

    async fn shutdown(&self) -> McpResult<()> {
        let _ = std::fs::remove_file(self.socket_dir.join("owner"));
        let mut state = self.state.lock().await;
        let mut error = None;
        let deadline = tokio::time::Instant::now() + CLOSE_TIMEOUT;
        for session in &state.sessions {
            rua_config::atomic_write(&self.socket_dir.join(format!("{session}.stop")), b"stop\n")
                .map_err(io_error)?;
        }
        for session in state.sessions.clone() {
            let result = tokio::time::timeout_at(deadline, self.close_session(&session))
                .await
                .unwrap_or_else(|_| {
                    Err(McpError::ProtocolError {
                        server: SERVER_ID.into(),
                        message: "Timed out waiting for owned browser shutdown".into(),
                    })
                });
            match result {
                Ok(()) => {
                    state.sessions.remove(&session);
                }
                Err(e) => {
                    error = Some(e);
                }
            }
        }
        for (_, transport) in std::mem::take(&mut state.transports) {
            if let Err(e) = transport.shutdown().await {
                error = Some(e);
            }
        }
        self.transport.shutdown().await?;
        if let Some(error) = error {
            return Err(error);
        }
        let _ = std::fs::remove_dir_all(&self.socket_dir);
        Ok(())
    }

    fn status(&self) -> McpServerStatus {
        let mut status = self.transport.status();
        status.transport_type = "bundled_browser".into();
        if let Some(message) = self
            .unavailable()
            .or_else(|| self.fault.read().expect("browser fault").clone())
        {
            status.state = McpConnectionState::Failed;
            status.last_error = Some(message);
        }
        status
    }
}

struct BrowserCall {
    stop_file: PathBuf,
    settled: bool,
}
impl Drop for BrowserCall {
    fn drop(&mut self) {
        if !self.settled {
            let _ = rua_config::atomic_write(&self.stop_file, b"stop\n");
        }
    }
}
impl Drop for BundledBrowserClient {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(self.socket_dir.join("owner"));
    }
}
