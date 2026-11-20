use std::collections::BTreeMap;
use std::fmt;
use std::hash::{Hash, Hasher};

use serde::{Deserialize, Serialize};

use crate::ChatId;

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
pub struct ChatAddress {
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
    pub fn new(chat_id: impl Into<ChatId>) -> Self {
        Self {
            chat_id: chat_id.into(),
            extra: BTreeMap::new(),
        }
    }

    pub fn chat_id(&self) -> &ChatId {
        &self.chat_id
    }

    pub fn identity_key(&self) -> String {
        self.chat_id.to_string()
    }
}

impl PartialEq for ChatAddress {
    fn eq(&self, other: &Self) -> bool {
        self.chat_id == other.chat_id
    }
}

impl Eq for ChatAddress {}

impl Hash for ChatAddress {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.chat_id.hash(state);
    }
}

impl fmt::Display for ChatAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "chat:{}", self.chat_id)
    }
}
