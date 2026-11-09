//! Per-Agent MCP proxy to the resident Runtime's private native Cua host.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::transports::stdio::{StdioConfig, StdioMcpClient};
use crate::{call_scope::CallScope, computer_control};
use async_trait::async_trait;
use mcp_manager::client::McpClient;
use mcp_manager::error::{McpError, McpResult};
use mcp_manager::status::{McpConnectionState, McpServerStatus};
use mcp_manager::types::{
    AutoConnectPolicy, McpCallContext, McpCallResult, McpConfig, McpInitializeResult,
    McpServerConfig, McpServerId, McpToolInfo, McpTransportConfig,
};
use rua_core::runtime_authority::RuntimeAuthority;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::Mutex;

pub const SERVER_ID: &str = "rua_computer";
pub const DRIVER_VERSION: &str = "0.38.0";
const TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
pub struct BundledComputer {
    pub driver: PathBuf,
    pub connection_file: PathBuf,
}

#[derive(Deserialize)]
struct Connection {
    #[serde(default)]
    generation: String,
    #[serde(default)]
    socket_path: String,
    #[serde(default)]
    driver_version: String,
    #[serde(default)]
    host_bundle_id: String,
    error: Option<String>,
    #[serde(flatten)]
    _extra: BTreeMap<String, Value>,
}

impl BundledComputer {
    pub fn connection_generation(&self) -> McpResult<String> {
        self.connection().map(|connection| connection.generation)
    }
    pub fn from_runtime(authority: &RuntimeAuthority, cache_dir: &Path) -> Option<Self> {
        if authority.inventory().host_profile != "desktop"
            || !matches!(
                authority.inventory().os_family.as_str(),
                "macos" | "windows"
            )
        {
            return None;
        }
        Some(Self {
            driver: std::env::var_os("RUA_BUNDLED_CUA_DRIVER")
                .map(PathBuf::from)
                .or_else(|| {
                    std::env::current_exe()
                        .ok()
                        .and_then(|exe| exe.canonicalize().ok())
                        .map(|exe| {
                            exe.with_file_name(if cfg!(windows) {
                                "cua-driver.exe"
                            } else {
                                "cua-driver"
                            })
                        })
                })?,
            connection_file: cache_dir.join("desktop-automation/computer/cua-connection.json"),
        })
    }

    pub fn defaults(&self, mut config: McpConfig) -> McpConfig {
        let id = McpServerId::new(SERVER_ID);
        if config
            .clients
            .get(&id)
            .is_some_and(|entry| !is_product_computer(entry))
        {
            return config;
        }
        let entry = config.clients.entry(id).or_insert_with(|| McpServerConfig {
            name: "Computer Use".into(),
            description: "Rua-owned native desktop observation and input".into(),
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
            extra: BTreeMap::from([("rua_builtin".into(), json!("computer"))]),
        });
        entry.transport = McpTransportConfig::Stdio {
            command: self.driver.to_string_lossy().into_owned(),
            args: vec!["mcp".into(), "--embedded".into()],
            env: BTreeMap::new(),
            cwd: None,
            use_proot: false,
        };
        config
    }

    fn connection(&self) -> McpResult<Connection> {
        if let Ok(error) = std::env::var("RUA_DESKTOP_AUTOMATION_ERROR") {
            return Err(unavailable(&error));
        }
        if !rua_core::runtime_authority::browser::executable_file(&self.driver) {
            return Err(unavailable(
                "Bundled Cua Driver is missing. Repair the Rua Desktop installation.",
            ));
        }
        let bytes = std::fs::read(&self.connection_file).map_err(|_| unavailable("Rua Computer Use host is unavailable. Recheck Computer Use in Desktop Tools on the execution device."))?;
        let connection: Connection = serde_json::from_slice(&bytes).map_err(|_| {
            unavailable(
                "Invalid Rua native host descriptor. Restart Computer Use in Desktop Tools.",
            )
        })?;
        if let Some(error) = &connection.error {
            return Err(unavailable(error));
        }
        if connection.driver_version != DRIVER_VERSION
            || connection.generation.is_empty()
            || connection.socket_path.is_empty()
            || connection.host_bundle_id.is_empty()
        {
            return Err(unavailable(
                "Rua native host version/connection mismatch. Restart Computer Use or repair the Rua installation.",
            ));
        }
        Ok(connection)
    }
}

