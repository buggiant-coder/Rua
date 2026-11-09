use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum Role {
    System,

    User,

    Assistant,

    Tool,

    Developer,
}

impl Role {
    pub fn is_assistant(&self) -> bool {
        matches!(self, Role::Assistant)
    }

    pub fn is_user(&self) -> bool {
        matches!(self, Role::User)
    }

    pub fn is_tool(&self) -> bool {
        matches!(self, Role::Tool)
    }
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Role::System => write!(f, "system"),
            Role::User => write!(f, "user"),
            Role::Assistant => write!(f, "assistant"),
            Role::Tool => write!(f, "tool"),
            Role::Developer => write!(f, "developer"),
        }
    }
}
