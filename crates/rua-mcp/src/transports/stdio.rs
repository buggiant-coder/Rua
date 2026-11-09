use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{oneshot, watch, Mutex as TokioMutex, Notify, RwLock};
use tokio::task::JoinHandle;

use mcp_manager::client::McpClient;
use mcp_manager::error::{McpError, McpResult};
use mcp_manager::status::{McpConnectionState, McpServerStatus};
use mcp_manager::types::{
    McpCallContext, McpCallResult, McpContent, McpInitializeResult, McpToolInfo,
};

use super::stdio_reader::{PendingMap, PendingRequest, ReaderTask};

#[derive(Debug, Clone)]
pub struct StdioConfig {
    pub server_key: String,
    pub server_name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub cwd: Option<PathBuf>,
    pub startup_timeout: Duration,
    pub call_timeout: Duration,

    pub stderr_log_prefix: Option<String>,
}

#[derive(Debug)]
pub(super) struct StdinWriter {
    pub(super) stdin: TokioMutex<ChildStdin>,
    complete_frame: std::sync::atomic::AtomicBool,
}

impl StdinWriter {
    /// A dropped/expired call still tells the server to cancel its request-scoped work.
    pub(super) fn cancel_request(
        self: Arc<Self>,
        id: u64,
        stop: tokio_util::sync::CancellationToken,
    ) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        runtime.spawn(async move {
            tokio::select! {
                biased;
                _ = stop.cancelled() => {},
                _ = tokio::time::timeout(Duration::from_secs(1), async {
                    let mut stream = self.stdin.lock().await;
                    if !self.complete_frame.swap(false, Ordering::AcqRel) { return; }
                    let line = format!("{}\n", serde_json::json!({
                        "jsonrpc":"2.0", "method":"notifications/cancelled",
                        "params":{"requestId":id,"reason":"Client stopped waiting for this call"}
                    }));
                    if stream.write_all(line.as_bytes()).await.is_ok() && stream.flush().await.is_ok() {
                        self.complete_frame.store(true, Ordering::Release);
                    }
                }) => {},
            }
        });
    }
}

#[derive(Debug)]
struct StdioState {
    connection: McpConnectionState,
    child: Option<Child>,

    request_id: AtomicU64,

    pending: PendingMap,

    stdin: Option<Arc<StdinWriter>>,

    reader_handle: Option<JoinHandle<()>>,

    tools: Vec<McpToolInfo>,

    init_result: Option<McpInitializeResult>,

    last_error: Option<String>,

    stderr_tail: Vec<String>,
}

pub struct StdioMcpClient {
    isolated_env: bool,
    env_remove: BTreeSet<String>,
    lifetime: tokio_util::sync::CancellationToken,
    lifecycle: TokioMutex<()>,
    config: StdioConfig,
    state: Arc<RwLock<StdioState>>,

    startup_ready: Arc<Notify>,
    shutdown_requested: Arc<Notify>,
    running_tx: Arc<watch::Sender<bool>>,
    running_rx: watch::Receiver<bool>,
}

impl StdioMcpClient {
    pub fn new(config: StdioConfig) -> Self {
        let (running_tx, running_rx) = watch::channel(false);
        Self {
            config,
            isolated_env: false,
            env_remove: BTreeSet::new(),
            lifetime: tokio_util::sync::CancellationToken::new(),
            lifecycle: TokioMutex::new(()),
            state: Arc::new(RwLock::new(StdioState {
                connection: McpConnectionState::NotConnected,
                child: None,
                request_id: AtomicU64::new(0),
                pending: Arc::new(std::sync::Mutex::new(HashMap::new())),
                stdin: None,
                reader_handle: None,
                tools: Vec::new(),
                init_result: None,
                last_error: None,
                stderr_tail: Vec::new(),
            })),
            startup_ready: Arc::new(Notify::new()),
            shutdown_requested: Arc::new(Notify::new()),
            running_tx: Arc::new(running_tx),
            running_rx,
        }
    }

