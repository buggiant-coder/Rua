use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::pin::Pin;

use futures::Stream;
use rua_core::event::RuntimeEvent;
use rua_core::id::SessionId;
use rua_core::RuaError;

macro_rules! define_invocation_id {
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
    };
}

define_invocation_id!(ChatId, "Unique identifier for a chat conversation.");
define_invocation_id!(UserId, "Unique identifier for a user.");

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InvocationChannel {
    #[default]
    Console,

    CronJob,

    External {
        #[serde(default)]
        source: Option<String>,
    },

    Android,
}

impl fmt::Display for InvocationChannel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Console => write!(f, "console"),
            Self::CronJob => write!(f, "cron_job"),
            Self::External { source } => {
                write!(f, "external")?;
                if let Some(s) = source {
                    write!(f, ":{s}")?;
                }
                Ok(())
            }
            Self::Android => write!(f, "android"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvocationContext {
    pub session_id: SessionId,
    pub chat_id: ChatId,
    pub user_id: UserId,
    pub channel: InvocationChannel,
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

impl InvocationContext {
    pub fn new(
        session_id: SessionId,
        chat_id: ChatId,
        user_id: UserId,
        channel: InvocationChannel,
    ) -> Self {
        Self {
            session_id,
            chat_id,
            user_id,
            channel,
            metadata: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CancellationHandle {
    sender: Option<tokio::sync::watch::Sender<bool>>,
    receiver: tokio::sync::watch::Receiver<bool>,
}

impl CancellationHandle {
    pub fn new() -> Self {
        let (tx, rx) = tokio::sync::watch::channel(false);
        Self {
            sender: Some(tx),
            receiver: rx,
        }
    }

    pub fn noop() -> Self {
        let (_tx, rx) = tokio::sync::watch::channel(false);
        Self {
            sender: None,
            receiver: rx,
        }
    }

    pub fn cancel(&self) {
        if let Some(tx) = &self.sender {
            let _ = tx.send(true);
        }
    }

    pub fn is_cancelled(&self) -> bool {
        *self.receiver.borrow()
    }

    pub fn subscribe(&self) -> tokio::sync::watch::Receiver<bool> {
        self.receiver.clone()
    }
}

impl Default for CancellationHandle {
    fn default() -> Self {
        Self::new()
    }
}

pub type EventStream =
    Pin<Box<dyn Stream<Item = std::result::Result<RuntimeEvent, RuaError>> + Send>>;