pub fn is_product_computer(config: &McpServerConfig) -> bool {
    config.extra.get("rua_builtin").and_then(Value::as_str) == Some("computer")
}
fn unavailable(message: &str) -> McpError {
    McpError::ConfigInvalid {
        message: message.into(),
    }
}

struct ComputerState {
    transport: Option<StdioMcpClient>,
    generation: String,
    sessions: BTreeSet<String>,
    observations: BTreeMap<String, Observation>,
}

struct Observation {
    id: String,
    pid: u64,
    window_id: u64,
    fingerprint: Value,
    captured: tokio::time::Instant,
    snapshot_id: Option<String>,
    elements: BTreeSet<String>,
    screenshot_size: Option<(u64, u64)>,
}

pub struct BundledComputerClient {
    settings: BundledComputer,
    state: Mutex<ComputerState>,
    status: std::sync::RwLock<McpServerStatus>,
}

impl BundledComputerClient {
    pub fn new(settings: BundledComputer) -> Self {
        Self {
            settings,
            state: Mutex::new(ComputerState {
                transport: None,
                generation: String::new(),
                sessions: BTreeSet::new(),
                observations: BTreeMap::new(),
            }),
            status: std::sync::RwLock::new(McpServerStatus {
                key: SERVER_ID.into(),
                name: "Computer Use".into(),
                enabled: true,
                state: McpConnectionState::NotConnected,
                transport_type: "bundled_computer".into(),
                tools_count: 0,
                last_error: None,
                connected_at: None,
                mobile_unavailable_reason: None,
                config_errors: vec![],
            }),
        }
    }

    fn mark(&self, error: Option<&McpError>) {
        let mut status = self.status.write().expect("computer status");
        status.state = if error.is_some() {
            McpConnectionState::Failed
        } else {
            McpConnectionState::Ready
        };
        status.last_error = error.map(ToString::to_string);
    }

    async fn connect_locked(&self, state: &mut ComputerState) -> McpResult<()> {
        let connection = self.settings.connection()?;
        if state.generation == connection.generation
            && state
                .transport
                .as_ref()
                .is_some_and(|transport| transport.status().state.is_available())
        {
            return Ok(());
        }
        if let Some(old) = state.transport.take() {
            old.shutdown().await?;
        }
        state.sessions.clear();
        state.observations.clear();
        let mut environment: BTreeMap<String, String> = std::env::vars()
            .filter(|(name, _)| {
                matches!(
                    name.as_str(),
                    "HOME"
                        | "USERPROFILE"
                        | "SystemRoot"
                        | "SYSTEMROOT"
                        | "TEMP"
                        | "TMP"
                        | "TMPDIR"
                        | "LANG"
                )
            })
            .collect();
        environment.insert("CUA_DRIVER_RS_TELEMETRY_ENABLED".into(), "0".into());
        environment.insert("CUA_DRIVER_RS_UPDATE_CHECK".into(), "false".into());
        let transport = StdioMcpClient::new(StdioConfig {
            server_key: SERVER_ID.into(),
            server_name: "Computer Use".into(),
            command: self.settings.driver.to_string_lossy().into_owned(),
            args: vec![
                "mcp".into(),
                "--embedded".into(),
                "--socket".into(),
                connection.socket_path,
                "--host-bundle-id".into(),
                connection.host_bundle_id,
            ],
            env: environment,
            cwd: None,
            startup_timeout: Duration::from_secs(10),
            call_timeout: TIMEOUT,
            stderr_log_prefix: Some("Computer Use".into()),
        })
        .with_isolated_env();
        transport.connect().await?;
        let info = transport.initialize_result().await?;
        if info.server_version != DRIVER_VERSION {
            transport.shutdown().await?;
            return Err(unavailable(
                "Cua Driver MCP version does not match this Rua installation.",
            ));
        }
        state.generation = connection.generation;
        state.transport = Some(transport);
        Ok(())
    }

    pub async fn control_requires_restart(&self) -> bool {
        let state = self.state.lock().await;
        computer_control::requires_reset(&state.generation).await
    }