    /// Use only the supplied environment for an application-owned driver.
    pub fn with_isolated_env(mut self) -> Self {
        self.isolated_env = true;
        self
    }

    /// Remove explicitly denied inherited variables for native subprocesses.
    pub fn with_env_remove(mut self, names: BTreeSet<String>) -> Self {
        self.env_remove = names;
        self
    }

    pub async fn start(&self) -> McpResult<()> {
        self.connect().await?;
        let _ = self.running_tx.send(true);
        self.startup_ready.notify_waiters();
        Ok(())
    }

    pub async fn request_shutdown(&self) -> McpResult<()> {
        self.shutdown_requested.notify_waiters();
        self.shutdown().await?;
        let _ = self.running_tx.send(false);
        Ok(())
    }

    pub fn is_running(&self) -> bool {
        *self.running_rx.borrow()
    }

    pub fn startup_ready(&self) -> Arc<Notify> {
        Arc::clone(&self.startup_ready)
    }

    pub fn shutdown_requested(&self) -> Arc<Notify> {
        Arc::clone(&self.shutdown_requested)
    }

    pub fn running_watch(&self) -> watch::Receiver<bool> {
        self.running_rx.clone()
    }

    pub async fn send_request(
        &self,
        method: &str,
        params: serde_json::Value,
        timeout: Duration,
    ) -> McpResult<serde_json::Value> {
        let deadline = tokio::time::Instant::now() + timeout;
        tokio::time::timeout_at(deadline, async {
            let (id, stdin, pending) = {
                let state = self.state.read().await;

                let id = state.request_id.fetch_add(1, Ordering::Relaxed) + 1;
                let stdin = state
                    .stdin
                    .clone()
                    .ok_or_else(|| McpError::TransportClosed {
                        server: self.config.server_key.clone(),
                        message: "stdin not available".into(),
                    })?;
                let pending = Arc::clone(&state.pending);
                (id, stdin, pending)
            };

            let (tx, rx) = oneshot::channel();
            pending.lock().expect("pending MCP calls").insert(id, tx);
            let mut request_guard = PendingRequest {
                pending: pending.clone(),
                id,
                cancel: None,
            };

            let request = serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": method,
                "params": params,
            });
            let mut line = match serde_json::to_string(&request) {
                Ok(s) => s,
                Err(e) => {
                    pending.lock().expect("pending MCP calls").remove(&id);
                    return Err(McpError::ProtocolError {
                        server: self.config.server_key.clone(),
                        message: format!("failed to serialize request: {e}"),
                    });
                }
            };
            line.push('\n');

            {
                let mut stdin_guard = stdin.stdin.lock().await;
                if !stdin.complete_frame.swap(false,Ordering::AcqRel) {
                    return Err(McpError::TransportClosed {server:self.config.server_key.clone(),message:"Previous request write was interrupted; reconnect before sending another request".into()});
                }
                if let Err(e) = stdin_guard.write_all(line.as_bytes()).await {
                    drop(stdin_guard);
                    pending.lock().expect("pending MCP calls").remove(&id);
                    return Err(McpError::TransportClosed {
                        server: self.config.server_key.clone(),
                        message: format!("failed to write to stdin: {e}"),
                    });
                }
                if let Err(e) = stdin_guard.flush().await {
                    drop(stdin_guard);
                    pending.lock().expect("pending MCP calls").remove(&id);
                    return Err(McpError::TransportClosed {
                        server: self.config.server_key.clone(),
                        message: format!("failed to flush stdin: {e}"),
                    });
                }
                stdin.complete_frame.store(true, Ordering::Release);
            }

            // Only a fully issued request may be cancelled (never initialize).
            request_guard.cancel = (method == "tools/call").then(|| (stdin.clone(), self.lifetime.child_token()));
            rx.await.unwrap_or_else(|_| {
                Err(McpError::TransportClosed {
                    server: self.config.server_key.clone(),
                    message: "reader task closed channel".into(),
                })
            })
        })
        .await
        .map_err(|_| McpError::CallTimeout {
            server: self.config.server_key.clone(),
            tool: method.to_string(),
            timeout_secs: timeout.as_secs(),
        })?
    }

    pub async fn send_notification(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> McpResult<()> {
        let stdin = {
            let state = self.state.read().await;
            state
                .stdin
                .clone()
                .ok_or_else(|| McpError::TransportClosed {
                    server: self.config.server_key.clone(),
                    message: "stdin not available".into(),
                })?
        };
        let notification = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        let mut line =
            serde_json::to_string(&notification).map_err(|e| McpError::ProtocolError {
                server: self.config.server_key.clone(),
                message: format!("failed to serialize notification: {e}"),
            })?;
        line.push('\n');
        let mut g = stdin.stdin.lock().await;
        if !stdin.complete_frame.swap(false, Ordering::AcqRel) {
            return Err(McpError::TransportClosed {
                server: self.config.server_key.clone(),
                message: "Previous write was interrupted; reconnect before sending a notification"
                    .into(),
            });
        }
        g.write_all(line.as_bytes())
            .await
            .map_err(|e| McpError::TransportClosed {
                server: self.config.server_key.clone(),
                message: format!("failed to write notification: {e}"),
            })?;
        g.flush().await.map_err(|e| McpError::TransportClosed {
            server: self.config.server_key.clone(),
            message: format!("failed to flush notification: {e}"),
        })?;
        stdin.complete_frame.store(true, Ordering::Release);
        Ok(())
    }

    async fn kill_process(child: &mut Child) {
        crate::process_tree::kill_child_process_tree(child).await;
    }

    fn spawn_stderr_reader(
        stderr: tokio::process::ChildStderr,
        state: std::sync::Weak<RwLock<StdioState>>,
        log_prefix: Option<String>,
        cancel: tokio_util::sync::CancellationToken,
    ) {
        tokio::spawn(async move {
            let reader = BufReader::new(stderr);
            let mut lines = reader.lines();
            loop {
                let line = tokio::select! {
                    biased;
                    _=cancel.cancelled()=>break,
                    line=lines.next_line()=>match line {Ok(Some(line))=>line,_=>break},
                };
                let Some(state) = state.upgrade() else {
                    break;
                };
                if let Some(ref prefix) = log_prefix {
                    tracing::info!(target: "plugin_stderr", "[{}] {}", prefix, line);
                }
                let mut s = state.write().await;
                s.stderr_tail.push(line);

                if s.stderr_tail.len() > 20 {
                    s.stderr_tail.remove(0);
                }
            }
        });
    }

    pub async fn ping(&self) -> McpResult<()> {
        self.send_request("ping", serde_json::json!({}), Duration::from_secs(5))
            .await?;
        Ok(())
    }
}

