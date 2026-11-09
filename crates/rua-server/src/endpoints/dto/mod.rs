pub mod local_model;
pub use local_model::*;
pub mod model;
pub use model::{
    CustomProviderCreateDto, DefaultModelDto, DefaultModelResponseDto, ModelDto, ProbeRequestDto,
    ProbeResultDto, ProviderDto, ProviderTemplateInfoDto, ProviderUpdateDto,
};
pub mod tool;
pub use tool::{ToolAsyncExecutionRequest, ToolConfigUpdateRequest, ToolInfo, ToolToggleRequest};
pub mod cron;
pub use cron::{
    CronDispatchHistoryEntry, CronDispatchSpec, CronDispatchTarget, CronDispatchTargetItem,
    CronDispatchTargetsResponse, CronJob, CronJobCreate, CronJobRunOverride, CronJobRuntimeSpec,
    CronJobState, CronJobUpdate, CronScheduleSpec,
};

pub mod config;
pub mod env;
pub mod security;
pub use security::{
    AllowNoAuthHostsConfig, ApprovalModeConfig, BlockedHistoryEntry, BlockedSkillFinding,
    BuiltinRule, CustomArgMatcherDto, CustomPathMatcherDto, CustomRule, FileGuardConfig,
    SkillScannerConfig, SkillWhitelistEntry, ToolGuardConfig,
};
pub mod prompt;
pub use prompt::{
    PromptBroadcastRequest, PromptBroadcastResponse, PromptCreate, PromptEntry,
    PromptLibrarySyncRequest, PromptLibrarySyncResponse, PromptUpdate,
};
pub mod settings;
pub mod template;
pub mod usage;