    async fn target(
        transport: &StdioMcpClient,
        pid: u64,
        window_id: u64,
        ctx: &McpCallContext,
    ) -> McpResult<Value> {
        let result = transport
            .call_tool("list_windows", json!({"pid":pid}), ctx.clone())
            .await?;
        if result.is_error {
            return Err(unavailable(&result.text()));
        }
        result.structured_content.as_ref().and_then(|s|s["windows"].as_array())
            .and_then(|windows|windows.iter().find(|w|w["pid"]==pid && w["window_id"]==window_id))
            .map(computer_control::window_fingerprint)
            .ok_or_else(||unavailable("Target window disappeared or belongs to another process. Observe again before input."))
    }

    async fn interact(&self, args: Value, ctx: McpCallContext) -> McpResult<McpCallResult> {
        let actions = computer_control::action_calls(&args)?;
        let session = crate::bundled_browser::session_key(&ctx)?;
        for key in ["turn_id", "tool_call_id"] {
            if ctx
                .metadata
                .get(key)
                .and_then(Value::as_str)
                .is_none_or(str::is_empty)
            {
                return Err(unavailable(
                    "Computer input requires a trusted Turn and tool call identity",
                ));
            }
        }
        let capture = args.get("include_screenshot") != Some(&json!(false));
        if capture
            && ctx
                .metadata
                .get(rua_core::tool::MODEL_SUPPORTS_IMAGE_METADATA_KEY)
                .and_then(Value::as_bool)
                != Some(true)
        {
            return Err(unavailable(
                "Computer screenshot requires a model configured for image input",
            ));
        }
        let scope = CallScope::new(&ctx, SERVER_ID, computer_control::INTERACT, TIMEOUT);
        let mut state = scope.wait(self.state.lock()).await?;
        scope.wait(self.connect_locked(&mut state)).await??;
        let mut lease = computer_control::acquire(&state.generation, &scope, &ctx).await?;
        let observation = state.observations.get(&session).ok_or_else(|| {
            unavailable("Observe this target window in the same Chat before input")
        })?;
        if observation.id != args["observation_id"]
            || observation.pid != args["pid"]
            || observation.window_id != args["window_id"]
            || observation.captured.elapsed() > Duration::from_secs(120)
        {
            return Err(unavailable(
                "Stale or foreign desktop observation. Observe this window again.",
            ));
        }
        let transport = state.transport.as_ref().expect("connected");
        let tools = scope.wait(transport.list_tools()).await??;
        for (name, input) in &actions {
            let tool = tools
                .iter()
                .find(|tool| tool.name == *name)
                .ok_or_else(|| unavailable("Input action is unsupported on this platform"))?;
            crate::tool_adapter::McpToolAdapter::validate_args(&tool.input_schema, input)
                .map_err(|e| unavailable(&e))?;
            if input
                .as_object()
                .expect("validated input")
                .keys()
                .any(|key| tool.input_schema["properties"].get(key).is_none())
            {
                return Err(unavailable(
                    "Input argument is unsupported on this platform",
                ));
            }
            if let Some(token) = input.get("element_token").and_then(Value::as_str) {
                if !observation.elements.contains(token) {
                    return Err(unavailable(
                        "Element token is not part of this Chat's observation",
                    ));
                }
            }
            if let Some(snapshot) = input.get("snapshot_id").and_then(Value::as_str) {
                if observation.snapshot_id.as_deref() != Some(snapshot) {
                    return Err(unavailable(
                        "Snapshot does not match this Chat's observation",
                    ));
                }
            }
            if let Some(x) = input.get("x").and_then(Value::as_f64) {
                let (width, height) = observation
                    .screenshot_size
                    .ok_or_else(|| unavailable("Pixel input requires this Chat's screenshot"))?;
                if x >= width as f64 || input["y"].as_f64().unwrap_or(f64::MAX) >= height as f64 {
                    return Err(unavailable("Pixel lies outside the observed screenshot"));
                }
            }
        }
        let permission = scope.wait(Self::permissions(transport)).await??;
        if !permission.input {
            return Err(unavailable(
                "Rua lacks input permission. Grant Accessibility and check again.",
            ));
        }
        let observation = state.observations.remove(&session).expect("validated");
        let mut completed = Vec::new();
        for (name, mut input) in computer_control::dispatch_steps(actions) {
            if let Err(error) = scope.check() {
                return Ok(interrupted(&completed, &error.to_string(), false));
            }
            let mut call_ctx = ctx.clone();
            call_ctx.timeout = Some(scope.remaining().min(Duration::from_secs(10)));
            let transport = state.transport.as_ref().expect("connected");
            let target = scope
                .wait(Self::target(
                    transport,
                    observation.pid,
                    observation.window_id,
                    &call_ctx,
                ))
                .await;
            match target {
                Ok(Ok(target)) if target == observation.fingerprint => {}
                Ok(Ok(_)) => {
                    return Ok(interrupted(
                        &completed,
                        "Target window changed. Observe again before input.",
                        false,
                    ))
                }
                Ok(Err(error)) | Err(error) => {
                    return Ok(interrupted(&completed, &error.to_string(), false))
                }
            }
            if let Err(error) = scope.check() {
                return Ok(interrupted(&completed, &error.to_string(), false));
            }
            input["session"] = json!(session);
            state.sessions.insert(session.clone());
            // A submitted OS operation is indivisible to this legacy driver. Keep
            // the lease until its reply, even if Stop arrives. No next step runs.
            // If the future is dropped, the lease records an uncertain generation.
            call_ctx.cancel = None;
            call_ctx.timeout = Some(scope.remaining().min(Duration::from_secs(10)));
            lease.submitted();
            let result = state
                .transport
                .as_ref()
                .expect("connected")
                .call_tool(&name, input.clone(), call_ctx)
                .await;
            match result {
                Ok(result)=>{
                    lease.settled();
                    completed.push(json!({"tool":name,"arguments":input,"is_error":result.is_error,"result":result.structured_content,"text":result.text()}));
                    if result.is_error {return Ok(interrupted(&completed,"Native input refused or failed. Observe before another action.",false));}
                }
                Err(error)=>return Ok(interrupted(&completed,&format!("{error}; submitted {name} outcome is unknown. The desktop Runtime is recovering its executor; observe again after recovery."),true)),
            }
        }
        if let Err(error) = scope.check() {
            return Ok(interrupted(&completed, &error.to_string(), false));
        }
        let mut observe_ctx = ctx.clone();
        observe_ctx.timeout = Some(scope.remaining());
        let result=state.transport.as_ref().expect("connected").call_tool("get_window_state",json!({"pid":observation.pid,"window_id":observation.window_id,"session":session,"include_screenshot":capture,"tree_format":"both"}),observe_ctx).await;
        match result {
            Ok(mut result) => {
                if capture && !result.content.iter().any(|part|matches!(part,mcp_manager::types::McpContent::Image{data,..} if !data.is_empty())) {
                    return Ok(interrupted(&completed,"Input submitted, but final screenshot unavailable. Observe again.",false));
                }
                result.structured_content = Some(
                    json!({"requested_actions":args["actions"],"actions":completed,"observation":result.structured_content,"requires_new_observation":true}),
                );
                Ok(result)
            }
            Err(error) => Ok(interrupted(
                &completed,
                &format!("Input completed; final observation failed: {error}"),
                false,
            )),
        }
    }