#[async_trait]
impl McpClient for StdioMcpClient {
    async fn connect(&self) -> McpResult<()> {
        let _lifecycle = self.lifecycle.lock().await;
        {
            let state = self.state.read().await;
            if state.connection.is_available()
                && state
                    .reader_handle
                    .as_ref()
                    .is_some_and(|handle| !handle.is_finished())
                && state.stdin.as_ref().is_some_and(|stdin| {
                    stdin.complete_frame.load(Ordering::Acquire) || stdin.stdin.try_lock().is_err()
                })
            {
                return Ok(());
            }
        }
        {
            let mut state = self.state.write().await;
            state.stdin = None;
            if let Some(mut child) = state.child.take() {
                Self::kill_process(&mut child).await;
            }
            if let Some(handle) = state.reader_handle.take() {
                handle.abort();
                let _ = handle.await;
            }
            state.connection = McpConnectionState::Connecting;
            state.last_error = None;
            state.tools.clear();
            state.init_result = None;

            state.request_id.store(0, Ordering::Relaxed);

            state.pending.lock().expect("pending MCP calls").clear();
        }

        let command = crate::path_expand::expand_tilde(&self.config.command);
        let mut cmd = Command::new(&command);
        for arg in &self.config.args {
            cmd.arg(crate::path_expand::expand_tilde(arg));
        }
        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        if self.isolated_env {
            cmd.env_clear();
        }
        cmd.kill_on_drop(true);
        for (k, v) in &self.config.env {
            cmd.env(k, v);
        }

        for name in &self.env_remove {
            cmd.env_remove(name);
        }

        if let Some(ref cwd) = self.config.cwd {
            let cwd_str = cwd.to_string_lossy();
            if !cwd_str.is_empty() {
                let expanded = crate::path_expand::expand_tilde(&cwd_str);
                cmd.current_dir(std::path::PathBuf::from(expanded));
            }
        }

        #[cfg(unix)]
        unsafe {
            #[allow(unused_imports)]
            use std::os::unix::process::CommandExt as _;
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(
                    server = %self.config.server_key,
                    raw_command = %self.config.command,
                    expanded_command = %command,
                    error_kind = ?e.kind(),
                    error = %e,
                    "stdio spawn failed"
                );
                let mut state = self.state.write().await;
                state.connection = McpConnectionState::Failed;
                let msg = e.to_string();
                state.last_error = Some(msg.clone());
                return Err(if e.kind() == std::io::ErrorKind::NotFound {
                    McpError::CommandNotFound {
                        command: command.clone(),
                    }
                } else {
                    McpError::ProtocolError {
                        server: self.config.server_key.clone(),
                        message: format!("failed to spawn process: {msg}"),
                    }
                });
            }
        };

