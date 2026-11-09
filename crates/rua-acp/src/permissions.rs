use std::collections::HashMap;

pub use agent_client_protocol::schema::v1::{
    PermissionOptionId, PermissionOptionKind, RequestPermissionOutcome, RequestPermissionRequest,
    RequestPermissionResponse, SelectedPermissionOutcome,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::{oneshot, RwLock};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionDecision {
    Denied { reason: String },
    Relay,
}

#[async_trait]
pub trait PermissionEvaluator: Send + Sync {
    async fn evaluate(
        &self,
        agent_id: &str,
        session_id: &str,
        request: &RequestPermissionRequest,
    ) -> PermissionDecision;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpPermissionRequestContext {
    pub agent_id: String,
    pub chat_id: String,
    pub chat_address: Option<rua_core::ChatAddress>,
    pub runner: String,
    pub task: Option<AcpPermissionTaskContext>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcpPermissionTaskContext {
    pub task_id: String,
    pub attempt_no: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_turn_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_tool_call_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcpPermissionOption {
    pub option_id: String,
    pub name: String,
    pub kind: PermissionOptionKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AcpPermissionRequest {
    pub id: String,
    pub agent_id: String,
    pub chat_id: String,
    pub chat_address: Box<rua_core::ChatAddress>,
    pub session_id: String,
    pub runner: String,
    pub tool_call_id: String,
    pub tool_name: String,
    pub tool_input: serde_json::Value,
    pub options: Vec<AcpPermissionOption>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<AcpPermissionTaskContext>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AcpPermissionSnapshot {
    pub pending: Vec<AcpPermissionRequest>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionRelayResolution {
    Selected(PermissionOptionId),
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AcpPermissionRelayError {
    NotFound(String),
    WrongScope,
    UnknownOption(String),
}

impl std::fmt::Display for AcpPermissionRelayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(id) => write!(f, "ACP permission request not found: {id}"),
            Self::WrongScope => {
                write!(f, "ACP permission request belongs to another Agent or Chat")
            }
            Self::UnknownOption(id) => write!(f, "unknown ACP permission option: {id}"),
        }
    }
}

impl std::error::Error for AcpPermissionRelayError {}

struct PendingPermission {
    request: AcpPermissionRequest,
    relay_scope_id: String,
    resolution_tx: oneshot::Sender<PermissionRelayResolution>,
}

#[derive(Default)]
pub struct AcpPermissionRelay {
    pending: RwLock<HashMap<String, PendingPermission>>,
}

impl AcpPermissionRelay {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn create_request(
        &self,
        context: AcpPermissionRequestContext,
        request: &RequestPermissionRequest,
        enriched_tool_call: Option<&agent_client_protocol::schema::v1::ToolCall>,
    ) -> (
        AcpPermissionRequest,
        oneshot::Receiver<PermissionRelayResolution>,
    ) {
        let relay_scope_id = context
            .chat_address
            .clone()
            .unwrap_or_else(|| rua_core::ChatAddress::agent(&context.agent_id, &context.chat_id))
            .identity_key();
        self.create_request_in_scope(relay_scope_id, context, request, enriched_tool_call)
            .await
    }

    pub async fn create_request_in_scope(
        &self,
        relay_scope_id: String,
        context: AcpPermissionRequestContext,
        request: &RequestPermissionRequest,
        enriched_tool_call: Option<&agent_client_protocol::schema::v1::ToolCall>,
    ) -> (
        AcpPermissionRequest,
        oneshot::Receiver<PermissionRelayResolution>,
    ) {
        let tool_call_id = request.tool_call.tool_call_id.to_string();
        let tool_name = enriched_tool_call
            .map(|tool_call| tool_call.title.clone())
            .or_else(|| request.tool_call.fields.title.clone())
            .filter(|title| !title.trim().is_empty())
            .unwrap_or_else(|| "ACP tool".to_string());
        let tool_input = enriched_tool_call
            .and_then(|tool_call| tool_call.raw_input.clone())
            .or_else(|| request.tool_call.fields.raw_input.clone())
            .unwrap_or_else(|| serde_json::json!({}));
        let chat_address = context
            .chat_address
            .clone()
            .unwrap_or_else(|| rua_core::ChatAddress::agent(&context.agent_id, &context.chat_id));
        let permission_request = AcpPermissionRequest {
            id: uuid::Uuid::new_v4().to_string(),
            agent_id: context.agent_id,
            chat_id: context.chat_id,
            chat_address: Box::new(chat_address),
            session_id: request.session_id.to_string(),
            runner: context.runner,
            tool_call_id,
            tool_name,
            tool_input,
            options: request
                .options
                .iter()
                .map(|option| AcpPermissionOption {
                    option_id: option.option_id.to_string(),
                    name: option.name.clone(),
                    kind: option.kind,
                })
                .collect(),
            task: context.task,
        };
        let (resolution_tx, resolution_rx) = oneshot::channel();
        self.pending.write().await.insert(
            permission_request.id.clone(),
            PendingPermission {
                request: permission_request.clone(),
                relay_scope_id,
                resolution_tx,
            },
        );
        (permission_request, resolution_rx)
    }

    pub async fn snapshot(&self, agent_id: &str, chat_id: &str) -> AcpPermissionSnapshot {
        self.snapshot_for_address(&rua_core::ChatAddress::agent(agent_id, chat_id))
            .await
    }

    pub async fn snapshot_for_address(
        &self,
        address: &rua_core::ChatAddress,
    ) -> AcpPermissionSnapshot {
        let mut pending = self
            .pending
            .read()
            .await
            .values()
            .filter(|pending| pending.request.chat_address.as_ref() == address)
            .map(|pending| pending.request.clone())
            .collect::<Vec<_>>();
        pending.sort_by(|left, right| left.id.cmp(&right.id));
        AcpPermissionSnapshot { pending }
    }

    pub async fn request(&self, request_id: &str) -> Option<AcpPermissionRequest> {
        self.pending
            .read()
            .await
            .get(request_id)
            .map(|pending| pending.request.clone())
    }

    pub async fn decide(
        &self,
        agent_id: &str,
        chat_id: &str,
        request_id: &str,
        option_id: &str,
    ) -> Result<AcpPermissionRequest, AcpPermissionRelayError> {
        self.decide_for_address(
            &rua_core::ChatAddress::agent(agent_id, chat_id),
            request_id,
            option_id,
        )
        .await
    }

    pub async fn decide_for_address(
        &self,
        address: &rua_core::ChatAddress,
        request_id: &str,
        option_id: &str,
    ) -> Result<AcpPermissionRequest, AcpPermissionRelayError> {
        let mut pending = self.pending.write().await;
        let request = pending
            .get(request_id)
            .ok_or_else(|| AcpPermissionRelayError::NotFound(request_id.to_string()))?;
        if request.request.chat_address.as_ref() != address {
            return Err(AcpPermissionRelayError::WrongScope);
        }
        if !request
            .request
            .options
            .iter()
            .any(|option| option.option_id == option_id)
        {
            return Err(AcpPermissionRelayError::UnknownOption(
                option_id.to_string(),
            ));
        }

        let pending = pending
            .remove(request_id)
            .expect("validated pending ACP permission request");
        let request = pending.request;
        let _ = pending
            .resolution_tx
            .send(PermissionRelayResolution::Selected(
                PermissionOptionId::new(option_id.to_string()),
            ));
        Ok(request)
    }

    pub async fn cancel_request(&self, request_id: &str) {
        if let Some(pending) = self.pending.write().await.remove(request_id) {
            let _ = pending
                .resolution_tx
                .send(PermissionRelayResolution::Cancelled);
        }
    }

    pub async fn cancel_chat(&self, agent_id: &str, chat_id: &str) {
        self.cancel_chat_address(&rua_core::ChatAddress::agent(agent_id, chat_id))
            .await;
    }

    pub async fn cancel_chat_address(&self, address: &rua_core::ChatAddress) {
        let request_ids = self
            .pending
            .read()
            .await
            .iter()
            .filter(|(_, pending)| pending.request.chat_address.as_ref() == address)
            .map(|(request_id, _)| request_id.clone())
            .collect::<Vec<_>>();
        for request_id in request_ids {
            self.cancel_request(&request_id).await;
        }
    }

    pub async fn cancel_scope(&self, relay_scope_id: &str) {
        let request_ids = self
            .pending
            .read()
            .await
            .iter()
            .filter(|(_, pending)| pending.relay_scope_id == relay_scope_id)
            .map(|(request_id, _)| request_id.clone())
            .collect::<Vec<_>>();
        for request_id in request_ids {
            self.cancel_request(&request_id).await;
        }
    }
}

pub fn selected_response(option_id: PermissionOptionId) -> RequestPermissionResponse {
    RequestPermissionResponse::new(RequestPermissionOutcome::Selected(
        SelectedPermissionOutcome::new(option_id),
    ))
}

pub fn cancelled_response() -> RequestPermissionResponse {
    RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled)
}

pub fn select_unambiguous_allow_once(
    request: &RequestPermissionRequest,
) -> Option<PermissionOptionId> {
    let mut allow_once = request
        .options
        .iter()
        .filter(|option| option.kind == PermissionOptionKind::AllowOnce);
    let selected = allow_once.next()?.option_id.clone();
    if allow_once.next().is_some() {
        return None;
    }
    Some(selected)
}

pub fn extract_tool_call_id(request: &RequestPermissionRequest) -> String {
    request.tool_call.tool_call_id.to_string()
}

pub fn extract_option_ids(request: &RequestPermissionRequest) -> Vec<PermissionOptionId> {
    request
        .options
        .iter()
        .map(|option| option.option_id.clone())
        .collect()
}