    pub async fn diagnostics(&self) -> McpResult<ComputerPermissions> {
        let scope = CallScope::new(
            &McpCallContext::default(),
            SERVER_ID,
            "check_permissions",
            TIMEOUT,
        );
        let result = scope
            .wait(async {
                let mut state = self.state.lock().await;
                self.connect_locked(&mut state).await?;
                Self::permissions(state.transport.as_ref().expect("connected")).await
            })
            .await
            .and_then(|result| result);
        self.mark(result.as_ref().err());
        result
    }

    async fn permissions(transport: &StdioMcpClient) -> McpResult<ComputerPermissions> {
        let result = transport
            .call_tool(
                "check_permissions",
                json!({}),
                McpCallContext {
                    timeout: Some(Duration::from_secs(5)),
                    ..Default::default()
                },
            )
            .await?;
        if result.is_error {
            return Err(unavailable(&result.text()));
        }
        let value = result.structured_content.as_ref().ok_or_else(|| {
            unavailable("Cua Driver returned no structured permission diagnostics.")
        })?;
        ComputerPermissions::from_driver(value)
    }
}

/// OS permission readiness is separate from successful pixel capture. Windows
/// has no macOS TCC grant; the actual observation still diagnoses a locked or
/// missing interactive desktop and elevated targets.
#[derive(Debug, Clone, Serialize)]
pub struct ComputerPermissions {
    pub platform: String,
    pub accessibility: bool,
    pub screen_recording: Option<bool>,
    pub input: bool,
    pub integrity_level: Option<String>,
}

