use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use rua_core::id::{AgentId, MessageId, SessionId};
use rua_core::message::ModelDialogueItem;
use rua_core::Result;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatSpec {
    pub id: SessionId,
    pub title: String,
    pub agent_id: AgentId,
    #[serde(default)]
    pub user_id: Option<String>,
    #[serde(default)]
    pub channel: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub pin_order: i64,
    #[serde(default)]
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateChatRequest {
    #[serde(default)]
    pub title: Option<String>,
    pub agent_id: AgentId,
    #[serde(default)]
    pub user_id: Option<String>,
    #[serde(default)]
    pub channel: Option<String>,
    #[serde(default)]
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ListChatsQuery {
    #[serde(default)]
    pub agent_id: Option<AgentId>,
    #[serde(default)]
    pub user_id: Option<String>,
    #[serde(default)]
    pub channel: Option<String>,
    pub offset: u64,
    pub limit: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListChatsResponse {
    pub data: Vec<ChatSpec>,
    pub total: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ChatPatch {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub pin_order: Option<i64>,
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MessageQuery {
    pub offset: u64,
    pub limit: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageList {
    pub messages: Vec<ModelDialogueItem>,
    pub total: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatSearchHit {
    pub chat: ChatSpec,
    #[serde(default)]
    pub matched_message_id: Option<MessageId>,
    #[serde(default)]
    pub snippet: Option<String>,
}

#[async_trait]
pub trait ChatSessionStore: Send + Sync {
    async fn create_chat(&self, req: CreateChatRequest) -> Result<ChatSpec>;
    async fn get_chat(&self, id: &SessionId) -> Result<Option<ChatSpec>>;
    async fn list_chats(&self, query: ListChatsQuery) -> Result<ListChatsResponse>;
    async fn update_chat(&self, id: &SessionId, patch: ChatPatch) -> Result<ChatSpec>;
    async fn delete_chat(&self, id: &SessionId) -> Result<()>;
    async fn append_message(&self, id: &SessionId, message: ModelDialogueItem) -> Result<()>;
    async fn list_messages(&self, id: &SessionId, query: MessageQuery) -> Result<MessageList>;
    async fn search_chats(&self, query: String) -> Result<Vec<ChatSearchHit>>;
}