        let stdout = child.stdout.take().ok_or_else(|| McpError::ProtocolError {
            server: self.config.server_key.clone(),
            message: "failed to capture stdout".into(),
        })?;

        let stdin_handle = child.stdin.take().ok_or_else(|| McpError::ProtocolError {
            server: self.config.server_key.clone(),
            message: "failed to capture stdin".into(),
        })?;

        if let Some(stderr) = child.stderr.take() {
            Self::spawn_stderr_reader(
                stderr,
                Arc::downgrade(&self.state),
                self.config.stderr_log_prefix.clone(),
                self.lifetime.child_token(),
            );
        }

        let pending = {
            let state = self.state.read().await;
            Arc::clone(&state.pending)
        };
        let reader_handle = ReaderTask::spawn(
            BufReader::new(stdout),
            pending,
            self.config.server_key.clone(),
        );

        {
            let mut state = self.state.write().await;
            state.stdin = Some(Arc::new(StdinWriter {
                stdin: TokioMutex::new(stdin_handle),
                complete_frame: std::sync::atomic::AtomicBool::new(true),
            }));
            state.child = Some(child);
            state.reader_handle = Some(reader_handle);
        }

        let init_result = async {
            let result = self
                .send_request(
                    "initialize",
                    serde_json::json!({
                        "protocolVersion": "2024-11-05",
                        "capabilities": {},
                        "clientInfo": {"name":"rua", "version":"0.1.0"}
                    }),
                    self.config.startup_timeout,
                )
                .await?;
            tokio::time::timeout(
                self.config.startup_timeout,
                self.send_notification("notifications/initialized", serde_json::json!({})),
            )
            .await
            .map_err(|_| McpError::StartupTimeout {
                server: self.config.server_key.clone(),
                timeout_secs: self.config.startup_timeout.as_secs(),
            })??;
            Ok::<_, McpError>(result)
        }
        .await;

