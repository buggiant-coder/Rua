use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use rua_core::{ChatAddress, ChatScope};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HandlerKind {
    Command,
    Plugin { plugin_id: String },
    Native,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HandlerExposure {
    pub data: serde_json::Value,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(skip)]
    pub sensitive_values: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct HandlerInvocation {
    pub invocation_id: String,
    pub kind: HandlerKind,
    pub authority_device_id: String,
    pub chat_address: ChatAddress,
    pub scope: ChatScope,
    pub resource_id: String,
    pub executable: Option<String>,
    pub program: Option<String>,
    pub args: Vec<String>,
    pub working_directory: PathBuf,
    pub timeout: Duration,
    pub exposure: HandlerExposure,
    pub cancellation: CancellationToken,
    pub remaining_budget: Duration,
    pub available_resources: Vec<String>,
    pub required_resources: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct HandlerAdapterInvocation {
    pub invocation_id: String,
    pub authority_device_id: String,
    pub chat_address: ChatAddress,
    pub scope: ChatScope,
    pub resource_id: String,
    pub executable: Option<String>,
    pub program: Option<String>,
    pub args: Vec<String>,
    pub working_directory: PathBuf,
    pub timeout: Duration,
    pub exposure: HandlerExposure,
    pub cancellation: CancellationToken,
    pub remaining_budget: Duration,
    pub available_resources: Vec<String>,
    pub required_resources: Vec<String>,
}

impl HandlerAdapterInvocation {
    pub fn required_callback(&self, label: &str) -> Result<&str, HandlerAdapterError> {
        self.executable.as_deref().ok_or_else(|| {
            HandlerAdapterError::UnavailableRequirements(vec![format!("{label} callback")])
        })
    }
}

impl From<HandlerInvocation> for HandlerAdapterInvocation {
    fn from(invocation: HandlerInvocation) -> Self {
        Self {
            invocation_id: invocation.invocation_id,
            authority_device_id: invocation.authority_device_id,
            chat_address: invocation.chat_address,
            scope: invocation.scope,
            resource_id: invocation.resource_id,
            executable: invocation.executable,
            program: invocation.program,
            args: invocation.args,
            working_directory: invocation.working_directory,
            timeout: invocation.timeout,
            exposure: invocation.exposure,
            cancellation: invocation.cancellation,
            remaining_budget: invocation.remaining_budget,
            available_resources: invocation.available_resources,
            required_resources: invocation.required_resources,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HandlerAdapterError {
    Launch(String),
    MalformedOutcome(String),
    UnavailableRequirements(Vec<String>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HandlerExecutionFailureKind {
    Launch,
    Timeout,
    MalformedOutcome,
    Cancelled,
    UnavailableRequirements,
    BudgetExhausted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandlerExecutionFailure {
    pub kind: HandlerExecutionFailureKind,
    pub diagnostic: String,
    pub attempts: u8,
}

impl std::fmt::Display for HandlerExecutionFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.diagnostic)
    }
}

impl std::error::Error for HandlerExecutionFailure {}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HandlerExecutionResult {
    pub outcome: serde_json::Value,
    pub diagnostics: Vec<String>,
    pub attempts: u8,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HandlerAdapterResult {
    pub outcome: serde_json::Value,
    pub diagnostics: Vec<String>,
}

#[async_trait]
pub trait CommandHandler: Send + Sync {
    async fn execute(
        &self,
        invocation: HandlerAdapterInvocation,
    ) -> Result<HandlerAdapterResult, HandlerAdapterError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandExecutionResource {
    pub scope: ChatScope,
    pub resource_id: String,
    pub root: PathBuf,
}

pub struct StrictCommandHandler {
    authority_device_id: String,
    resources: Vec<CommandExecutionResource>,
}

impl StrictCommandHandler {
    pub fn new(
        authority_device_id: impl Into<String>,
        resources: Vec<CommandExecutionResource>,
    ) -> Self {
        Self {
            authority_device_id: authority_device_id.into(),
            resources,
        }
    }

    fn validate(&self, invocation: &HandlerAdapterInvocation) -> Result<(), HandlerAdapterError> {
        if invocation.authority_device_id != self.authority_device_id {
            return Err(HandlerAdapterError::UnavailableRequirements(vec![format!(
                "authority device '{}'",
                invocation.authority_device_id
            )]));
        }
        let resource = self
            .resources
            .iter()
            .find(|resource| {
                resource.scope == invocation.scope && resource.resource_id == invocation.resource_id
            })
            .ok_or_else(|| {
                HandlerAdapterError::UnavailableRequirements(vec![format!(
                    "{} resource '{}'",
                    invocation.scope.kind(),
                    invocation.resource_id
                )])
            })?;
        let program = invocation.program.as_deref().ok_or_else(|| {
            HandlerAdapterError::UnavailableRequirements(vec!["command program".to_string()])
        })?;
        require_absolute_file("program", program)?;
        if !invocation.working_directory.is_absolute() {
            return Err(HandlerAdapterError::UnavailableRequirements(vec![
                "absolute working directory".to_string(),
            ]));
        }
        let root = std::fs::canonicalize(&resource.root).map_err(|error| {
            HandlerAdapterError::UnavailableRequirements(vec![format!(
                "resource root '{}': {error}",
                resource.root.display()
            )])
        })?;
        let working_directory =
            std::fs::canonicalize(&invocation.working_directory).map_err(|error| {
                HandlerAdapterError::UnavailableRequirements(vec![format!(
                    "working directory '{}': {error}",
                    invocation.working_directory.display()
                )])
            })?;
        if !working_directory.starts_with(&root) {
            return Err(HandlerAdapterError::UnavailableRequirements(vec![format!(
                "resource root '{}'",
                resource.root.display()
            )]));
        }
        Ok(())
    }
}

#[derive(Serialize)]
struct CommandInvocationPayload<'a> {
    invocation_id: &'a str,
    authority_device_id: &'a str,
    chat_address: &'a ChatAddress,
    scope: &'a ChatScope,
    resource_id: &'a str,
    program: &'a str,
    args: &'a [String],
    working_directory: &'a Path,
    timeout_ms: u64,
    remaining_budget_ms: u64,
    exposure: &'a HandlerExposure,
    available_resources: &'a [String],
    required_resources: &'a [String],
}

#[derive(Deserialize)]
struct CommandOutcome {
    outcome: serde_json::Value,
    #[serde(default)]
    diagnostics: Vec<String>,
    #[serde(flatten)]
    _extra: BTreeMap<String, serde_json::Value>,
}

#[async_trait]
impl CommandHandler for StrictCommandHandler {
    async fn execute(
        &self,
        invocation: HandlerAdapterInvocation,
    ) -> Result<HandlerAdapterResult, HandlerAdapterError> {
        self.validate(&invocation)?;
        let program = invocation.program.as_deref().ok_or_else(|| {
            HandlerAdapterError::UnavailableRequirements(vec!["command program".to_string()])
        })?;
        let payload = serde_json::to_vec(&CommandInvocationPayload {
            invocation_id: &invocation.invocation_id,
            authority_device_id: &invocation.authority_device_id,
            chat_address: &invocation.chat_address,
            scope: &invocation.scope,
            resource_id: &invocation.resource_id,
            program,
            args: &invocation.args,
            working_directory: &invocation.working_directory,
            timeout_ms: invocation.timeout.as_millis().min(u128::from(u64::MAX)) as u64,
            remaining_budget_ms: invocation
                .remaining_budget
                .as_millis()
                .min(u128::from(u64::MAX)) as u64,
            exposure: &invocation.exposure,
            available_resources: &invocation.available_resources,
            required_resources: &invocation.required_resources,
        })
        .map_err(|error| {
            HandlerAdapterError::MalformedOutcome(format!("serialize command invocation: {error}"))
        })?;
        let mut command = Command::new(program);
        command
            .args(&invocation.args)
            .current_dir(&invocation.working_directory)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command
            .spawn()
            .map_err(|error| HandlerAdapterError::Launch(error.to_string()))?;
        let mut stdin = child.stdin.take().ok_or_else(|| {
            HandlerAdapterError::Launch("command stdin was not available".to_string())
        })?;
        stdin
            .write_all(&payload)
            .await
            .map_err(|error| HandlerAdapterError::Launch(error.to_string()))?;
        stdin
            .shutdown()
            .await
            .map_err(|error| HandlerAdapterError::Launch(error.to_string()))?;
        drop(stdin);
        let output = child
            .wait_with_output()
            .await
            .map_err(|error| HandlerAdapterError::Launch(error.to_string()))?;
        if !output.status.success() {
            return Err(HandlerAdapterError::MalformedOutcome(format!(
                "command exited with status {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        let outcome: CommandOutcome = serde_json::from_slice(&output.stdout).map_err(|error| {
            HandlerAdapterError::MalformedOutcome(format!(
                "command returned malformed outcome: {error}"
            ))
        })?;
        Ok(HandlerAdapterResult {
            outcome: outcome.outcome,
            diagnostics: outcome.diagnostics,
        })
    }
}

fn require_absolute_file(label: &str, value: &str) -> Result<(), HandlerAdapterError> {
    let path = Path::new(value);
    if !path.is_absolute() || !path.is_file() {
        return Err(HandlerAdapterError::UnavailableRequirements(vec![format!(
            "absolute {label} '{value}'"
        )]));
    }
    Ok(())
}

#[async_trait]
pub trait NativeHandler: Send + Sync {
    async fn execute(
        &self,
        invocation: HandlerAdapterInvocation,
    ) -> Result<HandlerAdapterResult, HandlerAdapterError>;
}

#[async_trait]
pub trait PluginHandler: Send + Sync {
    async fn execute(
        &self,
        plugin_id: &str,
        invocation: HandlerAdapterInvocation,
    ) -> Result<HandlerAdapterResult, HandlerAdapterError>;
}

pub struct HandlerExecutor {
    command: Arc<dyn CommandHandler>,
    native: Arc<dyn NativeHandler>,
    plugin: Arc<dyn PluginHandler>,
}

impl HandlerExecutor {
    pub fn new(
        command: Arc<dyn CommandHandler>,
        native: Arc<dyn NativeHandler>,
        plugin: Arc<dyn PluginHandler>,
    ) -> Self {
        Self {
            command,
            native,
            plugin,
        }
    }

    pub async fn execute(
        &self,
        invocation: HandlerInvocation,
    ) -> Result<HandlerExecutionResult, HandlerExecutionFailure> {
        if invocation.cancellation.is_cancelled() {
            return Err(failure(
                HandlerExecutionFailureKind::Cancelled,
                "Hook invocation was cancelled",
                0,
            ));
        }
        if invocation.remaining_budget.is_zero() {
            return Err(failure(
                HandlerExecutionFailureKind::BudgetExhausted,
                "shared Turn budget is exhausted",
                0,
            ));
        }
        let unavailable = invocation
            .required_resources
            .iter()
            .filter(|required| !invocation.available_resources.contains(required))
            .cloned()
            .collect::<Vec<_>>();
        if !unavailable.is_empty() {
            return Err(failure(
                HandlerExecutionFailureKind::UnavailableRequirements,
                format!("unavailable requirements: {}", unavailable.join(", ")),
                0,
            ));
        }
        let kind = invocation.kind.clone();
        let adapter_invocation = HandlerAdapterInvocation::from(invocation);
        let sensitive_values = adapter_invocation.exposure.sensitive_values.clone();
        let cancellation = adapter_invocation.cancellation.clone();
        let handler_timeout = adapter_invocation.timeout;
        let remaining_budget = adapter_invocation.remaining_budget;
        let deadline = handler_timeout.min(remaining_budget);
        let budget_limited = remaining_budget < handler_timeout;
        let dispatch = async {
            match kind {
                HandlerKind::Command => self.command.execute(adapter_invocation).await,
                HandlerKind::Plugin { plugin_id } => {
                    self.plugin.execute(&plugin_id, adapter_invocation).await
                }
                HandlerKind::Native => self.native.execute(adapter_invocation).await,
            }
        };
        tokio::pin!(dispatch);
        let outcome = tokio::select! {
            biased;
            _ = cancellation.cancelled() => {
                return Err(failure(
                    HandlerExecutionFailureKind::Cancelled,
                    "Hook invocation was cancelled",
                    1,
                ));
            }
            result = &mut dispatch => result.map_err(|error| map_adapter_error(error, &sensitive_values))?,
            _ = tokio::time::sleep(deadline) => {
                let (kind, diagnostic) = if budget_limited {
                    (
                        HandlerExecutionFailureKind::BudgetExhausted,
                        "shared Turn budget was exhausted during Hook execution",
                    )
                } else {
                    (
                        HandlerExecutionFailureKind::Timeout,
                        "Hook invocation timed out",
                    )
                };
                return Err(failure(kind, diagnostic, 1));
            }
        };

        Ok(HandlerExecutionResult {
            outcome: outcome.outcome,
            diagnostics: outcome
                .diagnostics
                .iter()
                .map(|diagnostic| redact_diagnostic(diagnostic, &sensitive_values))
                .collect(),
            attempts: 1,
        })
    }
}

fn map_adapter_error(
    error: HandlerAdapterError,
    sensitive_values: &[String],
) -> HandlerExecutionFailure {
    let (kind, diagnostic) = match error {
        HandlerAdapterError::Launch(diagnostic) => {
            (HandlerExecutionFailureKind::Launch, diagnostic)
        }
        HandlerAdapterError::MalformedOutcome(diagnostic) => {
            (HandlerExecutionFailureKind::MalformedOutcome, diagnostic)
        }
        HandlerAdapterError::UnavailableRequirements(requirements) => (
            HandlerExecutionFailureKind::UnavailableRequirements,
            format!("unavailable requirements: {}", requirements.join(", ")),
        ),
    };
    failure(kind, redact_diagnostic(&diagnostic, sensitive_values), 1)
}

fn redact_diagnostic(diagnostic: &str, sensitive_values: &[String]) -> String {
    sensitive_values
        .iter()
        .filter(|value| !value.is_empty())
        .fold(
            rua_security::redact_secrets(diagnostic),
            |redacted, value| redacted.replace(value, "***"),
        )
}

fn failure(
    kind: HandlerExecutionFailureKind,
    diagnostic: impl Into<String>,
    attempts: u8,
) -> HandlerExecutionFailure {
    HandlerExecutionFailure {
        kind,
        diagnostic: diagnostic.into(),
        attempts,
    }
}
