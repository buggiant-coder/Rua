use std::fmt;

use agent_client_protocol::schema::v1::SessionId as AcpSessionId;
use rua_core::id::SessionId;
use tokio::sync::oneshot;

#[derive(Debug)]
pub enum SessionInit {
    New {
        cwd: String,
    },
    Load {
        session_id: AcpSessionId,
        cwd: String,
        history_tx: Option<oneshot::Sender<String>>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionKey {
    pub chat_id: SessionId,
    pub agent_name: String,
}

impl fmt::Display for SessionKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "({}, {})", self.chat_id, self.agent_name)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AcpError {
    #[error("agent not found: {0}")]
    AgentNotFound(String),

    #[error("agent not enabled: {0}")]
    AgentNotEnabled(String),

    #[error("session not found: {0}")]
    SessionNotFound(SessionKey),

    #[error("session already exists: {0}")]
    SessionAlreadyExists(SessionKey),

    #[error("turn already in progress for session: {0}")]
    TurnInProgress(SessionKey),

    #[error("no active turn to cancel for session: {0}")]
    NoActiveTurn(SessionKey),

    #[error("session closed: {0}")]
    SessionClosed(SessionKey),

    #[error("agent process exited unexpectedly")]
    ProcessExited,

    #[error("connection error: {0}")]
    Connection(String),

    #[error("timeout after {0:?}")]
    Timeout(std::time::Duration),

    #[error("spawn failed: {0}")]
    SpawnFailed(String),
}
