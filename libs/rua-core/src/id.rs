use serde::{Deserialize, Serialize};
use std::fmt;

macro_rules! define_id {
    ($name:ident, $doc:expr) => {
        #[doc = $doc]
        #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            pub fn new(id: impl Into<String>) -> Self {
                Self(id.into())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl From<String> for $name {
            fn from(s: String) -> Self {
                Self(s)
            }
        }

        impl From<&str> for $name {
            fn from(s: &str) -> Self {
                Self(s.to_string())
            }
        }

        impl From<&String> for $name {
            fn from(s: &String) -> Self {
                Self(s.clone())
            }
        }
    };
}

define_id!(AgentId, "Unique identifier for an agent.");
define_id!(ProjectId, "Unique identifier for a project.");
define_id!(ChatId, "Unique identifier for a chat.");
define_id!(SessionId, "Unique identifier for a chat session.");
define_id!(MessageId, "Unique identifier for a message.");
define_id!(ToolCallId, "Unique identifier for a tool call.");