impl ComputerPermissions {
    pub fn from_driver(value: &Value) -> McpResult<Self> {
        if let Some(uia) = value.get("uia").and_then(Value::as_bool) {
            return Ok(Self {
                platform: "windows".into(),
                accessibility: uia,
                screen_recording: None,
                input: value["post_message"].as_bool() == Some(true),
                integrity_level: value["integrity_level"].as_str().map(str::to_owned),
            });
        }
        if value.pointer("/source/attribution").and_then(Value::as_str) != Some("host") {
            return Err(unavailable("Computer Use is not attached to Rua's native permission host. Restart Rua Desktop."));
        }
        let accessibility = value["accessibility"]
            .as_bool()
            .ok_or_else(|| unavailable("Missing accessibility diagnostics"))?;
        let screen_recording = value["screen_recording"]
            .as_bool()
            .ok_or_else(|| unavailable("Missing screen recording diagnostics"))?;
        Ok(Self {
            platform: "macos".into(),
            accessibility,
            screen_recording: Some(screen_recording),
            input: accessibility,
            integrity_level: None,
        })
    }

    pub fn capture_allowed(&self) -> bool {
        self.screen_recording != Some(false)
    }
}

fn properties(name: &str) -> Option<&'static [&'static str]> {
    Some(match name {
        "list_apps" => &[],
        "list_windows" => &["pid", "on_screen_only"],
        "get_window_state" => &[
            "pid",
            "window_id",
            "query",
            "include_accessibility_tree",
            "include_screenshot",
            "max_elements",
            "max_depth",
            "max_dimension",
        ],
        "get_desktop_state" => &[],
        _ => return None,
    })
}

fn restrict(mut tool: McpToolInfo) -> Option<McpToolInfo> {
    let allowed = properties(&tool.name)?;
    if let Some(properties) = tool
        .input_schema
        .get_mut("properties")
        .and_then(Value::as_object_mut)
    {
        properties.retain(|name, _| allowed.contains(&name.as_str()));
    }
    tool.input_schema["additionalProperties"] = json!(false);
    if let Some(required) = tool
        .input_schema
        .get_mut("required")
        .and_then(Value::as_array_mut)
    {
        required.retain(|name| name.as_str().is_some_and(|name| allowed.contains(&name)));
    }
    tool.description.push_str(" Operates on the execution Device. Images require a vision-capable model; window text can use include_screenshot:false. Observe the exact pid/window before input.");
    tool.raw["inputSchema"] = tool.input_schema.clone();
    tool.raw["description"] = json!(tool.description);
    Some(tool)
}

