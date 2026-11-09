use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use futures::Stream;
use rua_contract::agent::{AgentInputMessage, AgentRequest, AgentRuntime, AttachmentRef};
use rua_core::event::RuntimeEvent;
use rua_core::id::SessionId;
use rua_core::message::ContentPart;
use tokio::sync::{watch, Mutex, RwLock};
use tracing;

pub mod v1;

pub const ACP_AGENT_META_KEY: &str = "rua.agent";
pub const ACP_ERROR_META_KEY: &str = "rua.error";
pub const ACP_MCP_CONFIG_HANDLE_META_KEY: &str = "rua.acp.mcp_config_handle";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum AcpProtocolVersion {
    V1,
}

impl AcpProtocolVersion {
    pub const fn label(self) -> &'static str {
        match self {
            Self::V1 => "v1",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct AcpSessionSetup {
    pub cwd: PathBuf,
    pub mcp_config: Option<serde_json::Value>,
}

impl AcpSessionSetup {
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        Self {
            cwd: cwd.into(),
            mcp_config: None,
        }
    }

    pub fn with_mcp_config(mut self, mcp_config: Option<serde_json::Value>) -> Self {
        self.mcp_config = mcp_config;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpSessionInfo {
    pub session_id: String,
    pub cwd: PathBuf,
}

static REQUEST_MCP_CONFIGS: OnceLock<std::sync::RwLock<HashMap<String, serde_json::Value>>> =
    OnceLock::new();

fn request_mcp_configs() -> &'static std::sync::RwLock<HashMap<String, serde_json::Value>> {
    REQUEST_MCP_CONFIGS.get_or_init(|| std::sync::RwLock::new(HashMap::new()))
}

pub fn register_request_mcp_config(config: serde_json::Value) -> String {
    let handle = uuid::Uuid::new_v4().to_string();
    request_mcp_configs()
        .write()
        .expect("ACP request MCP config registry poisoned")
        .insert(handle.clone(), config);
    handle
}

pub fn take_request_mcp_config(handle: &str) -> Option<serde_json::Value> {
    request_mcp_configs()
        .write()
        .expect("ACP request MCP config registry poisoned")
        .remove(handle)
}

fn remove_request_mcp_config(handle: &str) {
    request_mcp_configs()
        .write()
        .expect("ACP request MCP config registry poisoned")
        .remove(handle);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcpApprovalOptionKind {
    AllowOnce,
    AllowAlways,
    RejectOnce,
    RejectAlways,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpApprovalOption {
    pub id: String,
    pub label: String,
    pub kind: AcpApprovalOptionKind,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AcpApprovalRequest {
    pub request_id: String,
    pub tool_call_id: String,
    pub title: String,
    pub raw_input: serde_json::Value,
    pub options: Vec<AcpApprovalOption>,
}

#[async_trait]
pub trait AcpApprovalBackend: Send + Sync {
    async fn pending_request(
        &self,
        agent_id: &str,
        session_id: &str,
        request_id: &str,
    ) -> Result<Option<AcpApprovalRequest>, String>;

    async fn resolve(
        &self,
        agent_id: &str,
        request_id: &str,
        option_id: Option<&str>,
    ) -> Result<(), String>;

    async fn cancel_session(&self, agent_id: &str, session_id: &str) -> Result<(), String>;
}

#[derive(Debug, thiserror::Error)]
pub enum AcpServerError {
    #[error("ACP session not found: {0}")]
    SessionNotFound(String),
    #[error("ACP session already has an active prompt: {0}")]
    SessionBusy(String),
    #[error("ACP runtime failed: {0}")]
    Runtime(#[from] rua_core::RuaError),
}

struct SessionState {
    session_id: SessionId,
    cwd: PathBuf,
    model_slot: Option<AcpModelSlot>,
    mcp_config: Option<serde_json::Value>,
    cancel_tx: Option<watch::Sender<bool>>,
}

#[derive(Clone)]
struct AcpModelSlot {
    provider_id: String,
    model_id: String,
}

pub(crate) struct ActiveTurn {
    pub(crate) acp_session_id: String,
    pub(crate) events:
        Pin<Box<dyn Stream<Item = std::result::Result<RuntimeEvent, rua_core::RuaError>> + Send>>,
    cancel_rx: watch::Receiver<bool>,
    state: Arc<Mutex<SessionState>>,
    mcp_config_handle: Option<String>,
}

impl ActiveTurn {
    pub(crate) fn was_cancelled(&self) -> bool {
        *self.cancel_rx.borrow()
    }

    pub(crate) async fn finish(self) {
        if let Some(handle) = &self.mcp_config_handle {
            remove_request_mcp_config(handle);
        }
        let mut state = self.state.lock().await;
        state.cancel_tx = None;
    }
}

pub struct RuaAcpServer {
    runtime: Arc<dyn AgentRuntime>,
    agent_id: String,
    model_slot: Option<AcpModelSlot>,
    approval_backend: Option<Arc<dyn AcpApprovalBackend>>,
    sessions: RwLock<HashMap<String, Arc<Mutex<SessionState>>>>,
}

impl RuaAcpServer {
    pub fn new(runtime: Arc<dyn AgentRuntime>) -> Self {
        Self::new_with_agent_id(runtime, "default")
    }

    pub fn new_with_agent_id(runtime: Arc<dyn AgentRuntime>, agent_id: impl Into<String>) -> Self {
        Self {
            runtime,
            agent_id: agent_id.into(),
            model_slot: None,
            approval_backend: None,
            sessions: RwLock::new(HashMap::new()),
        }
    }

    pub fn with_model(
        mut self,
        provider_id: impl Into<String>,
        model_id: impl Into<String>,
    ) -> Self {
        self.model_slot = Some(AcpModelSlot {
            provider_id: provider_id.into(),
            model_id: model_id.into(),
        });
        self
    }

    pub fn with_approval_backend(mut self, backend: Arc<dyn AcpApprovalBackend>) -> Self {
        self.approval_backend = Some(backend);
        self
    }

    pub fn agent_id(&self) -> &str {
        &self.agent_id
    }

    pub async fn create_session(&self, setup: AcpSessionSetup) -> AcpSessionInfo {
        let session_id = uuid::Uuid::new_v4().to_string();
        let state = SessionState {
            session_id: SessionId::from(session_id.clone()),
            cwd: setup.cwd.clone(),
            model_slot: self.model_slot.clone(),
            mcp_config: setup.mcp_config,
            cancel_tx: None,
        };
        self.sessions
            .write()
            .await
            .insert(session_id.clone(), Arc::new(Mutex::new(state)));
        AcpSessionInfo {
            session_id,
            cwd: setup.cwd,
        }
    }

    pub async fn resume_session(&self, session_id: &str, setup: AcpSessionSetup) -> bool {
        let state = self.sessions.read().await.get(session_id).cloned();
        let Some(state) = state else {
            return false;
        };
        let mut state = state.lock().await;
        state.cwd = setup.cwd;
        state.mcp_config = setup.mcp_config;
        true
    }

    pub async fn close_session(&self, session_id: &str) -> bool {
        if !self.sessions.read().await.contains_key(session_id) {
            return false;
        }
        self.cancel_session(session_id).await;
        self.sessions.write().await.remove(session_id).is_some()
    }

    pub async fn list_sessions(&self, cwd: Option<&Path>) -> Vec<AcpSessionInfo> {
        let sessions = self.sessions.read().await;
        let mut infos = Vec::with_capacity(sessions.len());
        for (session_id, state) in sessions.iter() {
            let state = state.lock().await;
            if cwd.is_some_and(|cwd| cwd != state.cwd) {
                continue;
            }
            infos.push(AcpSessionInfo {
                session_id: session_id.clone(),
                cwd: state.cwd.clone(),
            });
        }
        infos
    }

    pub async fn cancel_session(&self, session_id: &str) {
        let state = self.sessions.read().await.get(session_id).cloned();
        let Some(state) = state else {
            return;
        };
        let internal_session_id = {
            let state = state.lock().await;
            if let Some(tx) = &state.cancel_tx {
                let _ = tx.send(true);
            }
            state.session_id.clone()
        };
        if let Err(error) = self.runtime.cancel(&internal_session_id).await {
            tracing::debug!(session_id, error = %error, "ACP runtime cancel returned an error");
        }
        if let Some(backend) = &self.approval_backend {
            if let Err(error) = backend.cancel_session(&self.agent_id, session_id).await {
                tracing::warn!(session_id, error, "failed to cancel ACP approval requests");
            }
        }
    }

    pub(crate) async fn start_turn(
        &self,
        acp_session_id: &str,
        content: Vec<ContentPart>,
        attachments: Vec<AttachmentRef>,
        protocol: AcpProtocolVersion,
    ) -> Result<ActiveTurn, AcpServerError> {
        let session_id_str = acp_session_id.to_string();
        let state_arc = self
            .sessions
            .read()
            .await
            .get(&session_id_str)
            .cloned()
            .ok_or_else(|| AcpServerError::SessionNotFound(session_id_str.clone()))?;

        let (cancel_tx, cancel_rx) = watch::channel(false);
        let (model_slot, cwd, internal_session_id, mcp_config) = {
            let mut state = state_arc.lock().await;
            if state.cancel_tx.is_some() {
                return Err(AcpServerError::SessionBusy(session_id_str));
            }
            state.cancel_tx = Some(cancel_tx);
            (
                state.model_slot.clone(),
                state.cwd.clone(),
                state.session_id.clone(),
                state.mcp_config.clone(),
            )
        };

        let mut metadata = HashMap::new();
        metadata.insert(
            "channel".to_string(),
            serde_json::Value::String("acp".to_string()),
        );
        metadata.insert(
            "acp_protocol".to_string(),
            serde_json::Value::String(protocol.label().to_string()),
        );
        if let Some(model_slot) = model_slot {
            metadata.insert(
                "chat_model_slot".to_string(),
                serde_json::json!({
                    "provider_id": model_slot.provider_id,
                    "model": model_slot.model_id,
                }),
            );
        }
        let mcp_config_handle = mcp_config.map(register_request_mcp_config);
        if let Some(handle) = &mcp_config_handle {
            metadata.insert(
                ACP_MCP_CONFIG_HANDLE_META_KEY.to_string(),
                serde_json::Value::String(handle.clone()),
            );
        }

        let agent_req = AgentRequest {
            session_id: internal_session_id.clone(),
            user_id: format!("acp:{acp_session_id}"),
            agent_id: self.agent_id.clone().into(),
            messages: vec![AgentInputMessage {
                role: "user".to_string(),
                content,
            }],
            model_override: None,
            tool_mode: "auto".to_string(),
            attachments,
            display_preferences: None,
            disabled_tools: vec![],
            metadata,
            cancel: Some(cancel_rx.clone()),
            run_control: None,
            workspace_dir: Some(cwd),
            conversation_store_dir: None,
            working_dir: None,
        };

        match self.runtime.stream(agent_req).await {
            Ok(events) => Ok(ActiveTurn {
                acp_session_id: session_id_str,
                events,
                cancel_rx,
                state: state_arc,
                mcp_config_handle,
            }),
            Err(error) => {
                if let Some(handle) = &mcp_config_handle {
                    remove_request_mcp_config(handle);
                }
                let mut state = state_arc.lock().await;
                state.cancel_tx = None;
                Err(AcpServerError::Runtime(error))
            }
        }
    }

    pub(crate) async fn pending_approval(
        &self,
        session_id: &str,
        request_id: &str,
    ) -> Result<Option<AcpApprovalRequest>, String> {
        let Some(backend) = &self.approval_backend else {
            return Ok(None);
        };
        backend
            .pending_request(&self.agent_id, session_id, request_id)
            .await
    }

    pub(crate) async fn resolve_approval(
        &self,
        request_id: &str,
        option_id: Option<&str>,
    ) -> Result<(), String> {
        let Some(backend) = &self.approval_backend else {
            return Err("ACP approval backend is not configured".to_string());
        };
        backend.resolve(&self.agent_id, request_id, option_id).await
    }
}
