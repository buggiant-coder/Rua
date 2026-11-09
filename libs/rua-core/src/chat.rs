use std::collections::BTreeMap;
use std::fmt;
use std::hash::{Hash, Hasher};

use serde::{Deserialize, Serialize};

use crate::{AgentId, ChatId, ProjectId};

pub const CHAT_COLLABORATION_MODE_META_KEY: &str = "agent_collaboration_mode";
pub const TURN_COLLABORATION_MODE_OVERRIDE_KEY: &str = "collaboration_mode";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentCollaborationMode {
    #[default]
    Default,
    Plan,
    Execute,
}

impl AgentCollaborationMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Plan => "plan",
            Self::Execute => "execute",
        }
    }
}

impl fmt::Display for AgentCollaborationMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl std::str::FromStr for AgentCollaborationMode {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "default" => Ok(Self::Default),
            "plan" => Ok(Self::Plan),
            "execute" => Ok(Self::Execute),
            _ => Err(format!(
                "expected one of default, plan, execute; got {value}"
            )),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ChatScope {
    Agent {
        agent_id: AgentId,
        #[serde(flatten)]
        extra: BTreeMap<String, serde_json::Value>,
    },
    Project {
        project_id: ProjectId,
        #[serde(flatten)]
        extra: BTreeMap<String, serde_json::Value>,
    },
}

impl ChatScope {
    pub fn agent(agent_id: impl Into<AgentId>) -> Self {
        Self::Agent {
            agent_id: agent_id.into(),
            extra: BTreeMap::new(),
        }
    }

    pub fn project(project_id: impl Into<ProjectId>) -> Self {
        Self::Project {
            project_id: project_id.into(),
            extra: BTreeMap::new(),
        }
    }

    pub fn agent_id(&self) -> Option<&AgentId> {
        match self {
            Self::Agent { agent_id, .. } => Some(agent_id),
            Self::Project { .. } => None,
        }
    }

    pub fn project_id(&self) -> Option<&ProjectId> {
        match self {
            Self::Agent { .. } => None,
            Self::Project { project_id, .. } => Some(project_id),
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Self::Agent { .. } => "agent",
            Self::Project { .. } => "project",
        }
    }

    pub fn owner_id(&self) -> &str {
        match self {
            Self::Agent { agent_id, .. } => agent_id.as_str(),
            Self::Project { project_id, .. } => project_id.as_str(),
        }
    }

    pub fn from_kind_and_owner(kind: &str, owner_id: impl Into<String>) -> Option<Self> {
        let owner_id = owner_id.into();
        match kind {
            "agent" => Some(Self::agent(owner_id)),
            "project" => Some(Self::project(owner_id)),
            _ => None,
        }
    }
}

impl PartialEq for ChatScope {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::Agent { agent_id: left, .. },
                Self::Agent {
                    agent_id: right, ..
                },
            ) => left == right,
            (
                Self::Project {
                    project_id: left, ..
                },
                Self::Project {
                    project_id: right, ..
                },
            ) => left == right,
            _ => false,
        }
    }
}

impl Eq for ChatScope {}

impl Hash for ChatScope {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self {
            Self::Agent { agent_id, .. } => {
                0_u8.hash(state);
                agent_id.hash(state);
            }
            Self::Project { project_id, .. } => {
                1_u8.hash(state);
                project_id.hash(state);
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatAddress {
    pub scope: ChatScope,
    pub chat_id: ChatId,
    #[serde(flatten)]
    extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationActivityPosition {
    pub activity_generation: String,
    pub next_activity_sequence: u64,
    #[serde(flatten)]
    extra: BTreeMap<String, serde_json::Value>,
}

impl ConversationActivityPosition {
    pub fn new(activity_generation: impl Into<String>, next_activity_sequence: u64) -> Self {
        Self {
            activity_generation: activity_generation.into(),
            next_activity_sequence: next_activity_sequence.max(1),
            extra: BTreeMap::new(),
        }
    }
}

impl ChatAddress {
    pub fn new(scope: ChatScope, chat_id: impl Into<ChatId>) -> Self {
        Self {
            scope,
            chat_id: chat_id.into(),
            extra: BTreeMap::new(),
        }
    }

    pub fn agent(agent_id: impl Into<AgentId>, chat_id: impl Into<ChatId>) -> Self {
        Self::new(ChatScope::agent(agent_id), chat_id)
    }

    pub fn project(project_id: impl Into<ProjectId>, chat_id: impl Into<ChatId>) -> Self {
        Self::new(ChatScope::project(project_id), chat_id)
    }

    pub fn scope(&self) -> &ChatScope {
        &self.scope
    }

    pub fn chat_id(&self) -> &ChatId {
        &self.chat_id
    }

    pub fn agent_id(&self) -> Option<&AgentId> {
        self.scope.agent_id()
    }

    pub fn project_id(&self) -> Option<&ProjectId> {
        self.scope.project_id()
    }

    pub fn identity_key(&self) -> String {
        serde_json::to_string(&(
            self.scope.kind(),
            self.scope.owner_id(),
            self.chat_id.as_str(),
        ))
        .expect("Chat Address identity tuple is serializable")
    }
}

impl PartialEq for ChatAddress {
    fn eq(&self, other: &Self) -> bool {
        self.scope == other.scope && self.chat_id == other.chat_id
    }
}

impl Eq for ChatAddress {}

impl Hash for ChatAddress {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.scope.hash(state);
        self.chat_id.hash(state);
    }
}

impl fmt::Display for ChatAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.scope {
            ChatScope::Agent { agent_id, .. } => {
                write!(f, "agent:{agent_id}/chat:{}", self.chat_id)
            }
            ChatScope::Project { project_id, .. } => {
                write!(f, "project:{project_id}/chat:{}", self.chat_id)
            }
        }
    }
}
