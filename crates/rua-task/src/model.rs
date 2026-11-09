use std::fmt;

use chrono::{DateTime, Utc};
use rua_core::ChatAddress;
use serde::{Deserialize, Serialize};

pub type TaskId = String;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TaskOwner {
    pub chat_address: ChatAddress,
}

impl TaskOwner {
    pub fn new(agent_id: impl Into<String>, chat_id: impl Into<String>) -> Self {
        Self {
            chat_address: ChatAddress::agent(agent_id.into(), chat_id.into()),
        }
    }

    pub fn from_address(chat_address: ChatAddress) -> Self {
        Self { chat_address }
    }

    pub fn chat_id(&self) -> &str {
        self.chat_address.chat_id().as_str()
    }

    pub fn agent_id(&self) -> Option<&str> {
        self.chat_address.agent_id().map(|id| id.as_str())
    }

    pub fn identity_key(&self) -> String {
        self.chat_address.identity_key()
    }

    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.chat_address.scope().owner_id().trim().is_empty() {
            return Err("owner Scope id must not be empty".to_string());
        }
        if self.chat_id().trim().is_empty() {
            return Err("owner chat_id must not be empty".to_string());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskOrigin {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskContextRef {
    pub uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_device_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskRunnerKind {
    Tool,
    RuaAgent,
    SubAgent,
    Acp,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskLiveOutputProjection {
    ModelVisible,
    #[default]
    ActivityOnly,
}

impl fmt::Display for TaskRunnerKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::Tool => "tool",
            Self::RuaAgent => "rua_agent",
            Self::SubAgent => "sub_agent",
            Self::Acp => "acp",
        };
        f.write_str(value)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TaskRunnerSpec {
    Tool {
        tool_name: String,
        #[serde(default)]
        arguments: serde_json::Value,
    },
    RuaAgent {
        agent_id: String,
    },
    SubAgent {},
    Acp {
        runner: String,
    },
}

impl TaskRunnerSpec {
    pub fn kind(&self) -> TaskRunnerKind {
        match self {
            Self::Tool { .. } => TaskRunnerKind::Tool,
            Self::RuaAgent { .. } => TaskRunnerKind::RuaAgent,
            Self::SubAgent {} => TaskRunnerKind::SubAgent,
            Self::Acp { .. } => TaskRunnerKind::Acp,
        }
    }

    pub fn live_output_projection(&self) -> TaskLiveOutputProjection {
        match self {
            Self::Tool { .. } => TaskLiveOutputProjection::ModelVisible,
            Self::RuaAgent { .. } | Self::SubAgent {} | Self::Acp { .. } => {
                TaskLiveOutputProjection::ActivityOnly
            }
        }
    }

    pub fn display_name(&self) -> &str {
        match self {
            Self::Tool { tool_name, .. } => tool_name,
            Self::RuaAgent { agent_id } => agent_id,
            Self::SubAgent {} => "subagent",
            Self::Acp { runner } => runner,
        }
    }

    pub(crate) fn validate(&self) -> Result<(), String> {
        match self {
            Self::Tool {
                tool_name,
                arguments,
            } => {
                if tool_name.trim().is_empty() {
                    return Err("tool runner requires tool_name".to_string());
                }
                if !arguments.is_object() && !arguments.is_null() {
                    return Err("tool runner arguments must be an object".to_string());
                }
            }
            Self::RuaAgent { agent_id } => {
                if agent_id.trim().is_empty() {
                    return Err("rua_agent runner requires agent_id".to_string());
                }
            }
            Self::SubAgent {} => {}
            Self::Acp { runner } => {
                if runner.trim().is_empty() {
                    return Err("acp runner requires runner".to_string());
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskSpec {
    pub owner: TaskOwner,
    pub instruction: String,
    pub runner: TaskRunnerSpec,
    #[serde(default)]
    pub context_refs: Vec<TaskContextRef>,
    #[serde(default)]
    pub depends_on: Vec<TaskId>,
    #[serde(default)]
    pub origin: TaskOrigin,
}

impl TaskSpec {
    pub fn validate(&self) -> Result<(), String> {
        self.owner.validate()?;
        if self.instruction.trim().is_empty() {
            return Err("task instruction must not be empty".to_string());
        }
        self.runner.validate()?;
        let mut unique = std::collections::HashSet::new();
        for dependency in &self.depends_on {
            if dependency.trim().is_empty() {
                return Err("depends_on contains an empty Task ID".to_string());
            }
            if !unique.insert(dependency) {
                return Err(format!("depends_on contains duplicate Task '{dependency}'"));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Accepted,
    Waiting,
    Blocked,
    Running,
    WaitingApproval,
    Completed,
    Failed,
    Cancelled,
    Interrupted,
}

impl TaskState {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Cancelled | Self::Interrupted
        )
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskDeviceContext {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authority_epoch: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskResourceRef {
    pub uri: String,
    pub ref_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_device_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskArtifact {
    pub id: String,
    pub name: String,
    pub media_type: String,
    pub resource: TaskResourceRef,
    #[serde(default)]
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskErrorKind {
    #[default]
    Runner,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskErrorInfo {
    #[serde(default)]
    pub kind: TaskErrorKind,
    pub message: String,
    #[serde(default)]
    pub retryable: bool,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub details: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskResult {
    pub status: TaskState,
    pub output: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_result_ref: Option<TaskResourceRef>,
    #[serde(default)]
    pub artifacts: Vec<TaskArtifact>,
    pub device: TaskDeviceContext,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<TaskErrorInfo>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskRecord {
    pub task_id: TaskId,
    pub spec: TaskSpec,
    pub state: TaskState,
    pub attempt_no: u32,
    #[serde(default)]
    pub input_capability: crate::session::TaskInputCapability,
    #[serde(default)]
    pub watch_active: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<TaskResult>,
    #[serde(default)]
    pub attempts: Vec<TaskAttempt>,
    #[serde(default)]
    pub waiting_on: Vec<TaskId>,
    #[serde(default)]
    pub blocked_by: Vec<TaskId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskAttempt {
    pub attempt_no: u32,
    pub state: TaskState,

    #[serde(default)]
    pub authorization_agent_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub configuration_epoch: Option<String>,
    #[serde(default)]
    pub device: TaskDeviceContext,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<TaskResult>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskAttemptAuthorization {
    pub agent_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub configuration_epoch: Option<String>,
}

impl TaskAttemptAuthorization {
    pub fn new(agent_id: impl Into<String>) -> Result<Self, String> {
        let agent_id = agent_id.into();
        if agent_id.trim().is_empty() {
            return Err("Task Attempt authorization Agent ID must not be empty".to_string());
        }
        Ok(Self {
            agent_id,
            configuration_epoch: None,
        })
    }

    pub fn with_configuration_epoch(mut self, epoch: String) -> Self {
        self.configuration_epoch = Some(epoch);
        self
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskListFilter {
    #[serde(default)]
    pub offset: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<TaskState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskEventKind {
    Accepted,
    Waiting,
    Blocked,
    Running,
    Completed,
    Failed,
    Cancelled,
    Interrupted,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskDependencyResult {
    pub task_id: TaskId,
    pub attempt_no: u32,
    pub status: TaskState,
    pub output: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_result_ref: Option<TaskResourceRef>,
    #[serde(default)]
    pub artifacts: Vec<TaskArtifact>,
    pub device: TaskDeviceContext,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskEvent {
    pub sequence: u64,
    pub owner: TaskOwner,
    pub task_id: TaskId,
    pub attempt_no: u32,
    pub kind: TaskEventKind,
    pub state: TaskState,
    pub created_at: DateTime<Utc>,
}