use rua_core::message::ContentPart;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginRequest {
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
    #[serde(default)]
    pub expires_in: Option<i64>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginResponse {
    #[serde(default)]
    pub token: String,
    #[serde(default)]
    pub username: String,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterRequest {
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
    #[serde(default)]
    pub expires_in: Option<i64>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthStatusResponse {
    pub enabled: bool,
    pub has_users: bool,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateProfileRequest {
    #[serde(default)]
    pub current_password: String,
    #[serde(default)]
    pub new_password: Option<String>,
    #[serde(default)]
    pub new_username: Option<String>,
    #[serde(default)]
    pub expires_in: Option<i64>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RevokeTokenRequest {
    #[serde(default)]
    pub token: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSummary {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub workspace_dir: String,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentListResponse {
    pub agents: Vec<AgentSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDetail {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub workspace_dir: String,
    #[serde(default)]
    pub template_id: Option<String>,
    #[serde(default)]
    pub language: String,
    #[serde(default)]
    pub system_prompt_files: Vec<String>,
    #[serde(default)]
    pub running: Option<serde_json::Value>,
    #[serde(default)]
    pub default_model: Option<serde_json::Value>,
    #[serde(default)]
    pub channels: Option<serde_json::Value>,
    #[serde(default)]
    pub mcp: Option<serde_json::Value>,
    #[serde(default)]
    pub heartbeat: Option<serde_json::Value>,
    #[serde(default)]
    pub llm_routing: Option<serde_json::Value>,
    #[serde(default)]
    pub tools: Option<serde_json::Value>,
    #[serde(default)]
    pub plan: Option<serde_json::Value>,
    #[serde(default)]
    pub security: Option<serde_json::Value>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentStatus {
    pub id: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub active_sessions: u32,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentCreate {
    #[serde(default)]
    pub id: Option<String>,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub workspace_dir: Option<String>,
    #[serde(default)]
    pub template_id: Option<String>,
    #[serde(default)]
    pub skill_names: Option<Vec<String>>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentUpdate {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub system_prompt_files: Option<Vec<String>>,
    #[serde(default)]
    pub running: Option<serde_json::Value>,
    #[serde(default, deserialize_with = "deserialize_explicit_value_option")]
    pub default_model: Option<serde_json::Value>,
    #[serde(default)]
    pub channels: Option<serde_json::Value>,
    #[serde(default)]
    pub mcp: Option<serde_json::Value>,
    #[serde(default)]
    pub heartbeat: Option<serde_json::Value>,
    #[serde(default)]
    pub llm_routing: Option<serde_json::Value>,
    #[serde(default)]
    pub tools: Option<serde_json::Value>,
    #[serde(default)]
    pub plan: Option<serde_json::Value>,
    #[serde(default)]
    pub security: Option<serde_json::Value>,
    #[serde(default)]
    pub skill_names: Option<Vec<String>>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatModelDto {
    #[serde(default)]
    pub provider_id: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub thinking_strength: Option<String>,
    #[serde(default)]
    pub thinking_enabled: Option<bool>,
    #[serde(default)]
    pub thinking_budget: Option<i64>,
    #[serde(default)]
    pub reasoning_effort: Option<String>,
    #[serde(default)]
    pub thinking_param_style: Option<String>,
    #[serde(default)]
    pub thinking_budget_range: Option<[i64; 2]>,
    #[serde(default)]
    pub reasoning_effort_options: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum ChatModelInput {
    Structured(ChatModelDto),
    LegacyId(String),
}

impl ChatModelInput {
    pub fn model_id(&self) -> &str {
        match self {
            Self::Structured(model) => &model.model,
            Self::LegacyId(model) => model,
        }
    }

    pub fn structured(&self) -> Option<&ChatModelDto> {
        match self {
            Self::Structured(model) => Some(model),
            Self::LegacyId(_) => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatRequest {
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub chat_id: Option<String>,
    #[serde(default)]
    pub input: Option<Vec<serde_json::Value>>,
    #[serde(default)]
    pub messages: Option<Vec<serde_json::Value>>,
    #[serde(default)]
    pub user_id: Option<String>,
    #[serde(default)]
    pub channel: Option<String>,
    #[serde(default)]
    pub stream: Option<bool>,
    #[serde(default)]
    pub model: Option<ChatModelInput>,
    #[serde(default)]
    pub attachments: Vec<AttachmentDto>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatLiveRequest {
    #[serde(default)]
    pub chat_id: Option<String>,
    #[serde(default)]
    pub viewer_id: Option<String>,
    #[serde(default)]
    pub activity_generation: Option<String>,
    #[serde(default)]
    pub next_activity_sequence: Option<u64>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl ChatLiveRequest {
    pub fn effective_chat_id(&self) -> Option<String> {
        self.chat_id.as_ref().filter(|id| !id.is_empty()).cloned()
    }

    pub fn activity_position(&self) -> Option<rua_core::ConversationActivityPosition> {
        Some(rua_core::ConversationActivityPosition::new(
            self.activity_generation.as_deref()?.trim(),
            self.next_activity_sequence?,
        ))
        .filter(|position| !position.activity_generation.is_empty())
    }
}

impl ChatRequest {
    pub fn structured_model(&self) -> Option<&ChatModelDto> {
        self.model.as_ref().and_then(ChatModelInput::structured)
    }

    pub fn prepare_model_for_console(&mut self) -> Option<String> {
        let model_id = self
            .model
            .as_ref()
            .map(ChatModelInput::model_id)
            .map(str::to_string);
        if let Some(model) = self.structured_model().cloned() {
            self.extra.insert(
                "chat_model_slot".to_string(),
                serde_json::to_value(model).expect("Chat model is serializable"),
            );
        }
        model_id
    }

    pub fn effective_message(&self) -> String {
        let source = self
            .input
            .as_ref()
            .filter(|v| !v.is_empty())
            .or_else(|| self.messages.as_ref().filter(|v| !v.is_empty()));

        if let Some(arr) = source {
            let mut texts = Vec::new();
            for msg in arr {
                if msg.get("role").and_then(|r| r.as_str()) == Some("user") {
                    if let Some(content) = msg.get("content") {
                        if let Some(text) = content.as_str() {
                            texts.push(text.to_string());
                            continue;
                        }

                        if let Some(parts) = content.as_array() {
                            for part in parts {
                                if let Some(text) = part.as_str() {
                                    texts.push(text.to_string());
                                } else if let Some(text) = part.get("text").and_then(|t| t.as_str())
                                {
                                    texts.push(text.to_string());
                                }
                            }
                        }
                    }
                }
            }
            if !texts.is_empty() {
                return texts.join("\n");
            }
        }
        self.message.clone()
    }

    pub fn effective_chat_id(&self) -> Option<String> {
        self.chat_id.clone().filter(|value| !value.is_empty())
    }

    pub fn has_user_content(&self) -> bool {
        !self.effective_message().trim().is_empty()
            || !self.extract_content_parts().is_empty()
            || !self.attachments.is_empty()
    }

    pub fn ordered_user_content_parts(&self) -> Vec<ContentPart> {
        let attachments = self
            .attachments
            .iter()
            .map(|attachment| rua_channel::core::console::AttachmentInfo {
                url: attachment.url.clone(),
                name: attachment.name.clone(),
                mime_type: attachment.mime_type.clone(),
                stored_name: attachment.stored_name.clone(),
                size: attachment.size,
            })
            .collect::<Vec<_>>();
        rua_channel::core::console::ordered_user_content_parts(
            &self.message,
            self.input.as_ref(),
            self.messages.as_ref(),
            &attachments,
        )
    }

    pub fn extract_content_parts(&self) -> Vec<ContentPart> {
        let source = self
            .input
            .as_ref()
            .filter(|v| !v.is_empty())
            .or_else(|| self.messages.as_ref().filter(|v| !v.is_empty()));

        let arr = match source {
            Some(arr) => arr,
            None => return Vec::new(),
        };

        arr.iter()
            .filter(|msg| msg.get("role").and_then(|r| r.as_str()) == Some("user"))
            .filter_map(|msg| msg.get("content").and_then(|c| c.as_array()))
            .flat_map(|content| content.iter())
            .filter_map(content_part_from_request_json)
            .filter(|part| !matches!(part, ContentPart::Text { .. }))
            .collect()
    }
}

fn content_part_from_request_json(part: &serde_json::Value) -> Option<ContentPart> {
    let type_str = part.get("type").and_then(|t| t.as_str())?;
    if part.get("media_ref").is_some() {
        let parsed = agent_types::content::content_part_from_value(part.clone());
        return (!matches!(parsed, ContentPart::Data { .. })).then_some(parsed);
    }
    match type_str {
        "text" => part
            .get("text")
            .and_then(|v| v.as_str())
            .filter(|text| !text.is_empty())
            .map(|text| ContentPart::Text {
                text: text.to_string(),
            }),
        "file" => {
            let url = part
                .get("file_url")
                .or_else(|| part.get("url"))
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let name = part
                .get("file_name")
                .or_else(|| part.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            (!url.is_empty()).then(|| ContentPart::File {
                url: url.to_string(),
                name: name.to_string(),
                media_type: optional_string(part, "mime_type"),
                size: part.get("size").and_then(|v| v.as_u64()),
            })
        }
        "image" => {
            let url = part
                .get("image_url")
                .or_else(|| part.get("url"))
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            (!url.is_empty()).then(|| ContentPart::Image {
                url: url.to_string(),
                media_type: optional_string(part, "mime_type"),
                alt: optional_string(part, "file_name").or_else(|| optional_string(part, "name")),
            })
        }
        "video" => {
            let url = part
                .get("video_url")
                .or_else(|| part.get("url"))
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            (!url.is_empty()).then(|| ContentPart::Video {
                url: url.to_string(),
                media_type: optional_string(part, "mime_type"),
            })
        }
        "audio" => {
            let url = part
                .get("audio_url")
                .or_else(|| part.get("data"))
                .or_else(|| part.get("url"))
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            (!url.is_empty()).then(|| ContentPart::Audio {
                url: url.to_string(),
                media_type: optional_string(part, "mime_type"),
            })
        }
        _ => None,
    }
}

fn optional_string(part: &serde_json::Value, key: &str) -> Option<String> {
    part.get(key)
        .and_then(|v| v.as_str())
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentDto {
    pub url: String,
    pub name: String,
    #[serde(default)]
    pub mime_type: Option<String>,
    #[serde(default)]
    pub stored_name: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChatEvent {
    Start {
        chat_id: String,
        #[serde(default)]
        agent_id: Option<String>,
    },

    TextDelta {
        message_id: String,
        delta: String,
    },

    ThinkingDelta {
        message_id: String,
        delta: String,
    },

    ModelCallReset {
        model_call_id: String,
    },

    ToolCallDelta {
        message_id: String,
        id: String,
        #[serde(default)]
        index: usize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        arguments_delta: String,
    },

    ToolCall {
        message_id: String,
        id: String,
        name: String,
        #[serde(default)]
        arguments: String,
    },

    ToolResult {
        message_id: String,
        id: String,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        content: serde_json::Value,
    },

    ToolResultDelta {
        message_id: String,
        id: String,
        #[serde(default)]
        name: Option<String>,
        output_delta: String,
    },

    Usage {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<serde_json::Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        last_call_usage: Option<serde_json::Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_input_length: Option<u64>,
    },

    Message {
        id: String,
        #[serde(default)]
        object: String,
        #[serde(default)]
        role: String,
        #[serde(default)]
        status: String,
        #[serde(default)]
        content: serde_json::Value,
    },

    UserMessage {
        #[serde(default)]
        message: serde_json::Value,
    },

    ToolApprovalRequest {
        #[serde(default)]
        object: String,
        #[serde(default)]
        id: String,
        #[serde(default)]
        role: String,
        #[serde(default)]
        status: String,
        #[serde(default)]
        content: serde_json::Value,
    },

    AskUserRequest {
        #[serde(default)]
        object: String,
        #[serde(default)]
        id: String,
        #[serde(default)]
        role: String,
        #[serde(default)]
        status: String,
        #[serde(default)]
        content: serde_json::Value,
    },
    DeviceDeliveryAccepted {
        chat_id: String,
        device_delivery_id: String,
        target_device_id: String,
        device_delivery_sequence: u64,
        accepted_at: String,
    },

    ContextCompaction {
        status: String,
        #[serde(default)]
        mode: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },

    Error {
        detail: String,
    },

    Interrupted {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },

    Done {
        #[serde(default)]
        finish_reason: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushMessage {
    #[serde(default)]
    pub id: String,
    #[serde(default, rename = "text", alias = "content")]
    pub content: String,
    #[serde(default)]
    pub timestamp: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StopChatRequest {
    #[serde(default)]
    pub turn_id: Option<String>,
    pub authority_epoch:
        crate::services::conversation_authority_placement::ConversationAuthorityEpoch,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl StopChatRequest {
    pub fn into_action(
        self,
    ) -> crate::services::device_command_bus::ConversationControlDeviceAction {
        match self.turn_id {
            Some(turn_id) => crate::services::device_command_bus::ConversationControlDeviceAction::StopTurn { turn_id, authority_epoch: self.authority_epoch },
            None => crate::services::device_command_bus::ConversationControlDeviceAction::PauseQueuedInputs { authority_epoch: self.authority_epoch },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeletedResponse {
    pub deleted: bool,
}

impl DeletedResponse {
    pub fn ok() -> Self {
        Self { deleted: true }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillUploadResponse {
    pub imported: Vec<String>,
    pub count: usize,
    pub enabled: bool,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub conflicts: Vec<SkillUploadConflict>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillUploadConflict {
    pub skill_name: String,
    pub suggested_name: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuccessResponse {
    pub success: bool,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl SuccessResponse {
    pub fn ok() -> Self {
        Self {
            success: true,
            message: None,
            extra: serde_json::Map::new(),
        }
    }

    pub fn with_message(message: impl Into<String>) -> Self {
        Self {
            success: true,
            message: Some(message.into()),
            extra: serde_json::Map::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub bot_prefix: String,
    #[serde(default)]
    pub is_builtin: Option<bool>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

pub type ChannelConfigMap = serde_json::Map<String, serde_json::Value>;

pub type ChannelConfigUpdateAll = serde_json::Map<String, serde_json::Value>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QrCodeResponse {
    pub qrcode_img: String,
    pub poll_token: String,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QrCodeStatus {
    pub status: String,
    #[serde(default)]
    pub credentials: Option<serde_json::Value>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub every: String,
    #[serde(default)]
    pub target: String,
    #[serde(default)]
    pub active_hours: Option<serde_json::Value>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserTimezoneConfig {
    #[serde(default)]
    pub timezone: String,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LlmRoutingModelSlot {
    #[serde(default)]
    pub provider_id: String,
    #[serde(default)]
    pub model: String,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl Default for LlmRoutingModelSlot {
    fn default() -> Self {
        Self {
            provider_id: String::new(),
            model: String::new(),
            extra: serde_json::Map::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LlmRoutingConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_llm_routing_mode")]
    pub mode: String,
    #[serde(default)]
    pub local: LlmRoutingModelSlot,
    #[serde(default)]
    pub cloud: Option<LlmRoutingModelSlot>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

fn default_llm_routing_mode() -> String {
    "local_first".to_string()
}

impl Default for LlmRoutingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            mode: default_llm_routing_mode(),
            local: LlmRoutingModelSlot::default(),
            cloud: None,
            extra: serde_json::Map::new(),
        }
    }
}

pub use rua_mcp::store_dto::{
    McpClientConfig, McpClientCreate, McpClientCreateNested, McpClientUpdate, McpServerConfig,
    McpToggleRequest, McpToolInfo, McpTransportConfig, SharedMcpAgentResult,
    SharedMcpBroadcastRequest, SharedMcpBroadcastResponse, SharedMcpClientConfig,
    SharedMcpClientCreate, SharedMcpClientUpdate, SharedMcpSyncRequest,
};

pub use crate::endpoints::acp::{AcpRunnerCreate, AcpRunnerUpdate};
pub use rua_config::types::{AcpAgentConfig, AcpConfig};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillInfo {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub content_hash: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install_meta: Option<serde_json::Value>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillCreate {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillUpdate {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillDetail {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub content_hash: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install_meta: Option<serde_json::Value>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillPoolEntry {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
    #[serde(default, rename = "version_text")]
    pub version: String,
    #[serde(default)]
    pub downloads: u64,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install_meta: Option<serde_json::Value>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillHubEntry {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillOptimizeRequest {
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub goal: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillPoolInstallRequest {
    #[serde(default)]
    pub skill_name: String,

    #[serde(default)]
    pub skill_id: String,
    #[serde(default)]
    pub targets: Vec<PoolDownloadTarget>,
    #[serde(default)]
    pub all_workspaces: bool,
    #[serde(default)]
    pub overwrite: bool,
    #[serde(default)]
    pub preview_only: bool,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolDownloadTarget {
    pub workspace_id: String,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillRefreshResponse {
    pub success: bool,
    #[serde(default)]
    pub count: u32,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceSkillSummary {
    #[serde(default)]
    pub agent_id: String,
    #[serde(default)]
    pub agent_name: String,
    #[serde(default)]
    pub workspace_dir: String,
    #[serde(default)]
    pub skills: Vec<WorkspaceSkillSpec>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceSkillSpec {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub version_text: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub channels: Vec<String>,
    #[serde(default)]
    pub config: Option<serde_json::Value>,
    #[serde(default)]
    pub last_updated: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install_meta: Option<serde_json::Value>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillHubSearchQuery {
    #[serde(default)]
    pub q: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillHubInstallStartRequest {
    #[serde(default)]
    pub bundle_url: String,
    #[serde(default)]
    pub version: String,
    #[serde(default = "default_install_enable")]
    pub enable: bool,
    #[serde(default)]
    pub target_name: String,
    #[serde(default)]
    pub installed_from: Option<String>,
    #[serde(default)]
    pub market_meta: Option<serde_json::Value>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

fn default_install_enable() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillHubInstallStatus {
    pub task_id: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub progress: Option<f64>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillPoolCreateRequest {
    pub name: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub description: String,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillPoolSaveRequest {
    pub name: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub description: String,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillPoolTagsRequest {
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillPoolImportRequest {
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillPoolDownloadRequest {
    pub skill_name: String,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillPoolImportBuiltinRequest {
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub skill_names: Vec<String>,
    #[serde(default)]
    pub overwrite_conflicts: bool,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuiltinImportSpec {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub version_text: String,
    #[serde(default)]
    pub current_version_text: Option<String>,
    #[serde(default)]
    pub current_source: Option<String>,
    #[serde(default)]
    pub status: String,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuiltinSkillInfo {
    pub name: String,
    #[serde(default)]
    pub version_text: String,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillPoolBuiltinSources {
    #[serde(default)]
    pub sources: Vec<BuiltinImportSpec>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillPoolBuiltinNotice {
    #[serde(default)]
    pub has_updates: bool,
    #[serde(default)]
    pub fingerprint: String,
    #[serde(default)]
    pub total_changes: u32,
    #[serde(default)]
    pub actionable_skill_names: Vec<String>,
    #[serde(default)]
    pub added: Vec<BuiltinSkillInfo>,
    #[serde(default)]
    pub missing: Vec<BuiltinSkillInfo>,
    #[serde(default)]
    pub updated: Vec<BuiltinSkillInfo>,
    #[serde(default)]
    pub removed: Vec<BuiltinSkillInfo>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillPoolConfig {
    #[serde(flatten)]
    pub inner: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchDeleteRequest {
    #[serde(default)]
    pub names: Vec<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchToggleRequest {
    #[serde(default)]
    pub names: Vec<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum BatchToggleBody {
    Array(Vec<String>),
    Object(BatchToggleRequest),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillSaveRequest {
    pub name: String,
    #[serde(default)]
    pub source_name: Option<String>,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub config: Option<serde_json::Value>,
    #[serde(default)]
    pub overwrite: Option<bool>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillChannelsRequest {
    #[serde(default)]
    pub channels: Vec<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillConfig {
    #[serde(flatten)]
    pub inner: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileRenameRequest {
    pub new_name: String,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubdirsInfoResponse {
    #[serde(default)]
    pub subdirs: Vec<SubdirEntry>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubdirFileCreateRequest {
    #[serde(default)]
    pub filename: Option<String>,
    #[serde(default)]
    pub content: String,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemPromptFilesUpdateRequest {
    #[serde(default)]
    pub files: Vec<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    #[serde(rename = "filename")]
    pub name: String,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default, rename = "modified_time")]
    pub modified: String,
    #[serde(default)]
    pub created_time: String,
    #[serde(default)]
    pub builtin: bool,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileContent {
    #[serde(rename = "filename")]
    pub name: String,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub size: u64,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileWriteRequest {
    #[serde(default)]
    pub path: String,
    pub content: String,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubdirEntry {
    pub name: String,
    #[serde(default)]
    pub path: String,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemPromptFileEntry {
    pub name: String,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub readonly: bool,
    #[serde(default)]
    pub builtin: bool,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceImportResult {
    pub success: bool,
    #[serde(default)]
    pub agents_imported: u32,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceInfo {
    #[serde(default)]
    pub workspace_dir: String,
    #[serde(default)]
    pub total_size: Option<u64>,
    #[serde(default)]
    pub file_count: Option<u32>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvVar {
    pub key: String,
    #[serde(default)]
    pub value: String,
    #[serde(default)]
    pub masked: Option<bool>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

pub type EnvVarsList = Vec<EnvVar>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenUsageQuery {
    #[serde(default)]
    pub days: Option<u32>,
    #[serde(default, alias = "model")]
    pub model_name: Option<String>,
    #[serde(default, alias = "provider")]
    pub provider_id: Option<String>,
    #[serde(default)]
    pub start_date: Option<String>,
    #[serde(default)]
    pub end_date: Option<String>,
    #[serde(default)]
    pub agent_id: Option<String>,
    #[serde(default)]
    pub chat_id: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenUsageStats {
    #[serde(default)]
    pub prompt_tokens: u64,
    #[serde(default)]
    pub completion_tokens: u64,
    #[serde(default)]
    pub call_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenUsageByModel {
    #[serde(default)]
    pub provider_id: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub prompt_tokens: u64,
    #[serde(default)]
    pub completion_tokens: u64,
    #[serde(default)]
    pub call_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenUsageRecord {
    pub date: String,
    #[serde(default)]
    pub provider_id: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub prompt_tokens: u64,
    #[serde(default)]
    pub completion_tokens: u64,
    #[serde(default)]
    pub call_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenUsageResponse {
    #[serde(default)]
    pub total_prompt_tokens: u64,
    #[serde(default)]
    pub total_completion_tokens: u64,
    #[serde(default)]
    pub total_calls: u64,

    #[serde(default)]
    pub by_model: std::collections::HashMap<String, TokenUsageByModel>,

    #[serde(default)]
    pub by_provider: std::collections::HashMap<String, TokenUsageStats>,

    #[serde(default)]
    pub by_date: std::collections::HashMap<String, TokenUsageStats>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DailyUsageItem {
    pub date: String,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_cost: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginInfo {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub installed: bool,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginInstallRequest {
    pub plugin_id: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginToggleRequest {
    #[serde(default)]
    pub enabled: bool,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LanguageSetting {
    #[serde(default)]
    pub language: String,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionInfo {
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub build: Option<String>,
    #[serde(default)]
    pub commit: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DebugLogsResponse {
    #[serde(default)]
    pub logs: Vec<String>,
    #[serde(default)]
    pub total: u64,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageSendRequest {
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub agent_id: Option<String>,
    #[serde(default)]
    pub channel: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageSendResponse {
    pub success: bool,
    #[serde(default)]
    pub message_id: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTemplateResponse {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub prompt_files: Vec<String>,
    #[serde(default)]
    pub builtin: bool,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTemplateDetailResponse {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub prompt_files: Vec<String>,
    #[serde(default)]
    pub builtin: bool,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,

    #[serde(default, rename = "files_status")]
    pub prompt_file_status: Vec<PromptFileStatus>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptFileStatus {
    #[serde(default, rename = "filename")]
    pub name: String,
    #[serde(default, rename = "exists")]
    pub available: bool,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTemplateCreate {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub prompt_files: Vec<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTemplateUpdate {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub prompt_files: Option<Vec<String>>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentTemplateResponse {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub system_prompt: String,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub visible_to: Vec<String>,
    #[serde(default)]
    pub builtin: bool,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentTemplateCreate {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub system_prompt: String,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub visible_to: Vec<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentTemplateUpdate {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub system_prompt: Option<String>,
    #[serde(default)]
    pub tools: Option<Vec<String>>,
    #[serde(default)]
    pub visible_to: Option<Vec<String>>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TwilioIncomingRequest {
    #[serde(alias = "CallSid", default)]
    pub call_sid: String,

    #[serde(alias = "From", default)]
    pub from: String,

    #[serde(alias = "To", default)]
    pub to: String,

    #[serde(alias = "CallStatus", default)]
    pub call_status: Option<String>,

    #[serde(alias = "Direction", default)]
    pub direction: Option<String>,

    #[serde(alias = "AccountSid", default)]
    pub account_sid: Option<String>,

    #[serde(alias = "ApiVersion", default)]
    pub api_version: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TwilioStatusCallback {
    #[serde(alias = "CallSid", default)]
    pub call_sid: String,

    #[serde(alias = "CallStatus", default)]
    pub call_status: String,

    #[serde(alias = "CallDuration", default)]
    pub call_duration: Option<String>,

    #[serde(alias = "From", default)]
    pub from: Option<String>,

    #[serde(alias = "To", default)]
    pub to: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceWsQuery {
    #[serde(default)]
    pub token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ConversationRelayEvent {
    Setup {
        #[serde(alias = "callSid", default)]
        call_sid: Option<String>,
        #[serde(alias = "from", default)]
        from: Option<String>,
        #[serde(alias = "to", default)]
        to: Option<String>,
        #[serde(flatten)]
        extra: serde_json::Map<String, serde_json::Value>,
    },

    Prompt {
        #[serde(alias = "voicePrompt", default)]
        voice_prompt: Option<String>,
        #[serde(flatten)]
        extra: serde_json::Map<String, serde_json::Value>,
    },

    Interrupt {
        #[serde(alias = "utteranceUntilInterrupt", default)]
        utterance_until_interrupt: Option<String>,
        #[serde(flatten)]
        extra: serde_json::Map<String, serde_json::Value>,
    },

    Dtmf {
        #[serde(default)]
        digit: Option<String>,
        #[serde(flatten)]
        extra: serde_json::Map<String, serde_json::Value>,
    },

    Transcript {
        #[serde(default)]
        transcript: Option<String>,
        #[serde(flatten)]
        extra: serde_json::Map<String, serde_json::Value>,
    },

    Close {
        #[serde(flatten)]
        extra: serde_json::Map<String, serde_json::Value>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ConversationRelayResponse {
    Text {
        token: String,
        #[serde(default)]
        last: bool,
    },

    End {},
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceWsToken {
    pub token: String,
    pub expires_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SkillOptimizeEvent {
    Delta { content: String },

    Done { done: bool },

    Error { detail: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatSearchResult {
    pub chat_id: String,
    pub chat_name: String,
    pub card_index: usize,
    pub role: String,
    pub snippet: String,
    pub message_id: String,
    pub turn_id: String,
    pub sequence: u64,
    pub span_start_sequence: u64,
    pub span_end_sequence: u64,
    #[serde(default)]
    pub preview: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatSearchResponse {
    pub results: Vec<ChatSearchResult>,
    pub offset: usize,
    pub limit: usize,
    pub total: usize,
    #[serde(default)]
    pub next_offset: Option<usize>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}
fn deserialize_explicit_value_option<'de, D>(
    deserializer: D,
) -> std::result::Result<Option<serde_json::Value>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    serde_json::Value::deserialize(deserializer).map(Some)
}
