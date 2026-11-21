use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use rua_core::tool::ToolResult;
use rua_device_network::{DeviceSessionCommandContext, DeviceSessionManager};
use rua_tool::{CommandExecutor, CommandOutput, CommandRequest, RuntimeCommandExecutor};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use super::chat_file_read_admission::{
    FileReadAdmission, TargetFileReadContext, TargetReadRejection,
};
use super::device_read_check::{DeviceReadCheckReply, DeviceReadCheckRequest};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatCommandContext {
    pub execution_id: String,
    pub call: DeviceReadCheckRequest,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatCommandCommand {
    pub context: ChatCommandContext,
    pub action: ChatCommandAction,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatCommandAction {
    Prepare,
    Start,
    Status,
    Cancel,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatCommandPreparation {
    pub execution_id: String,
    pub device_id: String,
    pub public_cwd: String,
    pub policy: DeviceReadCheckReply,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatCommandState {
    Prepared,
    Running,
    Succeeded,
    Failed,
    Cancelled,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatCommandSnapshot {
    pub execution_id: String,
    pub device_id: String,
    pub public_cwd: String,
    pub state: ChatCommandState,
    pub result: Option<ToolResult>,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ChatCommandResponse {
    Prepared {
        preparation: Box<ChatCommandPreparation>,
    },
    Operation {
        operation: Box<ChatCommandSnapshot>,
    },
    Policy {
        response: DeviceReadCheckReply,
    },
}

const COMMAND_OUTPUT_BYTES: usize = 64 * 1024;

/// Execution records belong to the target runtime, independently of RPC futures.
/// The same cancel token, runner session and completion boundary can be adopted by
/// Task Runtime without starting another process.
struct CommandExecution {
    context: ChatCommandContext,
    host_cwd: PathBuf,
    cancel: CancellationToken,
    finished: CancellationToken,
    runner_session: Arc<rua_task::TaskRunnerSession>,
    state: tokio::sync::Mutex<CommandExecutionState>,
}

struct CommandExecutionState {
    snapshot: ChatCommandSnapshot,
    connection: Option<DeviceSessionCommandContext>,
    last_used: Instant,
}

#[derive(Clone)]
pub struct ChatCommandService {
    rua_root: PathBuf,
    local_device_id: String,
    target: Arc<TargetFileReadContext>,
    sessions: Arc<RwLock<Option<DeviceSessionManager>>>,
    executions: Arc<Mutex<HashMap<String, Arc<CommandExecution>>>>,
}

impl ChatCommandService {
    pub fn new(
        rua_root: PathBuf,
        local_device_id: String,
        target: Arc<TargetFileReadContext>,
        sessions: Arc<RwLock<Option<DeviceSessionManager>>>,
    ) -> Self {
        Self {
            rua_root,
            local_device_id,
            target,
            sessions,
            executions: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn handle(
        &self,
        command: ChatCommandCommand,
        connection: DeviceSessionCommandContext,
    ) -> Result<ChatCommandResponse, String> {
        let context = command.context;
        if context.execution_id.is_empty()
            || context.execution_id.len() > 128
            || !context
                .execution_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            || context.call.source_device_id != connection.peer_device_id()
            || context.call.source_device_id == self.local_device_id
            || context.call.target_device_id != self.local_device_id
            || context.call.tool_name != "bash"
        {
            return Err("Invalid remote command identity".into());
        }
        let existing = {
            let mut executions = self.executions.lock().expect("remote executions lock");
            executions.retain(|_, execution| {
                execution.state.try_lock().map_or(true, |state| {
                    matches!(
                        state.snapshot.state,
                        ChatCommandState::Prepared | ChatCommandState::Running
                    ) || state.last_used.elapsed() < Duration::from_secs(600)
                })
            });
            executions.get(&context.execution_id).cloned()
        };
        if let Some(execution) = &existing {
            if !same_invocation(&execution.context, &context) {
                return Err("Execution identity belongs to a different Chat invocation".into());
            }
        }
        match command.action {
            ChatCommandAction::Prepare => self.prepare(context, connection, existing).await,
            ChatCommandAction::Start => {
                let Some(execution) = existing else {
                    return Ok(self.missing(&context));
                };
                self.start(&context, connection, execution).await
            }
            ChatCommandAction::Status => match existing {
                Some(execution) => Ok(operation(&execution).await),
                None => Ok(self.missing(&context)),
            },
            ChatCommandAction::Cancel => match existing {
                Some(execution) => {
                    cancel_execution(&execution).await;
                    Ok(operation(&execution).await)
                }
                None => Ok(self.missing(&context)),
            },
        }
    }

    fn missing(&self, context: &ChatCommandContext) -> ChatCommandResponse {
        ChatCommandResponse::Operation { operation: Box::new(ChatCommandSnapshot {
            execution_id: context.execution_id.clone(), device_id: self.local_device_id.clone(),
            public_cwd: String::new(), state: ChatCommandState::Unknown,
            result: Some(ToolResult::error(&context.call.tool_call_id,
                "The target has no execution record. Execution and side effects are unconfirmed; the command was not replayed.")),
            extra: Default::default(),
        }) }
    }

    fn filesystem(&self) -> Result<rua_tool::WorkspaceFs, String> {
        rua_tool::WorkspaceFs::for_execution(
            &self.rua_root,
            rua_core::runtime_layout::RuntimeLayout::from_current_process(None),
        )
        .map_err(|error| error.to_string())
    }

    fn resolve_directory(&self, context: &ChatCommandContext) -> Result<(String, PathBuf), String> {
        let layout = rua_core::runtime_layout::RuntimeLayout::from_current_process(None);
        let public = match context.call.arguments["cwd"].as_str() {
            Some(path) => PathBuf::from(path),
            None => match &layout.linux_compat {
                Some(guest) => guest.guest_home.clone(),
                None => dirs::home_dir().ok_or("Target environment has no user home directory")?,
            },
        };
        let fs = self.filesystem()?;
        let host = fs.resolve(&public).map_err(|error| error.to_string())?;
        if !host.is_dir() {
            return Err("Command working directory is not a directory".into());
        }
        let public = fs
            .public_path_from_host(&host)
            .and_then(|path| path.to_str().map(str::to_owned))
            .ok_or("Command working directory has no public target path")?;
        let request = self.command_request(context, host.clone(), None);
        let plan = RuntimeCommandExecutor::shell_launch_plan_for_request(&request);
        if !plan.diagnostics.is_empty() {
            return Err(plan.diagnostics.join("; "));
        }
        plan.process().map_err(|error| error.to_string())?;
        Ok((public, host))
    }

    fn command_request(
        &self,
        context: &ChatCommandContext,
        host_cwd: PathBuf,
        session: Option<Arc<rua_task::TaskRunnerSession>>,
    ) -> CommandRequest {
        let store = rua_core::ChatStore::resolve(&self.rua_root, context.call.address.clone());
        CommandRequest {
            command: context.call.arguments["command"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
            working_dir: host_cwd,
            tool_result_store_dir: store.root().join("tool_call_logs"),
            timeout: Duration::ZERO,
            env: Vec::new(),
            env_remove: vec![
                "AWS_SECRET_ACCESS_KEY".into(),
                "OPENAI_API_KEY".into(),
                "ANTHROPIC_API_KEY".into(),
            ],
            raw_capture_max_bytes: COMMAND_OUTPUT_BYTES,
            raw_capture_id: Some(context.execution_id.clone()),
            task_session: session,
        }
    }

    async fn prepare(
        &self,
        context: ChatCommandContext,
        connection: DeviceSessionCommandContext,
        existing: Option<Arc<CommandExecution>>,
    ) -> Result<ChatCommandResponse, String> {
        if connection.is_closed() {
            return Ok(policy(DeviceReadCheckReply::Unavailable));
        }
        if let Err(rejection) = self
            .target
            .check_command_authority(&context.call.as_target_read())
            .await
        {
            return Ok(policy(rejection_reply(rejection)));
        }
        let execution = if let Some(execution) = existing {
            let state = execution.state.lock().await;
            if state.snapshot.state != ChatCommandState::Prepared {
                return Ok(snapshot(&state));
            }
            if state
                .connection
                .as_ref()
                .is_none_or(|owner| owner.generation() != connection.generation())
            {
                return Ok(policy(DeviceReadCheckReply::Unavailable));
            }
            drop(state);
            execution
        } else {
            let (public_cwd, host_cwd) = self.resolve_directory(&context)?;
            let execution = Arc::new(CommandExecution {
                context: context.clone(),
                host_cwd,
                cancel: CancellationToken::new(),
                finished: CancellationToken::new(),
                runner_session: rua_task::TaskRunnerSession::new(),
                state: tokio::sync::Mutex::new(CommandExecutionState {
                    snapshot: ChatCommandSnapshot {
                        execution_id: context.execution_id.clone(),
                        device_id: self.local_device_id.clone(),
                        public_cwd,
                        state: ChatCommandState::Prepared,
                        result: None,
                        extra: Default::default(),
                    },
                    connection: Some(connection.clone()),
                    last_used: Instant::now(),
                }),
            });
            let inserted = {
                let mut executions = self.executions.lock().expect("remote executions lock");
                if executions.contains_key(&context.execution_id) {
                    false
                } else {
                    executions.insert(context.execution_id.clone(), execution.clone());
                    true
                }
            };
            if !inserted {
                return Err(
                    "Command preparation is already in progress; query its execution identity"
                        .into(),
                );
            }
            let watched = execution.clone();
            tokio::spawn(async move {
                tokio::select! {
                    biased;
                    _ = watched.finished.cancelled() => return,
                    _ = connection.closed() => { cancel_execution(&watched).await; return; },
                    _ = tokio::time::sleep(Duration::from_secs(600)) => {
                        let mut state = watched.state.lock().await;
                        if state.snapshot.state == ChatCommandState::Prepared {
                            watched.cancel.cancel();
                            finish_before_start(&watched, &mut state);
                            return;
                        }
                    }
                }
                // A wait limit is not an execution deadline. Once started, only
                // actual connection loss or explicit cancellation stops the work.
                tokio::select! {
                    biased;
                    _ = watched.finished.cancelled() => {},
                    _ = connection.closed() => cancel_execution(&watched).await,
                }
            });
            execution
        };
        let mut state = execution.state.lock().await;
        state.last_used = Instant::now();
        let admission = self
            .target
            .inspect_command(
                &context.call.as_target_read(),
                &state.snapshot.public_cwd,
                false,
            )
            .await;
        let policy = match admission {
            Ok(FileReadAdmission::Allowed) => DeviceReadCheckReply::Checked,
            Ok(FileReadAdmission::RequiresUserConfirmation) => {
                DeviceReadCheckReply::ConfirmationRequired
            }
            Err(rejection) => rejection_reply(rejection),
        };
        Ok(ChatCommandResponse::Prepared {
            preparation: Box::new(ChatCommandPreparation {
                execution_id: context.execution_id,
                device_id: self.local_device_id.clone(),
                public_cwd: state.snapshot.public_cwd.clone(),
                policy,
                extra: Default::default(),
            }),
        })
    }

    async fn start(
        &self,
        context: &ChatCommandContext,
        connection: DeviceSessionCommandContext,
        execution: Arc<CommandExecution>,
    ) -> Result<ChatCommandResponse, String> {
        let mut state = execution.state.lock().await;
        state.last_used = Instant::now();
        if state.snapshot.state != ChatCommandState::Prepared {
            return Ok(snapshot(&state));
        }
        if connection.is_closed()
            || !state.connection.as_ref().is_some_and(|owner| {
                owner.generation() == connection.generation() && !owner.is_closed()
            })
        {
            return Ok(policy(DeviceReadCheckReply::Unavailable));
        }
        // Claim the original Chat decision only at the real launch boundary.
        let confirmed = if let Some(confirmation) = &context.call.confirmation {
            let manager = self.sessions.read().expect("Device sessions lock").clone();
            let Some(manager) = manager else {
                return Ok(policy(DeviceReadCheckReply::Unavailable));
            };
            if !context.call.verify_at_origin(&manager, confirmation).await {
                return Ok(policy(DeviceReadCheckReply::Unavailable));
            }
            true
        } else {
            false
        };
        match self
            .target
            .inspect_command(
                &context.call.as_target_read(),
                &state.snapshot.public_cwd,
                confirmed,
            )
            .await
        {
            Ok(FileReadAdmission::Allowed) => {}
            Ok(FileReadAdmission::RequiresUserConfirmation) => {
                return Ok(policy(DeviceReadCheckReply::ConfirmationRequired))
            }
            Err(rejection) => return Ok(policy(rejection_reply(rejection))),
        }
        let fs = self.filesystem()?;
        if fs
            .resolve(&state.snapshot.public_cwd)
            .map_err(|error| error.to_string())?
            != execution.host_cwd
        {
            return Err("Prepared command directory changed before execution".into());
        }
        if execution.cancel.is_cancelled() || connection.is_closed() {
            finish_before_start(&execution, &mut state);
            return Ok(snapshot(&state));
        }
        state.snapshot.state = ChatCommandState::Running;
        let response = snapshot(&state);
        let service = self.clone();
        let request = self.command_request(
            context,
            execution.host_cwd.clone(),
            Some(execution.runner_session.clone()),
        );
        drop(state);
        tokio::spawn(async move {
            service.execute(execution, request).await;
        });
        Ok(response)
    }

    async fn execute(&self, execution: Arc<CommandExecution>, request: CommandRequest) {
        let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(execution.cancel.is_cancelled());
        let cancel = execution.cancel.clone();
        let finished = execution.finished.clone();
        let bridge = tokio::spawn(async move {
            tokio::select! {
                _ = finished.cancelled() => {},
                _ = cancel.cancelled() => { let _ = cancel_tx.send(true); },
            }
        });
        let executing = tokio::spawn(async move {
            RuntimeCommandExecutor::new()
                .execute(request, cancel_rx)
                .await
        });
        let output = executing.await;
        let (state, result) = match output {
            Ok(Ok(output)) => self.command_result(&execution, output),
            Ok(Err(error)) => {
                let state = match &error {
                    rua_tool::CommandError::SpawnFailed(_)
                    | rua_tool::CommandError::Disabled(_) => ChatCommandState::Failed,
                    _ => ChatCommandState::Unknown,
                };
                (
                    state,
                    ToolResult::error(&execution.context.call.tool_call_id, error.to_string()),
                )
            }
            Err(error) => (
                ChatCommandState::Unknown,
                ToolResult::error(
                    &execution.context.call.tool_call_id,
                    format!("Target execution terminal status is unconfirmed: {error}"),
                ),
            ),
        };
        let mut record = execution.state.lock().await;
        record.snapshot.state = state;
        record.snapshot.result = Some(result);
        record.connection = None;
        record.last_used = Instant::now();
        execution.finished.cancel();
        bridge.abort();
    }

    fn command_result(
        &self,
        execution: &CommandExecution,
        output: CommandOutput,
    ) -> (ChatCommandState, ToolResult) {
        let state = if output.cancelled {
            ChatCommandState::Cancelled
        } else if output.exit_code == Some(0) {
            ChatCommandState::Succeeded
        } else if output.exit_code.is_some() {
            ChatCommandState::Failed
        } else {
            ChatCommandState::Unknown
        };
        let mut text = format!("[stdout]\n{}\n[stderr]\n{}", output.stdout, output.stderr);
        if text.len() > COMMAND_OUTPUT_BYTES {
            let mut end = COMMAND_OUTPUT_BYTES;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            text.truncate(end);
        }
        let mut references = String::new();
        let mut remote_capture = None;
        if let Some(capture) = output.raw_capture {
            if let Ok(fs) = self.filesystem() {
                let public = |path: &Option<PathBuf>| {
                    path.as_ref()
                        .and_then(|path| fs.public_path_from_host(path))
                };
                let stdout = public(&capture.stdout_path);
                let stderr = public(&capture.stderr_path);
                for (stream, path) in [("stdout", &stdout), ("stderr", &stderr)] {
                    if let Some(path) = path {
                        references.push_str(&format!(
                            "\n{stream} capture: Device {} path {}",
                            self.local_device_id,
                            path.display()
                        ));
                    }
                }
                remote_capture = Some(serde_json::json!({
                    "device_id":self.local_device_id,
                    "stdout_path":stdout, "stderr_path":stderr,
                    "stdout_total_bytes":capture.stdout_total_bytes, "stderr_total_bytes":capture.stderr_total_bytes,
                    "stdout_omitted_bytes":capture.stdout_omitted_bytes, "stderr_omitted_bytes":capture.stderr_omitted_bytes,
                }));
            }
        }
        let limit = COMMAND_OUTPUT_BYTES.saturating_sub(references.len());
        if text.len() > limit {
            let mut end = limit;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            text.truncate(end);
        }
        text.push_str(&references);
        let mut result = if state == ChatCommandState::Succeeded {
            ToolResult::text(&execution.context.call.tool_call_id, text)
        } else {
            ToolResult::error(&execution.context.call.tool_call_id, text)
        };
        result.duration_ms = Some(output.duration_ms);
        result
            .metadata
            .insert("exit_code".into(), serde_json::json!(output.exit_code));
        result
            .metadata
            .insert("cancelled".into(), output.cancelled.into());
        result
            .metadata
            .insert("timed_out".into(), output.timed_out.into());
        if let Some(capture) = remote_capture {
            result.metadata.insert("remote_capture".into(), capture);
        }
        (state, result)
    }
}

fn same_invocation(left: &ChatCommandContext, right: &ChatCommandContext) -> bool {
    let mut left = left.clone();
    let mut right = right.clone();
    left.call.confirmation = None;
    right.call.confirmation = None;
    left == right
}

fn snapshot(state: &CommandExecutionState) -> ChatCommandResponse {
    ChatCommandResponse::Operation {
        operation: Box::new(state.snapshot.clone()),
    }
}

async fn operation(execution: &CommandExecution) -> ChatCommandResponse {
    let mut state = execution.state.lock().await;
    state.last_used = Instant::now();
    snapshot(&state)
}

fn finish_before_start(execution: &CommandExecution, state: &mut CommandExecutionState) {
    state.snapshot.state = ChatCommandState::Cancelled;
    state.snapshot.result = Some(ToolResult::error(
        &execution.context.call.tool_call_id,
        "Command was cancelled before it started.",
    ));
    state.connection = None;
    state.last_used = Instant::now();
    execution.finished.cancel();
}

async fn cancel_execution(execution: &CommandExecution) {
    execution.cancel.cancel();
    let mut state = execution.state.lock().await;
    if state.snapshot.state == ChatCommandState::Prepared {
        finish_before_start(execution, &mut state);
    } else if state.snapshot.state == ChatCommandState::Running {
        state
            .snapshot
            .extra
            .insert("cancel_requested".into(), true.into());
    }
}

fn policy(response: DeviceReadCheckReply) -> ChatCommandResponse {
    ChatCommandResponse::Policy { response }
}

fn rejection_reply(rejection: TargetReadRejection) -> DeviceReadCheckReply {
    match rejection {
        TargetReadRejection::Denied => DeviceReadCheckReply::Denied,
        TargetReadRejection::Unavailable | TargetReadRejection::FileUnavailable => {
            DeviceReadCheckReply::Unavailable
        }
    }
}