        match init_result {
            Ok(result) => {
                let init = McpInitializeResult {
                    server_name: result
                        .get("serverInfo")
                        .and_then(|s| s.get("name"))
                        .and_then(|n| n.as_str())
                        .unwrap_or("unknown")
                        .to_string(),
                    server_version: result
                        .get("serverInfo")
                        .and_then(|s| s.get("version"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown")
                        .to_string(),
                    protocol_version: result
                        .get("protocolVersion")
                        .and_then(|v| v.as_str())
                        .unwrap_or("2024-11-05")
                        .to_string(),
                    capabilities: result
                        .get("capabilities")
                        .and_then(|c| {
                            serde_json::from_value::<BTreeMap<String, serde_json::Value>>(c.clone())
                                .ok()
                        })
                        .unwrap_or_default(),
                };
                let mut state = self.state.write().await;
                state.init_result = Some(init);
                state.connection = McpConnectionState::Ready;
                Ok(())
            }
            Err(err) => {
                let (child, reader_handle, classified) = {
                    let mut state = self.state.write().await;
                    state.connection = McpConnectionState::Failed;
                    let classified = match &err {
                        McpError::CallTimeout { .. } => McpError::StartupTimeout {
                            server: self.config.server_key.clone(),
                            timeout_secs: self.config.startup_timeout.as_secs(),
                        },
                        McpError::TransportClosed { message, .. } => McpError::HandshakeFailed {
                            server: self.config.server_key.clone(),
                            message: message.clone(),
                        },
                        other => McpError::HandshakeFailed {
                            server: self.config.server_key.clone(),
                            message: other.to_string(),
                        },
                    };
                    state.last_error = Some(err.to_string());
                    state.stdin = None;
                    let child = state.child.take();
                    let reader_handle = state.reader_handle.take();
                    (child, reader_handle, classified)
                };
                if let Some(mut child) = child {
                    Self::kill_process(&mut child).await;
                }
                if let Some(handle) = reader_handle {
                    handle.abort();
                    let _ = handle.await;
                }
                Err(classified)
            }
        }
    }

    async fn initialize_result(&self) -> McpResult<McpInitializeResult> {
        let state = self.state.read().await;
        state
            .init_result
            .clone()
            .ok_or_else(|| McpError::ProtocolError {
                server: self.config.server_key.clone(),
                message: "not initialized".into(),
            })
    }

    async fn list_tools(&self) -> McpResult<Vec<McpToolInfo>> {
        {
            let state = self.state.read().await;
            if !state.tools.is_empty() {
                return Ok(state.tools.clone());
            }
        }

        let result = self
            .send_request(
                "tools/list",
                serde_json::json!({}),
                self.config.call_timeout,
            )
            .await?;

        let tools_array = result
            .get("tools")
            .and_then(|t| t.as_array())
            .cloned()
            .unwrap_or_default();

        let tools: Vec<McpToolInfo> = tools_array
            .into_iter()
            .map(|t| McpToolInfo {
                server_id: mcp_manager::types::McpServerId::new(&self.config.server_key),
                name: t
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("unknown")
                    .to_string(),
                description: t
                    .get("description")
                    .and_then(|d| d.as_str())
                    .unwrap_or("")
                    .to_string(),
                input_schema: t
                    .get("inputSchema")
                    .cloned()
                    .unwrap_or(serde_json::json!({"type": "object"})),
                raw: t,
            })
            .collect();

        let mut state = self.state.write().await;
        state.tools = tools.clone();

        Ok(tools)
    }

    async fn call_tool(
        &self,
        name: &str,
        args: serde_json::Value,
        ctx: McpCallContext,
    ) -> McpResult<McpCallResult> {
        let timeout = ctx.timeout.unwrap_or(self.config.call_timeout);
        if ctx.cancel.as_ref().is_some_and(|rx| *rx.borrow()) {
            return Err(McpError::Cancelled {
                server: self.config.server_key.clone(),
            });
        }

        let call_future = self.send_request(
            "tools/call",
            serde_json::json!({
                "name": name,
                "arguments": args,
            }),
            timeout,
        );

        let result = if let Some(mut cancel_rx) = ctx.cancel {
            tokio::select! {
                biased;
                _ = async {
                    loop {
                        if *cancel_rx.borrow() { break; }
                        if cancel_rx.changed().await.is_err() {std::future::pending::<()>().await;}
                    }
                } => {
                    Err(McpError::Cancelled {
                        server: self.config.server_key.clone(),
                    })
                },
                result = call_future => result,
            }
        } else {
            call_future.await
        }?;

        let content = result
            .get("content")
            .and_then(|c| c.as_array())
            .map(|arr| {
                arr.iter()
                    .map(|item| {
                        let item_type = item.get("type").and_then(|t| t.as_str()).unwrap_or("text");
                        match item_type {
                            "text" => McpContent::Text {
                                text: item
                                    .get("text")
                                    .and_then(|t| t.as_str())
                                    .unwrap_or("")
                                    .to_string(),
                            },
                            "image" => McpContent::Image {
                                data: item
                                    .get("data")
                                    .and_then(|d| d.as_str())
                                    .unwrap_or("")
                                    .to_string(),
                                mime_type: item
                                    .get("mimeType")
                                    .and_then(|m| m.as_str())
                                    .unwrap_or("image/png")
                                    .to_string(),
                            },
                            "resource" => McpContent::Resource {
                                uri: item
                                    .get("uri")
                                    .and_then(|u| u.as_str())
                                    .unwrap_or("")
                                    .to_string(),
                                text: item.get("text").and_then(|t| t.as_str()).map(String::from),
                            },
                            _ => McpContent::Text {
                                text: serde_json::to_string(item).unwrap_or_default(),
                            },
                        }
                    })
                    .collect()
            })
            .unwrap_or_else(|| {
                vec![McpContent::Text {
                    text: serde_json::to_string(&result).unwrap_or_default(),
                }]
            });

        let is_error = result
            .get("isError")
            .and_then(|e| e.as_bool())
            .unwrap_or(false);

        Ok(McpCallResult {
            content,
            structured_content: result.get("structuredContent").cloned(),
            is_error,
        })
    }

    async fn invalidate_tool_cache(&self) {
        let mut state = self.state.write().await;
        state.tools.clear();
    }

    async fn shutdown(&self) -> McpResult<()> {
        let _lifecycle = self.lifecycle.lock().await;
        let (child, reader_handle) = {
            let mut state = self.state.write().await;
            state.connection = McpConnectionState::Stopping;

            state.stdin = None;
            state.tools.clear();
            let child = state.child.take();
            let reader_handle = state.reader_handle.take();
            (child, reader_handle)
        };

        if let Some(mut child) = child {
            Self::kill_process(&mut child).await;
        }

        if let Some(handle) = reader_handle {
            handle.abort();
            let _ = handle.await;
        }

        let mut state = self.state.write().await;
        state.connection = McpConnectionState::NotConnected;
        Ok(())
    }

    fn status(&self) -> McpServerStatus {
        match self.state.try_read() {
            Ok(state) => McpServerStatus {
                key: self.config.server_key.clone(),
                name: self.config.server_name.clone(),
                enabled: true,
                state: if state.connection.is_available()
                    && (state
                        .reader_handle
                        .as_ref()
                        .is_some_and(|h| h.is_finished())
                        || state.stdin.as_ref().is_some_and(|s| {
                            !s.complete_frame.load(Ordering::Acquire) && s.stdin.try_lock().is_ok()
                        }))
                {
                    McpConnectionState::Failed
                } else {
                    state.connection
                },
                transport_type: "stdio".to_string(),
                tools_count: state.tools.len(),
                last_error: state.last_error.clone(),
                connected_at: None,
                mobile_unavailable_reason: None,
                config_errors: Vec::new(),
            },
            Err(_) => McpServerStatus {
                key: self.config.server_key.clone(),
                name: self.config.server_name.clone(),
                enabled: true,
                state: McpConnectionState::Connecting,
                transport_type: "stdio".to_string(),
                tools_count: 0,
                last_error: None,
                connected_at: None,
                mobile_unavailable_reason: None,
                config_errors: Vec::new(),
            },
        }
    }
}

impl Drop for StdioMcpClient {
    fn drop(&mut self) {
        self.lifetime.cancel();
        if let Ok(mut state) = self.state.try_write() {
            if let Some(handle) = state.reader_handle.take() {
                handle.abort();
            }
            if let Some(ref mut child) = state.child {
                crate::process_tree::start_kill_child_process_tree(child);
            }
        }
    }
}

impl Drop for StdioState {
    fn drop(&mut self) {
        if let Some(handle) = self.reader_handle.take() {
            handle.abort();
        }
        if let Some(child) = self.child.as_mut() {
            crate::process_tree::start_kill_child_process_tree(child);
        }
    }
}