#[async_trait]
impl McpClient for BundledComputerClient {
    async fn connect(&self) -> McpResult<()> {
        let scope = CallScope::new(&McpCallContext::default(), SERVER_ID, "connect", TIMEOUT);
        let result = scope
            .wait(async {
                let mut state = self.state.lock().await;
                self.connect_locked(&mut state).await
            })
            .await
            .and_then(|result| result);
        self.mark(result.as_ref().err());
        result
    }
    async fn initialize_result(&self) -> McpResult<McpInitializeResult> {
        let state = self.state.lock().await;
        state
            .transport
            .as_ref()
            .ok_or_else(|| unavailable("Computer Use is disconnected"))?
            .initialize_result()
            .await
    }
    async fn list_tools(&self) -> McpResult<Vec<McpToolInfo>> {
        let scope = CallScope::new(&McpCallContext::default(), SERVER_ID, "tools/list", TIMEOUT);
        let result = scope
            .wait(async {
                let mut state = self.state.lock().await;
                self.connect_locked(&mut state).await?;
                let transport = state.transport.as_ref().expect("connected");
                let permissions = Self::permissions(transport).await?;
                let accessibility = permissions.accessibility;
                let screenshot = permissions.capture_allowed();
                let native_tools = transport.list_tools().await?;
                let mut tools = native_tools
                    .clone()
                    .into_iter()
                    .filter(|tool| match tool.name.as_str() {
                        "get_window_state" => accessibility || screenshot,
                        "get_desktop_state" => screenshot,
                        _ => true,
                    })
                    .filter_map(restrict)
                    .collect::<Vec<_>>();
                if permissions.input {
                    if let Some(tool) = computer_control::interaction_tool(&native_tools) {
                        tools.push(tool);
                    }
                }
                self.status.write().expect("computer status").tools_count = tools.len();
                Ok(tools)
            })
            .await
            .and_then(|result| result);
        self.mark(result.as_ref().err());
        result
    }
    async fn call_tool(
        &self,
        name: &str,
        mut args: Value,
        ctx: McpCallContext,
    ) -> McpResult<McpCallResult> {
        if name == computer_control::INTERACT {
            let result = self.interact(args, ctx).await;
            if let Ok(value) = &result {
                if value
                    .structured_content
                    .as_ref()
                    .is_some_and(|v| v["outcome_unknown"] == true)
                {
                    self.mark(Some(&unavailable(&value.text())));
                } else if !value.is_error {
                    self.mark(None);
                }
            }
            return result;
        }
        let scope = CallScope::new(&ctx, SERVER_ID, name, TIMEOUT);
        let allowed = properties(name).ok_or_else(|| McpError::ToolNotFound {
            server: SERVER_ID.into(),
            tool: name.into(),
        })?;
        let object = args
            .as_object()
            .ok_or_else(|| unavailable("Computer Use arguments must be an object"))?;
        if object.keys().any(|key| !allowed.contains(&key.as_str())) {
            return Err(unavailable(
                "Computer Use does not expose arbitrary sessions, output paths or driver controls",
            ));
        }
        let session = crate::bundled_browser::session_key(&ctx)?;
        if ctx.cancel.as_ref().is_some_and(|rx| *rx.borrow()) {
            return Err(McpError::Cancelled {
                server: SERVER_ID.into(),
            });
        }
        let capture = name == "get_desktop_state"
            || name == "get_window_state" && args.get("include_screenshot") != Some(&json!(false));
        if capture
            && ctx
                .metadata
                .get(rua_core::tool::MODEL_SUPPORTS_IMAGE_METADATA_KEY)
                .and_then(Value::as_bool)
                != Some(true)
        {
            return Err(unavailable("Computer observation with screenshots requires a model configured for image input. Use get_window_state with include_screenshot:false for accessibility text."));
        }
        let mut state = scope.wait(self.state.lock()).await?;
        if matches!(name, "get_window_state" | "get_desktop_state") {
            state.observations.remove(&session);
        }
        scope.wait(self.connect_locked(&mut state)).await??;
        let transport = state.transport.as_ref().expect("connected");
        let tool = scope
            .wait(transport.list_tools())
            .await??
            .into_iter()
            .find(|tool| tool.name == name)
            .and_then(restrict)
            .ok_or_else(|| unavailable("Computer Use operation is unsupported on this platform"))?;
        if object
            .keys()
            .any(|key| tool.input_schema["properties"].get(key).is_none())
        {
            return Err(unavailable(
                "Computer Use argument is unsupported on this platform",
            ));
        }
        crate::tool_adapter::McpToolAdapter::validate_args(&tool.input_schema, &args)
            .map_err(|message| unavailable(&message))?;
        let permissions = scope.wait(Self::permissions(transport)).await??;
        if capture && !permissions.capture_allowed() {
            return Err(unavailable("Screen Recording permission is missing. Grant Rua permission in the execution device's system settings, then recheck Computer Use."));
        }
        if name == "get_window_state"
            && args.get("include_accessibility_tree") != Some(&json!(false))
            && !permissions.accessibility
        {
            return Err(unavailable("Accessibility permission is missing. Grant Rua permission, then recheck Computer Use."));
        }
        if matches!(name, "get_window_state" | "get_desktop_state") {
            args["session"] = json!(session);
            if name == "get_window_state" {
                args["tree_format"] = json!("both");
            }
            if !state.sessions.contains(&session) {
                let started = scope
                    .wait(transport.call_tool(
                        "start_session",
                        json!({"session":session}),
                        ctx.clone(),
                    ))
                    .await??;
                if started.is_error {
                    return Ok(started);
                }
            }
            state.sessions.insert(session.clone());
        }
        let target = if name == "get_window_state" {
            Some(
                scope
                    .wait(Self::target(
                        state.transport.as_ref().expect("connected"),
                        args["pid"]
                            .as_u64()
                            .ok_or_else(|| unavailable("Invalid pid"))?,
                        args["window_id"]
                            .as_u64()
                            .ok_or_else(|| unavailable("Invalid window id"))?,
                        &ctx,
                    ))
                    .await??,
            )
        } else {
            None
        };
        scope.check()?;
        let result =
            scope
                .wait(state.transport.as_ref().expect("connected").call_tool(
                    name,
                    args.clone(),
                    ctx,
                ))
                .await
                .and_then(|result| result);
        self.mark(result.as_ref().err());
        let mut result = result?;
        if capture && !result.is_error && !result.content.iter().any(|part|
            matches!(part, mcp_manager::types::McpContent::Image { data, .. } if !data.is_empty())) {
            return Err(unavailable("Cua Driver did not return the requested screenshot. Observe again before taking action."));
        }
        if let Some(fingerprint) = target.filter(|_| !result.is_error) {
            let id = uuid::Uuid::new_v4().to_string();
            let payload = result.structured_content.get_or_insert_with(|| json!({}));
            let snapshot_id = payload["snapshot_id"].as_str().map(str::to_owned);
            let elements = payload["elements"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|e| e["element_token"].as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            let screenshot_size = payload["screenshot_width"]
                .as_u64()
                .zip(payload["screenshot_height"].as_u64());
            payload["observation_id"] = json!(id);
            state.observations.insert(
                session,
                Observation {
                    id,
                    pid: args["pid"].as_u64().unwrap(),
                    window_id: args["window_id"].as_u64().unwrap(),
                    fingerprint,
                    captured: tokio::time::Instant::now(),
                    snapshot_id,
                    elements,
                    screenshot_size,
                },
            );
        }
        Ok(result)
    }
    async fn invalidate_tool_cache(&self) {
        if let Some(transport) = &self.state.lock().await.transport {
            transport.invalidate_tool_cache().await;
        }
    }
    async fn shutdown(&self) -> McpResult<()> {
        let mut state = self.state.lock().await;
        if let Some(transport) = state.transport.take() {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
            for session in std::mem::take(&mut state.sessions) {
                let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                if remaining.is_zero() {
                    break;
                }
                let _ = transport
                    .call_tool(
                        "end_session",
                        json!({"session":session}),
                        McpCallContext {
                            timeout: Some(remaining.min(Duration::from_secs(1))),
                            ..Default::default()
                        },
                    )
                    .await;
            }
            transport.shutdown().await?;
        }
        state.observations.clear();
        let mut status = self.status.write().expect("computer status");
        status.state = McpConnectionState::NotConnected;
        status.last_error = None;
        Ok(())
    }
    fn status(&self) -> McpServerStatus {
        let mut status = self.status.read().expect("computer status").clone();
        if let Err(error) = self.settings.connection() {
            status.state = McpConnectionState::Failed;
            status.last_error = Some(error.to_string());
        }
        status
    }
}

fn interrupted(completed: &[Value], message: &str, unknown: bool) -> McpCallResult {
    McpCallResult {content:vec![mcp_manager::types::McpContent::Text{text:format!("Desktop interaction interrupted after {} replied steps: {message}. Subsequent steps were not sent.",completed.len())}],is_error:true,structured_content:Some(json!({"actions":completed,"outcome_unknown":unknown,"interrupted":true,"requires_new_observation":true}))}
}
