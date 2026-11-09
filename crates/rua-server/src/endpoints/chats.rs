use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use agent_types::conversation_message_identity::{
    tool_result_message_id, tool_use_message_id, CANONICAL_MESSAGE_ID_METADATA_KEY,
    REASONING_MESSAGE_ID_METADATA_KEY, TEXT_MESSAGE_ID_METADATA_KEY,
};
use axum::extract::{Path as AxumPath, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post, put};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use rua_session::{
    ChatMetadata, ChatSessionHeader, MessageLogEntry, NewConversationMessage, NewConversationTurn,
    TurnIndexRecord, TurnLifecycle, WorkspaceSessionStore,
};
use rua_workspace::WorkspaceRuntime;
use serde::{Deserialize, Serialize};

use crate::endpoints::dto::ChatModelDto;
use crate::errors::ApiError;
use crate::extractors::AgentExtractorContext;
use crate::services::chat_device_activity::{
    apply_chat_created, copy_chat_activity_to_header_extra, device_id_from_sync_device,
    ChatDeviceActivityUpdate,
};
use crate::services::chat_index;
use crate::services::chat_metadata_store::ChatMetadataRecord;
use crate::services::conversation_authority::{
    ConversationControlView, ConversationInputEntrySnapshot,
};
use crate::services::conversation_authority_placement::{
    chat_acp_binding_active, contains_conversation_authority_placement, ConversationAuthorityEpoch,
    ConversationAuthorityPlacement, CHAT_ACP_BINDING_META_KEY, CHAT_AUTHORITY_PLACEMENT_META_KEY,
    CHAT_MODEL_META_KEY,
};
use crate::services::conversation_control_dispatcher::dispatch_conversation_control;
use crate::services::device_command_bus::{
    deliver_conversation_authority_adopt_via_session, spawn_conversation_authority_supersession,
    ConversationAuthorityAdoptDeviceCommand, ConversationControlDeviceAction,
    ConversationControlDeviceCommand, ConversationControlDeviceResponse,
    DeviceCommandDeliveryError,
};
use crate::state::AppState;
use crate::sync_publication::{publish_sync_fact, SyncPublicationFact};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatSpecDto {
    pub id: String,
    pub name: String,
    pub user_id: String,
    pub channel: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub meta: serde_json::Value,
    #[serde(default = "default_status")]
    pub status: String,
    #[serde(default)]
    pub model: Option<ChatModelDto>,
    #[serde(default)]
    pub collaboration_mode: rua_core::AgentCollaborationMode,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub pin_order: i64,
    #[serde(default)]
    pub tag: rua_chat::ChatTag,
    #[serde(default)]
    pub archived: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collaboration: Option<CollaborationChatDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollaborationChatDto {
    pub source: rua_core::ChatAddress,
    pub executor: rua_session::collaboration::CollaborationExecutor,
    pub executor_agent_id: String,
    pub configuration: rua_session::collaboration::CollaborationConfigurationState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_available: Option<bool>,
}

fn collaboration_from_meta(meta: &serde_json::Value) -> Option<CollaborationChatDto> {
    let descriptor: rua_session::collaboration::CollaborationDescriptor = serde_json::from_value(
        meta.get(rua_session::collaboration::COLLABORATION_META_KEY)?
            .clone(),
    )
    .ok()?;
    Some(CollaborationChatDto {
        source: descriptor.source,
        executor: descriptor.executor,
        executor_agent_id: descriptor.executor_agent_id,
        configuration: descriptor.configuration,
        source_available: None,
    })
}

async fn publish_chat_index_upsert(
    state: &AppState,
    agent_id: &str,
    chat_id: &str,
    chat: serde_json::Value,
) {
    let _ = publish_sync_fact(
        state,
        SyncPublicationFact::ChatIndexUpserted {
            address: rua_core::ChatAddress::agent(agent_id, chat_id),
            chat,
        },
    )
    .await;
}

async fn publish_evergreen_chat_snapshot(
    state: &AppState,
    ws_dir: &Path,
    agent_id: &str,
    chat_id: &str,
) {
    let _ = publish_sync_fact(
        state,
        SyncPublicationFact::StableConversationState {
            scope_root: ws_dir.to_path_buf(),
            address: rua_core::ChatAddress::agent(agent_id, chat_id),
        },
    )
    .await;
}

fn default_status() -> String {
    "idle".to_string()
}

async fn normalize_chat_model_thinking(state: &AppState, model: &mut ChatModelDto) {
    if model.provider_id.trim().is_empty() || model.model.trim().is_empty() {
        return;
    }
    let config = state
        .provider_cells
        .cell(&model.provider_id)
        .read()
        .await
        .ok()
        .flatten();
    let entry = config.as_ref().and_then(|config| {
        config
            .all_models()
            .into_iter()
            .find(|entry| entry.id == model.model)
    });
    let knowledge = rua_provider::model_knowledge::lookup(&model.provider_id, &model.model);

    if model.thinking_param_style.is_none() {
        model.thinking_param_style = entry
            .as_ref()
            .and_then(|entry| entry.thinking_param_style.clone())
            .or_else(|| {
                config
                    .as_ref()
                    .and_then(|config| config.thinking_param_style.clone())
            })
            .or_else(|| {
                knowledge.and_then(|knowledge| {
                    knowledge.reasoning_style.map(|style| match style {
                        rua_provider::model_knowledge::ReasoningStyle::Effort => {
                            "effort".to_string()
                        }
                        rua_provider::model_knowledge::ReasoningStyle::Budget => {
                            "budget".to_string()
                        }
                    })
                })
            })
            .or_else(|| {
                config
                    .as_ref()
                    .and_then(|config| config.protocol_thinking_param_style().map(str::to_string))
            });
    }
    if model.thinking_budget_range.is_none() {
        model.thinking_budget_range = entry
            .as_ref()
            .and_then(|entry| entry.thinking_budget_range)
            .or_else(|| {
                config
                    .as_ref()
                    .and_then(|config| config.thinking_budget_range)
            })
            .or_else(|| knowledge.and_then(|knowledge| knowledge.thinking_budget_range));
    }
    if model.reasoning_effort_options.is_empty() {
        model.reasoning_effort_options = entry
            .as_ref()
            .map(|entry| entry.reasoning_effort_options.clone())
            .filter(|options| !options.is_empty())
            .or_else(|| {
                config.as_ref().and_then(|config| {
                    (!config.reasoning_effort_options.is_empty())
                        .then(|| config.reasoning_effort_options.clone())
                })
            })
            .or_else(|| {
                knowledge.and_then(|knowledge| {
                    (!knowledge.reasoning_effort_options.is_empty()).then(|| {
                        knowledge
                            .reasoning_effort_options
                            .iter()
                            .map(|option| (*option).to_string())
                            .collect()
                    })
                })
            })
            .unwrap_or_default();
    }

    if model.thinking_strength.is_none() {
        model.thinking_strength = match (
            model.thinking_enabled,
            model.reasoning_effort.as_deref(),
            model.thinking_budget,
        ) {
            (Some(false), _, _) => Some("off".to_string()),
            (_, Some("low" | "medium" | "high"), _) => model.reasoning_effort.clone(),
            _ => Some("auto".to_string()),
        };
    }
    model.thinking_enabled = None;
    model.thinking_budget = None;
    model.reasoning_effort = None;
}

#[derive(Debug, Deserialize)]
struct ListChatsParams {
    #[serde(default)]
    user_id: Option<String>,
    #[serde(default)]
    channel: Option<String>,
    #[serde(default)]
    search: Option<String>,
    #[serde(default)]
    tag: Option<rua_chat::ChatTag>,
    #[serde(default)]
    offset: Option<u64>,
    #[serde(default)]
    limit: Option<u64>,
    #[serde(default)]
    sort_by: rua_chat::ChatSortField,
    #[serde(default)]
    sort_order: rua_chat::SortOrder,
    #[serde(default = "default_conversation_view")]
    view: rua_session::chat_index::ChatConversationView,
    #[serde(default)]
    source_chat_id: Option<String>,
    #[serde(default)]
    executor: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    archived: Option<bool>,
}

pub(crate) fn default_conversation_view() -> rua_session::chat_index::ChatConversationView {
    rua_session::chat_index::ChatConversationView::User
}

#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct ChatHistoryParams {
    #[serde(default)]
    turn_limit: Option<usize>,
    #[serde(default)]
    before_turn_ordinal: Option<u64>,
    #[serde(default)]
    around_turn_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CreateChatBody {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    user_id: Option<String>,
    #[serde(default)]
    channel: Option<String>,
    #[serde(default)]
    meta: Option<serde_json::Value>,
    #[serde(default)]
    model: Option<ChatModelDto>,
    #[serde(default)]
    collaboration_mode: Option<rua_core::AgentCollaborationMode>,
    #[serde(default)]
    authority_device_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChatUpdateBody {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    user_id: Option<String>,
    #[serde(default)]
    channel: Option<String>,
    #[serde(default)]
    created_at: Option<serde_json::Value>,
    #[serde(default)]
    updated_at: Option<serde_json::Value>,
    #[serde(default)]
    meta: Option<serde_json::Value>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    model: Option<ChatModelDto>,
    #[serde(default)]
    collaboration_mode: Option<rua_core::AgentCollaborationMode>,
    #[serde(default)]
    pinned: Option<bool>,
    #[serde(default)]
    pin_order: Option<i64>,
    #[serde(default)]
    tag: Option<rua_chat::ChatTag>,
    #[serde(default)]
    archived: Option<bool>,
    #[serde(default)]
    turn_id: Option<String>,
    #[serde(default)]
    selected_variant_id: Option<String>,
    #[serde(default)]
    update_configuration: bool,
}

#[derive(Debug, Deserialize)]
struct ConversationAuthorityPlacementUpdateBody {
    device_id: String,
}

#[derive(Debug, Deserialize)]
struct QueueInputEditBody {
    message: String,
}

#[derive(Debug, Serialize)]
struct QueueInputEditResponse {
    entry: ConversationInputEntrySnapshot,
    control: ConversationControlView,
}

#[derive(Debug, Serialize)]
struct QueueInputDeleteResponse {
    deleted_input_id: String,
    control: ConversationControlView,
}

#[derive(Debug, Deserialize)]
struct QueueInputSendNowBody {
    command_id: String,
    authority_epoch: ConversationAuthorityEpoch,
}

#[derive(Debug, Serialize)]
struct QueueInputSendNowResponse {
    input_id: String,
    applied: bool,
    control: ConversationControlView,
}

#[derive(Debug, Deserialize)]
pub(crate) struct AcpHistoryPreviewBody {
    preview_id: u64,
    text: String,
    #[serde(default)]
    runner: Option<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct AcpHistoryPreviewResponse {
    turn_id: String,
    message_id: String,
    created: bool,
}

const ACP_HISTORY_PREVIEW_MESSAGE_TYPE: &str = "acp_history_preview";

#[derive(Debug, Serialize, Deserialize)]
pub struct DurableConversationCoverageDto {
    pub latest_message_sequence: u64,
    pub manifest_hash: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ChatHistoryDto {
    pub messages: Vec<serde_json::Value>,
    pub model: Option<ChatModelDto>,
    #[serde(default)]
    pub meta: serde_json::Value,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub oldest_turn_ordinal: Option<u64>,
    #[serde(default)]
    pub newest_turn_ordinal: Option<u64>,
    #[serde(default)]
    pub has_more_before: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub durable_coverage: Option<DurableConversationCoverageDto>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ProjectionExclusionsDto {
    #[serde(default)]
    message_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct PinOrderItem {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    pin_order: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct BranchActionBody {
    before_turn_id: String,
}

#[derive(Debug, Serialize)]
struct BranchActionResponse {
    chat: ChatSpecDto,
}

pub fn root_router() -> Router<AppState> {
    Router::new()
        .route("/chats", get(list_chats_root).post(create_chat_root))
        .route("/chats/pin-order", put(update_pin_order_root))
        .route("/chats/batch-delete", post(batch_delete_root))
        .route("/chats/{id}/rollback", post(rollback_chat_root))
        .route("/chats/{id}/branch", post(branch_chat_root))
        .route(
            "/chats/{id}/acp-history-preview",
            post(persist_acp_history_preview_root),
        )
        .route(
            "/chats/{id}/authority-placement",
            put(update_authority_placement_root),
        )
        .route(
            "/chats/{id}",
            get(get_chat_root)
                .put(update_chat_root)
                .delete(delete_chat_root),
        )
}

pub fn agent_scoped_router() -> Router<AppState> {
    Router::new()
        .route(
            "/agents/{agentId}/chats",
            get(list_chats_agent).post(create_chat_agent),
        )
        .route(
            "/agents/{agentId}/chats/pin-order",
            put(update_pin_order_agent),
        )
        .route(
            "/agents/{agentId}/chats/batch-delete",
            post(batch_delete_agent),
        )
        .route(
            "/agents/{agentId}/chats/{id}/rollback",
            post(rollback_chat_agent),
        )
        .route(
            "/agents/{agentId}/chats/{id}/projection-exclusions",
            get(get_projection_exclusions_agent).put(put_projection_exclusions_agent),
        )
        .route(
            "/agents/{agentId}/chats/{id}/branch",
            post(branch_chat_agent),
        )
        .route(
            "/agents/{agentId}/chats/{id}/acp-history-preview",
            post(persist_acp_history_preview_agent),
        )
        .route(
            "/agents/{agentId}/chats/{id}/authority-placement",
            put(update_authority_placement_agent),
        )
        .route(
            "/agents/{agentId}/chats/{id}/running-config",
            get(get_chat_running_config_agent).put(put_chat_running_config_agent),
        )
        .route(
            "/agents/{agentId}/chats/{id}/metadata",
            get(get_chat_metadata_agent),
        )
        .route(
            "/agents/{agentId}/chats/{id}/configuration",
            get(preview_chat_configuration_agent).put(apply_chat_configuration_agent),
        )
        .route(
            "/agents/{agentId}/chats/{id}/queue/{inputId}",
            patch(edit_queued_input_agent).delete(delete_queued_input_agent),
        )
        .route(
            "/agents/{agentId}/chats/{id}/queue/{inputId}/send-now",
            post(send_now_queued_input_agent),
        )
        .route(
            "/agents/{agentId}/chats/{id}",
            get(get_chat_agent)
                .put(update_chat_agent)
                .delete(delete_chat_agent),
        )
}

pub(crate) async fn dispatch_agent_chat_control(
    state: &AppState,
    agent_id: &str,
    ws_dir: &Path,
    chat_id: &str,
    action: ConversationControlDeviceAction,
) -> Result<ConversationControlDeviceResponse, ApiError> {
    let placement =
        crate::services::conversation_authority_target::load_optional_chat_authority_placement(
            ws_dir, chat_id,
        )
        .await
        .map_err(ApiError::Internal)?
        .ok_or_else(|| {
            ApiError::Conflict(format!(
                "conversation authority placement not found for Chat '{chat_id}'"
            ))
        })?;
    let source_device_id = device_id_from_sync_device(&state.sync_device)
        .ok_or_else(|| ApiError::Unavailable("local Device identity is unavailable".to_string()))?;
    dispatch_conversation_control(
        &state.conversation_input_intake,
        ConversationControlDeviceCommand {
            command_id: format!("chat-running-config-{}", uuid::Uuid::new_v4()),
            agent_id: agent_id.to_string(),
            address: rua_core::ChatAddress::agent(agent_id, chat_id),
            source_device_id,
            target_device_id: placement.device_id,
            authority_epoch: placement.authority_epoch,
            action,
        },
    )
    .await
    .map_err(|error| chat_command_delivery_error(error, "Chat options"))
}

#[derive(Deserialize)]
pub(crate) struct ChatConfigurationApply {
    pub expected_epoch: String,
    pub candidate_epoch: String,
}

async fn preview_chat_configuration_agent(
    State(state): State<AppState>,
    AxumPath((agent_id, chat_id)): AxumPath<(String, String)>,
) -> Response {
    chat_configuration_agent(
        &state,
        &agent_id,
        &chat_id,
        ConversationControlDeviceAction::ViewConfiguration,
    )
    .await
}

async fn apply_chat_configuration_agent(
    State(state): State<AppState>,
    AxumPath((agent_id, chat_id)): AxumPath<(String, String)>,
    Json(body): Json<ChatConfigurationApply>,
) -> Response {
    chat_configuration_agent(
        &state,
        &agent_id,
        &chat_id,
        ConversationControlDeviceAction::UpdateConfiguration {
            expected_epoch: body.expected_epoch,
            candidate_epoch: body.candidate_epoch,
        },
    )
    .await
}

async fn chat_configuration_agent(
    state: &AppState,
    agent_id: &str,
    chat_id: &str,
    action: ConversationControlDeviceAction,
) -> Response {
    let runtime = match runtime_for_agent(state, agent_id).await {
        Ok(runtime) => runtime,
        Err(error) => return error.into_response(),
    };
    match dispatch_agent_chat_control(state, agent_id, &runtime.workspace_dir(), chat_id, action)
        .await
    {
        Ok(ConversationControlDeviceResponse::Configuration { configuration }) => {
            Json(configuration).into_response()
        }
        Ok(_) => {
            ApiError::Internal("unexpected Chat configuration response".to_string()).into_response()
        }
        Err(error) => error.into_response(),
    }
}

async fn get_chat_running_config_agent(
    State(state): State<AppState>,
    AxumPath((agent_id, chat_id)): AxumPath<(String, String)>,
) -> Response {
    let runtime = match runtime_for_agent(&state, &agent_id).await {
        Ok(runtime) => runtime,
        Err(error) => return error.into_response(),
    };
    match dispatch_agent_chat_control(
        &state,
        &agent_id,
        &runtime.workspace_dir(),
        &chat_id,
        ConversationControlDeviceAction::ViewRunningConfig,
    )
    .await
    {
        Ok(ConversationControlDeviceResponse::RunningConfig { running }) => {
            Json(running).into_response()
        }
        Ok(_) => ApiError::Internal("unexpected Chat options response".to_string()).into_response(),
        Err(error) => error.into_response(),
    }
}

async fn put_chat_running_config_agent(
    State(state): State<AppState>,
    AxumPath((agent_id, chat_id)): AxumPath<(String, String)>,
    Json(running): Json<serde_json::Value>,
) -> Response {
    let runtime = match runtime_for_agent(&state, &agent_id).await {
        Ok(runtime) => runtime,
        Err(error) => return error.into_response(),
    };
    match dispatch_agent_chat_control(
        &state,
        &agent_id,
        &runtime.workspace_dir(),
        &chat_id,
        ConversationControlDeviceAction::UpdateRunningConfig { running },
    )
    .await
    {
        Ok(ConversationControlDeviceResponse::RunningConfigUpdated { running, .. }) => {
            Json(running).into_response()
        }
        Ok(_) => ApiError::Internal("unexpected Chat options update response".to_string())
            .into_response(),
        Err(error) => error.into_response(),
    }
}

async fn list_chats_root(
    State(state): State<AppState>,
    AgentExtractorContext { agent_id, runtime }: AgentExtractorContext,
    Query(params): Query<ListChatsParams>,
) -> Response {
    list_chats_core(&state, &agent_id, &runtime.workspace_dir(), params).await
}

async fn create_chat_root(
    State(state): State<AppState>,
    AgentExtractorContext { .. }: AgentExtractorContext,
    Json(body): Json<CreateChatBody>,
) -> Response {
    create_chat_core(body, &state).await
}

async fn get_chat_root(
    State(state): State<AppState>,
    AgentExtractorContext { agent_id, runtime }: AgentExtractorContext,
    AxumPath(chat_id): AxumPath<String>,
    Query(params): Query<ChatHistoryParams>,
) -> Response {
    let address = rua_core::ChatAddress::agent(&agent_id, &chat_id);
    get_chat_core(&state, &address, &runtime.workspace_dir(), &chat_id, params).await
}

async fn update_chat_root(
    State(state): State<AppState>,
    AgentExtractorContext { agent_id, runtime }: AgentExtractorContext,
    AxumPath(chat_id): AxumPath<String>,
    Json(body): Json<ChatUpdateBody>,
) -> Response {
    update_chat_core(&agent_id, &runtime.workspace_dir(), &chat_id, body, &state).await
}

async fn update_authority_placement_root(
    State(state): State<AppState>,
    AgentExtractorContext { agent_id, runtime }: AgentExtractorContext,
    AxumPath(chat_id): AxumPath<String>,
    Json(body): Json<ConversationAuthorityPlacementUpdateBody>,
) -> Response {
    update_authority_placement_core(&agent_id, &runtime.workspace_dir(), &chat_id, body, &state)
        .await
}

async fn delete_chat_root(
    State(state): State<AppState>,
    AgentExtractorContext { agent_id, runtime }: AgentExtractorContext,
    AxumPath(chat_id): AxumPath<String>,
) -> Response {
    delete_chat_core(&agent_id, &runtime.workspace_dir(), &chat_id, &state).await
}

async fn rollback_chat_root(
    State(state): State<AppState>,
    AgentExtractorContext { agent_id, runtime }: AgentExtractorContext,
    AxumPath(chat_id): AxumPath<String>,
    Json(body): Json<BranchActionBody>,
) -> Response {
    rollback_chat_core(&agent_id, &runtime.workspace_dir(), &chat_id, body, &state).await
}

async fn branch_chat_root(
    State(state): State<AppState>,
    AgentExtractorContext { agent_id, runtime }: AgentExtractorContext,
    AxumPath(chat_id): AxumPath<String>,
    Json(body): Json<BranchActionBody>,
) -> Response {
    branch_chat_core(&agent_id, &runtime.workspace_dir(), &chat_id, body, &state).await
}

async fn persist_acp_history_preview_root(
    State(state): State<AppState>,
    AgentExtractorContext { agent_id, runtime }: AgentExtractorContext,
    AxumPath(chat_id): AxumPath<String>,
    Json(body): Json<AcpHistoryPreviewBody>,
) -> Response {
    persist_acp_history_preview_core(
        &rua_core::ChatAddress::agent(&agent_id, &chat_id),
        &agent_id,
        &runtime.workspace_dir(),
        &chat_id,
        body,
        &state,
    )
    .await
}

async fn update_pin_order_root(
    State(state): State<AppState>,
    AgentExtractorContext { agent_id, runtime }: AgentExtractorContext,
    Json(items): Json<Vec<PinOrderItem>>,
) -> Response {
    update_pin_order_core(&agent_id, &runtime.workspace_dir(), items, &state).await
}

async fn batch_delete_root(
    State(state): State<AppState>,
    AgentExtractorContext { agent_id, runtime }: AgentExtractorContext,
    Json(ids): Json<Vec<String>>,
) -> Response {
    batch_delete_core(&agent_id, &runtime.workspace_dir(), ids, &state).await
}

async fn list_chats_agent(
    State(state): State<AppState>,
    AxumPath(agent_id): AxumPath<String>,
    Query(params): Query<ListChatsParams>,
) -> Response {
    let runtime = match runtime_for_agent(&state, &agent_id).await {
        Ok(rt) => rt,
        Err(e) => return e.into_response(),
    };
    list_chats_core(&state, &agent_id, &runtime.workspace_dir(), params).await
}

async fn create_chat_agent(
    State(state): State<AppState>,
    AxumPath(agent_id): AxumPath<String>,
    Json(body): Json<CreateChatBody>,
) -> Response {
    match state.agent_service.get_agent(&agent_id).await {
        Ok(Some(_)) => create_chat_core(body, &state).await,
        Ok(None) => ApiError::NotFound(format!("Agent not found: {agent_id}")).into_response(),
        Err(error) => ApiError::Internal(error.to_string()).into_response(),
    }
}

async fn get_chat_metadata_agent(
    State(state): State<AppState>,
    AxumPath((agent_id, chat_id)): AxumPath<(String, String)>,
) -> Response {
    let agent = match state.agent_service.get_agent(&agent_id).await {
        Ok(Some(agent)) => agent,
        Ok(None) => {
            return ApiError::NotFound(format!("Agent '{agent_id}' not found")).into_response()
        }
        Err(error) => return ApiError::from(error).into_response(),
    };
    get_chat_metadata_core(Path::new(&agent.workspace_dir), &chat_id).await
}

async fn get_chat_agent(
    State(state): State<AppState>,
    AxumPath((agent_id, chat_id)): AxumPath<(String, String)>,
    Query(params): Query<ChatHistoryParams>,
) -> Response {
    let runtime = match runtime_for_agent(&state, &agent_id).await {
        Ok(rt) => rt,
        Err(e) => return e.into_response(),
    };
    let address = rua_core::ChatAddress::agent(&agent_id, &chat_id);
    get_chat_core(&state, &address, &runtime.workspace_dir(), &chat_id, params).await
}

async fn update_chat_agent(
    State(state): State<AppState>,
    AxumPath((agent_id, chat_id)): AxumPath<(String, String)>,
    Json(body): Json<ChatUpdateBody>,
) -> Response {
    let runtime = match runtime_for_agent(&state, &agent_id).await {
        Ok(rt) => rt,
        Err(e) => return e.into_response(),
    };
    update_chat_core(&agent_id, &runtime.workspace_dir(), &chat_id, body, &state).await
}

async fn update_authority_placement_agent(
    State(state): State<AppState>,
    AxumPath((agent_id, chat_id)): AxumPath<(String, String)>,
    Json(body): Json<ConversationAuthorityPlacementUpdateBody>,
) -> Response {
    let runtime = match runtime_for_agent(&state, &agent_id).await {
        Ok(rt) => rt,
        Err(e) => return e.into_response(),
    };
    update_authority_placement_core(&agent_id, &runtime.workspace_dir(), &chat_id, body, &state)
        .await
}

async fn edit_queued_input_agent(
    State(state): State<AppState>,
    AxumPath((agent_id, chat_id, input_id)): AxumPath<(String, String, String)>,
    Json(body): Json<QueueInputEditBody>,
) -> Response {
    if body.message.trim().is_empty() {
        return ApiError::BadRequest("message cannot be empty".to_string()).into_response();
    }

    let runtime = match runtime_for_agent(&state, &agent_id).await {
        Ok(rt) => rt,
        Err(e) => return e.into_response(),
    };
    match remote_conversation_control_if_needed(
        &state,
        &agent_id,
        &runtime.workspace_dir(),
        &chat_id,
        ConversationControlDeviceAction::EditQueuedInput {
            input_id: input_id.clone(),
            message: body.message.clone(),
        },
        None,
    )
    .await
    {
        Ok(Some(ConversationControlDeviceResponse::Edited { entry, control })) => {
            return (
                StatusCode::OK,
                Json(QueueInputEditResponse { entry, control }),
            )
                .into_response();
        }
        Ok(Some(_)) => {
            return ApiError::Internal("unexpected remote queue edit response".to_string())
                .into_response();
        }
        Ok(None) => {}
        Err(error) => return error.into_response(),
    }
    let entry = match state
        .conversation_authority
        .edit_queued_input(
            &rua_core::ChatAddress::agent(&agent_id, &chat_id),
            &input_id,
            &body.message,
        )
        .await
    {
        Some(entry) => entry,
        None => {
            return ApiError::Conflict("queued input is not editable".to_string()).into_response();
        }
    };
    let control =
        match control_view_for_chat(&state, &agent_id, &runtime.workspace_dir(), &chat_id).await {
            Ok(control) => control,
            Err(e) => return e.into_response(),
        };

    (
        StatusCode::OK,
        Json(QueueInputEditResponse { entry, control }),
    )
        .into_response()
}

async fn delete_queued_input_agent(
    State(state): State<AppState>,
    AxumPath((agent_id, chat_id, input_id)): AxumPath<(String, String, String)>,
) -> Response {
    let runtime = match runtime_for_agent(&state, &agent_id).await {
        Ok(rt) => rt,
        Err(e) => return e.into_response(),
    };
    match remote_conversation_control_if_needed(
        &state,
        &agent_id,
        &runtime.workspace_dir(),
        &chat_id,
        ConversationControlDeviceAction::DeleteQueuedInput {
            input_id: input_id.clone(),
        },
        None,
    )
    .await
    {
        Ok(Some(ConversationControlDeviceResponse::Deleted {
            deleted_input_id,
            control,
        })) => {
            return (
                StatusCode::OK,
                Json(QueueInputDeleteResponse {
                    deleted_input_id,
                    control,
                }),
            )
                .into_response();
        }
        Ok(Some(_)) => {
            return ApiError::Internal("unexpected remote queue delete response".to_string())
                .into_response();
        }
        Ok(None) => {}
        Err(error) => return error.into_response(),
    }
    let deleted = match state
        .conversation_authority
        .delete_queued_input(
            &rua_core::ChatAddress::agent(&agent_id, &chat_id),
            &input_id,
        )
        .await
    {
        Some(entry) => entry,
        None => {
            return ApiError::Conflict("queued input is not editable".to_string()).into_response();
        }
    };
    let control =
        match control_view_for_chat(&state, &agent_id, &runtime.workspace_dir(), &chat_id).await {
            Ok(control) => control,
            Err(e) => return e.into_response(),
        };

    (
        StatusCode::OK,
        Json(QueueInputDeleteResponse {
            deleted_input_id: deleted.input_id,
            control,
        }),
    )
        .into_response()
}

async fn send_now_queued_input_agent(
    State(state): State<AppState>,
    AxumPath((agent_id, chat_id, input_id)): AxumPath<(String, String, String)>,
    Json(body): Json<QueueInputSendNowBody>,
) -> Response {
    let runtime = match runtime_for_agent(&state, &agent_id).await {
        Ok(rt) => rt,
        Err(e) => return e.into_response(),
    };
    match remote_conversation_control_if_needed(
        &state,
        &agent_id,
        &runtime.workspace_dir(),
        &chat_id,
        ConversationControlDeviceAction::SendNow {
            input_id: input_id.clone(),
            control_command_id: body.command_id.clone(),
        },
        Some(body.authority_epoch),
    )
    .await
    {
        Ok(Some(ConversationControlDeviceResponse::SendNow {
            input_id,
            applied,
            control,
        })) => {
            return (
                StatusCode::OK,
                Json(QueueInputSendNowResponse {
                    input_id,
                    applied,
                    control,
                }),
            )
                .into_response();
        }
        Ok(Some(_)) => {
            return ApiError::Internal("unexpected remote Send Now response".to_string())
                .into_response();
        }
        Ok(None) => {}
        Err(error) => return error.into_response(),
    }
    let entry = match state
        .conversation_input_intake
        .send_now(
            &agent_id,
            &chat_id,
            &input_id,
            body.authority_epoch,
            &body.command_id,
        )
        .await
    {
        Ok(entry) => entry,
        Err(detail) => return ApiError::Conflict(detail).into_response(),
    };
    let control =
        match control_view_for_chat(&state, &agent_id, &runtime.workspace_dir(), &chat_id).await {
            Ok(control) => control,
            Err(e) => return e.into_response(),
        };
    (
        StatusCode::OK,
        Json(QueueInputSendNowResponse {
            input_id: entry.input_id,
            applied: true,
            control,
        }),
    )
        .into_response()
}

async fn delete_chat_agent(
    State(state): State<AppState>,
    AxumPath((agent_id, chat_id)): AxumPath<(String, String)>,
) -> Response {
    let runtime = match runtime_for_agent(&state, &agent_id).await {
        Ok(rt) => rt,
        Err(e) => return e.into_response(),
    };
    delete_chat_core(&agent_id, &runtime.workspace_dir(), &chat_id, &state).await
}

async fn persist_acp_history_preview_agent(
    State(state): State<AppState>,
    AxumPath((agent_id, chat_id)): AxumPath<(String, String)>,
    Json(body): Json<AcpHistoryPreviewBody>,
) -> Response {
    let runtime = match runtime_for_agent(&state, &agent_id).await {
        Ok(rt) => rt,
        Err(e) => return e.into_response(),
    };
    persist_acp_history_preview_core(
        &rua_core::ChatAddress::agent(&agent_id, &chat_id),
        &agent_id,
        &runtime.workspace_dir(),
        &chat_id,
        body,
        &state,
    )
    .await
}

async fn rollback_chat_agent(
    State(state): State<AppState>,
    AxumPath((agent_id, chat_id)): AxumPath<(String, String)>,
    Json(body): Json<BranchActionBody>,
) -> Response {
    let runtime = match runtime_for_agent(&state, &agent_id).await {
        Ok(rt) => rt,
        Err(e) => return e.into_response(),
    };
    rollback_chat_core(&agent_id, &runtime.workspace_dir(), &chat_id, body, &state).await
}

async fn get_projection_exclusions_agent(
    State(state): State<AppState>,
    AxumPath((agent_id, chat_id)): AxumPath<(String, String)>,
) -> Response {
    let runtime = match runtime_for_agent(&state, &agent_id).await {
        Ok(runtime) => runtime,
        Err(error) => return error.into_response(),
    };
    match dispatch_agent_chat_control(
        &state,
        &agent_id,
        &runtime.workspace_dir(),
        &chat_id,
        ConversationControlDeviceAction::ViewProjectionExclusions,
    )
    .await
    {
        Ok(ConversationControlDeviceResponse::ProjectionExclusions { message_ids }) => {
            Json(ProjectionExclusionsDto { message_ids }).into_response()
        }
        Ok(_) => ApiError::Internal("unexpected projection exclusions response".to_string())
            .into_response(),
        Err(error) => error.into_response(),
    }
}

async fn put_projection_exclusions_agent(
    State(state): State<AppState>,
    AxumPath((agent_id, chat_id)): AxumPath<(String, String)>,
    Json(body): Json<ProjectionExclusionsDto>,
) -> Response {
    let runtime = match runtime_for_agent(&state, &agent_id).await {
        Ok(runtime) => runtime,
        Err(error) => return error.into_response(),
    };
    match dispatch_agent_chat_control(
        &state,
        &agent_id,
        &runtime.workspace_dir(),
        &chat_id,
        ConversationControlDeviceAction::UpdateProjectionExclusions {
            message_ids: body.message_ids,
        },
    )
    .await
    {
        Ok(ConversationControlDeviceResponse::ProjectionExclusionsUpdated { message_ids }) => {
            Json(ProjectionExclusionsDto { message_ids }).into_response()
        }
        Ok(_) => ApiError::Internal("unexpected projection exclusions update response".to_string())
            .into_response(),
        Err(error) => error.into_response(),
    }
}

async fn branch_chat_agent(
    State(state): State<AppState>,
    AxumPath((agent_id, chat_id)): AxumPath<(String, String)>,
    Json(body): Json<BranchActionBody>,
) -> Response {
    let runtime = match runtime_for_agent(&state, &agent_id).await {
        Ok(rt) => rt,
        Err(e) => return e.into_response(),
    };
    branch_chat_core(&agent_id, &runtime.workspace_dir(), &chat_id, body, &state).await
}

async fn update_pin_order_agent(
    State(state): State<AppState>,
    AxumPath(agent_id): AxumPath<String>,
    Json(items): Json<Vec<PinOrderItem>>,
) -> Response {
    let runtime = match runtime_for_agent(&state, &agent_id).await {
        Ok(rt) => rt,
        Err(e) => return e.into_response(),
    };
    update_pin_order_core(&agent_id, &runtime.workspace_dir(), items, &state).await
}

async fn batch_delete_agent(
    State(state): State<AppState>,
    AxumPath(agent_id): AxumPath<String>,
    Json(ids): Json<Vec<String>>,
) -> Response {
    let runtime = match runtime_for_agent(&state, &agent_id).await {
        Ok(rt) => rt,
        Err(e) => return e.into_response(),
    };
    batch_delete_core(&agent_id, &runtime.workspace_dir(), ids, &state).await
}

async fn runtime_for_agent(
    state: &AppState,
    agent_id: &str,
) -> Result<Arc<WorkspaceRuntime>, ApiError> {
    state
        .registry
        .get_or_create(agent_id)
        .await
        .map_err(|e| ApiError::Unavailable(format!("workspace for '{agent_id}' unavailable: {e}")))
}

pub(crate) fn chat_to_index_entry(chat: &ChatSpecDto) -> chat_index::ChatIndexEntry {
    let mut meta = chat.meta.clone();
    if !meta.is_object() {
        meta = serde_json::json!({});
    }
    meta.as_object_mut()
        .expect("Chat metadata normalized to an object")
        .insert(
            rua_core::CHAT_COLLABORATION_MODE_META_KEY.to_string(),
            serde_json::json!(chat.collaboration_mode),
        );
    chat_index::ChatIndexEntry {
        id: chat.id.clone(),
        name: chat.name.clone(),
        user_id: chat.user_id.clone(),
        channel: chat.channel.clone(),
        created_at: chat.created_at,
        updated_at: chat.updated_at,
        meta,
        status: chat.status.clone(),
        pinned: chat.pinned,
        pin_order: chat.pin_order,
        tag: chat.tag,
        archived: chat.archived,
    }
}

fn chat_model_value(model: &Option<ChatModelDto>) -> Option<serde_json::Value> {
    model
        .as_ref()
        .and_then(|model| serde_json::to_value(model).ok())
}

fn chat_model_from_header(header: &ChatSessionHeader) -> Option<ChatModelDto> {
    rua_chat::model_slot_from_header(header).and_then(|value| serde_json::from_value(value).ok())
}

fn sync_chat_model_to_header_extra(
    model: &Option<ChatModelDto>,
    extra: &mut serde_json::Map<String, serde_json::Value>,
) {
    if let Some(value) = chat_model_value(model) {
        extra.insert(CHAT_MODEL_META_KEY.to_string(), value);
    } else {
        extra.remove(CHAT_MODEL_META_KEY);
    }
}

async fn load_chat_model_from_header(ws_dir: &Path, chat_id: &str) -> Option<ChatModelDto> {
    WorkspaceSessionStore::new(ws_dir)
        .load_chat_header(chat_id)
        .await
        .ok()
        .flatten()
        .and_then(|header| chat_model_from_header(&header))
}

async fn default_chat_model_slot(state: &AppState) -> Option<ChatModelDto> {
    state
        .conversation_input_intake
        .resolve_initial_authority_chat_model(None)
        .await
        .ok()
        .flatten()
        .and_then(|model| serde_json::from_value(model).ok())
}

pub(crate) fn chat_from_index_entry(entry: chat_index::ChatIndexEntry) -> ChatSpecDto {
    let collaboration_mode = entry
        .meta
        .get(rua_core::CHAT_COLLABORATION_MODE_META_KEY)
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default();
    let collaboration = collaboration_from_meta(&entry.meta);
    ChatSpecDto {
        id: entry.id,
        name: entry.name,
        user_id: entry.user_id,
        channel: entry.channel,
        created_at: entry.created_at,
        updated_at: entry.updated_at,
        meta: entry.meta,
        status: entry.status,
        model: None,
        collaboration_mode,
        pinned: entry.pinned,
        pin_order: entry.pin_order,
        tag: entry.tag,
        archived: entry.archived,
        collaboration,
    }
}

pub(crate) async fn chat_spec_from_common_chat(
    ws_dir: &Path,
    chat: &rua_chat::Chat,
) -> rua_core::Result<ChatSpecDto> {
    let chat_id = chat.address().chat_id().as_str();
    let mut spec = chat_from_index_entry(
        chat_index::get(ws_dir, chat_id)
            .await
            .map_err(|message| rua_core::RuaError::Io { message })?
            .ok_or_else(|| rua_core::RuaError::NotFound {
                resource: "Chat".to_string(),
                id: chat_id.to_string(),
            })?,
    );
    spec.model = chat
        .model()
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|error| rua_core::RuaError::Validation {
            message: error.to_string(),
        })?;
    Ok(spec)
}

fn chat_from_metadata_record(record: ChatMetadataRecord) -> ChatSpecDto {
    let collaboration_mode = record
        .meta
        .get(rua_core::CHAT_COLLABORATION_MODE_META_KEY)
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default();
    let collaboration = collaboration_from_meta(&record.meta);
    ChatSpecDto {
        id: record.id,
        name: record.name,
        user_id: record.user_id,
        channel: record.channel,
        created_at: record.created_at,
        updated_at: record.updated_at,
        meta: record.meta,
        status: record.status,
        model: record
            .model
            .and_then(|model| serde_json::from_value(model).ok()),
        collaboration_mode,
        pinned: record.pinned,
        pin_order: record.pin_order,
        tag: record.tag,
        archived: record.archived,
        collaboration,
    }
}

async fn upsert_chat_index(ws_dir: &Path, chat: &ChatSpecDto) -> Result<(), ApiError> {
    chat_index::upsert(ws_dir, chat_to_index_entry(chat))
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to update chat index: {e}")))
}

async fn control_view_for_chat(
    state: &AppState,
    agent_id: &str,
    ws_dir: &Path,
    chat_id: &str,
) -> Result<ConversationControlView, ApiError> {
    let placement =
        crate::services::conversation_authority_target::load_optional_chat_authority_placement(
            ws_dir, chat_id,
        )
        .await
        .map_err(ApiError::Internal)?
        .ok_or_else(|| {
            ApiError::Conflict(format!(
                "conversation authority placement not found for Chat '{chat_id}'"
            ))
        })?;
    let header = WorkspaceSessionStore::new(ws_dir)
        .load_chat_header(chat_id)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::NotFound(format!("chat '{chat_id}' not found")))?;
    let meta = serde_json::Value::Object(header.extra);
    let local_device_id = device_id_from_sync_device(&state.sync_device)
        .ok_or_else(|| ApiError::Unavailable("local Device identity is unavailable".to_string()))?;
    if placement.device_id != local_device_id {
        let response = dispatch_conversation_control(
            &state.conversation_input_intake,
            ConversationControlDeviceCommand {
                command_id: format!("control-view-{}", uuid::Uuid::new_v4()),
                agent_id: agent_id.to_string(),
                address: rua_core::ChatAddress::agent(agent_id, chat_id),
                source_device_id: local_device_id,
                target_device_id: placement.device_id,
                authority_epoch: placement.authority_epoch,
                action: ConversationControlDeviceAction::View,
            },
        )
        .await;
        let response = match response {
            Ok(response) => response,
            Err(error) if error.is_session_version_incompatible() => {
                return Err(ApiError::UpgradeRequired(error.to_string()));
            }
            Err(error) => {
                return Err(chat_command_delivery_error(
                    error,
                    "remote Conversation control view",
                ));
            }
        };
        return match response {
            ConversationControlDeviceResponse::View { control, .. } => Ok(control),
            _ => Err(ApiError::Internal(
                "unexpected remote Conversation Control response".to_string(),
            )),
        };
    }
    Ok(state
        .conversation_authority
        .control_view(&rua_core::ChatAddress::agent(agent_id, chat_id), &meta)
        .await)
}

async fn remote_conversation_control_if_needed(
    state: &AppState,
    agent_id: &str,
    ws_dir: &Path,
    chat_id: &str,
    action: ConversationControlDeviceAction,
    expected_authority_epoch: Option<ConversationAuthorityEpoch>,
) -> Result<Option<ConversationControlDeviceResponse>, ApiError> {
    let placement =
        crate::services::conversation_authority_target::load_optional_chat_authority_placement(
            ws_dir, chat_id,
        )
        .await
        .map_err(ApiError::Internal)?
        .ok_or_else(|| {
            ApiError::Conflict(format!(
                "conversation authority placement not found for Chat '{chat_id}'"
            ))
        })?;
    if expected_authority_epoch.is_some_and(|expected| expected != placement.authority_epoch) {
        return Err(ApiError::Conflict(
            "stale conversation authority Epoch".to_string(),
        ));
    }
    let local_device_id = device_id_from_sync_device(&state.sync_device)
        .ok_or_else(|| ApiError::Unavailable("local Device identity is unavailable".to_string()))?;
    if placement.device_id == local_device_id {
        return Ok(None);
    }
    dispatch_conversation_control(
        &state.conversation_input_intake,
        ConversationControlDeviceCommand {
            command_id: format!("conversation-control-{}", uuid::Uuid::new_v4()),
            agent_id: agent_id.to_string(),
            address: rua_core::ChatAddress::agent(agent_id, chat_id),
            source_device_id: local_device_id,
            target_device_id: placement.device_id,
            authority_epoch: placement.authority_epoch,
            action,
        },
    )
    .await
    .map(Some)
    .map_err(|error| chat_command_delivery_error(error, "remote Conversation control"))
}

pub(crate) async fn populate_collaboration_source_availability(
    rua_root: &Path,
    chats: &mut [ChatSpecDto],
) -> Result<(), String> {
    for chat in chats {
        let Some(collaboration) = chat.collaboration.as_mut() else {
            continue;
        };
        let scope = rua_core::ChatScopeStore::resolve(rua_root, collaboration.source.clone());
        collaboration.source_available = Some(
            WorkspaceSessionStore::new(scope.root())
                .load_chat_header(collaboration.source.chat_id().as_str())
                .await
                .map_err(|error| error.to_string())?
                .is_some(),
        );
    }
    Ok(())
}

async fn list_chats_core(
    state: &AppState,
    agent_id: &str,
    ws_dir: &Path,
    params: ListChatsParams,
) -> Response {
    tracing::info!(
        target: "diag.session",
        offset = ?params.offset,
        limit = ?params.limit,
        user_id = ?params.user_id,
        channel = ?params.channel,
        ws_dir = %ws_dir.display(),
        "[DIAG-session] list_chats_core ENTER",
    );

    let offset = params.offset.unwrap_or(0);
    let limit = params.limit.unwrap_or(50).clamp(1, 200);
    let address = rua_core::ChatAddress::agent(agent_id, "00000000-0000-4000-8000-000000000000");
    let page = match rua_chat::ChatService::for_resolved_scope(ws_dir, address.clone())
        .list(
            &address,
            rua_chat::ChatListQuery {
                user_id: params.user_id,
                channel: params.channel,
                search: params.search,
                tag: params.tag,
                conversation_view: params.view,
                source_chat_id: params.source_chat_id,
                executor: params.executor,
                status: params.status,
                sort_by: params.sort_by,
                sort_order: params.sort_order,
                offset,
                limit,
                archived: params.archived,
            },
        )
        .await
    {
        Ok(page) => page,
        Err(e) => {
            return ApiError::Internal(format!("Failed to list chat index: {e}")).into_response();
        }
    };

    let mut result = page
        .entries
        .into_iter()
        .map(chat_from_index_entry)
        .collect::<Vec<_>>();
    if let Err(error) =
        populate_collaboration_source_availability(&state.rua_root, &mut result).await
    {
        return ApiError::Internal(error).into_response();
    }
    let mut headers = HeaderMap::new();
    headers.insert(
        "X-Total-Count",
        HeaderValue::from_str(&page.total.to_string())
            .unwrap_or_else(|_| HeaderValue::from_static("0")),
    );

    (StatusCode::OK, headers, Json(result)).into_response()
}

async fn create_chat_core(body: CreateChatBody, state: &AppState) -> Response {
    let now = Utc::now();
    let chat_id = uuid::Uuid::new_v4().to_string();
    let mut meta = body.meta.unwrap_or(serde_json::json!({}));
    let mut model = body.model;
    if let Some(ref mut model) = model {
        normalize_chat_model_thinking(state, model).await;
    } else {
        model = default_chat_model_slot(state).await;
    }
    if contains_conversation_authority_placement(&meta) {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "detail": "authority_placement cannot be set by source-side chat creation"
            })),
        )
            .into_response();
    }
    if body
        .authority_device_id
        .as_deref()
        .is_some_and(|device_id| !device_id.trim().is_empty())
    {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "detail": "new chats always start on the current Device; use authority-placement to switch afterward"
            })),
        )
            .into_response();
    }
    if !meta.is_object() {
        return ApiError::BadRequest("meta must be an object".into()).into_response();
    }
    let device_activity = ChatDeviceActivityUpdate {
        device_id: device_id_from_sync_device(&state.sync_device),
        host_profile: state.profile.to_string(),
    };
    apply_chat_created(&mut meta, &device_activity);
    let chat = ChatSpecDto {
        id: chat_id.clone(),
        name: body.name.unwrap_or_else(|| "New Chat".to_string()),
        user_id: body.user_id.unwrap_or_else(|| "default".to_string()),
        channel: body.channel.unwrap_or_else(|| "default".to_string()),
        created_at: now,
        updated_at: now,
        meta,
        status: "idle".to_string(),
        model,
        collaboration_mode: body.collaboration_mode.unwrap_or_default(),
        pinned: false,
        pin_order: 0,
        tag: rua_chat::ChatTag::None,
        archived: false,
        collaboration: None,
    };

    (StatusCode::OK, Json(chat)).into_response()
}

pub(crate) fn chat_session_header(chat: &ChatSpecDto, agent_id: &str) -> ChatSessionHeader {
    let mut header = ChatSessionHeader::new(ChatMetadata {
        chat_id: chat.id.clone(),
        agent_id: agent_id.to_string(),
        title: chat.name.clone(),
        created_at: chat.created_at.to_rfc3339(),
        last_activity_at: chat.updated_at.to_rfc3339(),
        pinned: chat.pinned,
        archived: chat.archived,
        preview: None,
    });
    header.tag = chat.tag;
    copy_chat_activity_to_header_extra(&chat.meta, &mut header.extra);
    for key in [
        CHAT_AUTHORITY_PLACEMENT_META_KEY,
        CHAT_ACP_BINDING_META_KEY,
        crate::services::product_hooks::CHAT_HOOK_CONFIGURATION_META_KEY,
        "harness_configuration",
    ] {
        if let Some(value) = chat.meta.get(key) {
            header.extra.insert(key.to_string(), value.clone());
        }
    }
    sync_chat_model_to_header_extra(&chat.model, &mut header.extra);
    header.extra.insert(
        rua_core::CHAT_COLLABORATION_MODE_META_KEY.to_string(),
        serde_json::json!(chat.collaboration_mode),
    );
    rua_chat::apply_agent_scope_defaults(&mut header, agent_id);
    header
}

fn new_chat_authority_placement(
    address: &rua_core::ChatAddress,
    device_id: String,
) -> Result<ConversationAuthorityPlacement, ApiError> {
    let base_manifest_hash =
        crate::services::conversation_manifest::initial_conversation_manifest_hash(address)
            .map_err(ApiError::from)?;
    let placement = ConversationAuthorityPlacement {
        authority_epoch: ConversationAuthorityEpoch::next_for_device(&device_id, None),
        device_id,
        base_manifest_hash,
    };
    placement
        .validate()
        .map_err(|error| ApiError::BadRequest(error.to_string()))?;
    Ok(placement)
}

pub(crate) async fn persist_acp_history_preview_core(
    address: &rua_core::ChatAddress,
    agent_id: &str,
    ws_dir: &Path,
    chat_id: &str,
    body: AcpHistoryPreviewBody,
    state: &AppState,
) -> Response {
    let text = body.text.trim();
    if text.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"detail": "text is required"})),
        )
            .into_response();
    }

    let preview_id = body.preview_id;
    let turn_id = format!("acp-history-preview-{preview_id}");
    let message_id = format!("acp-history-preview-message-{preview_id}");
    let store = WorkspaceSessionStore::new(ws_dir);

    match store.read_turn_range(chat_id, 1, usize::MAX).await {
        Ok(turns) if turns.iter().any(|turn| turn.turn_id == turn_id) => {
            return (
                StatusCode::OK,
                Json(AcpHistoryPreviewResponse {
                    turn_id,
                    message_id,
                    created: false,
                }),
            )
                .into_response();
        }
        Ok(_) => {}
        Err(e) => return ApiError::from(e).into_response(),
    }

    let now = Utc::now();
    let now_rfc3339 = now.to_rfc3339();
    let mut chat = match chat_index::get(ws_dir, chat_id).await {
        Ok(Some(entry)) => chat_from_index_entry(entry),
        Ok(None) => ChatSpecDto {
            id: chat_id.to_string(),
            name: "New Chat".to_string(),
            user_id: "default".to_string(),
            channel: "console".to_string(),
            created_at: now,
            updated_at: now,
            meta: serde_json::json!({}),
            status: "idle".to_string(),
            model: None,
            collaboration_mode: rua_core::AgentCollaborationMode::default(),
            pinned: false,
            pin_order: 0,
            tag: rua_chat::ChatTag::None,
            archived: false,
            collaboration: None,
        },
        Err(e) => {
            return ApiError::Internal(format!("Failed to get chat index: {e}")).into_response();
        }
    };
    chat.updated_at = now;

    match store.load_chat_header(chat_id).await {
        Ok(Some(_)) => {}
        Ok(None) => {
            if let Err(e) = store
                .save_chat_header(&chat_session_header(&chat, agent_id))
                .await
            {
                return ApiError::from(e).into_response();
            }
        }
        Err(e) => return ApiError::from(e).into_response(),
    }

    let runner = body
        .runner
        .as_deref()
        .map(str::trim)
        .filter(|runner| !runner.is_empty())
        .map(str::to_string);
    let mut origin = serde_json::Map::new();
    origin.insert("kind".to_string(), serde_json::json!("acp"));
    if let Some(runner) = runner.as_ref() {
        origin.insert("runner".to_string(), serde_json::json!(runner));
    }

    let metadata = serde_json::Map::from_iter([
        (
            "message_type".to_string(),
            serde_json::json!(ACP_HISTORY_PREVIEW_MESSAGE_TYPE),
        ),
        (
            "acp_history_preview".to_string(),
            serde_json::Value::Bool(true),
        ),
        ("ui_only".to_string(), serde_json::Value::Bool(true)),
        ("model_visible".to_string(), serde_json::Value::Bool(false)),
        ("preview_id".to_string(), serde_json::json!(preview_id)),
        ("origin".to_string(), serde_json::Value::Object(origin)),
    ]);

    let append_result = store
        .append_conversation_turn(
            chat_id,
            NewConversationTurn {
                turn_id: turn_id.clone(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: now_rfc3339.clone(),
                    assistant_started_at: Some(now_rfc3339.clone()),
                    assistant_completed_at: Some(now_rfc3339.clone()),
                    status: "completed".to_string(),
                },
                selected_variant_id: Some(message_id.clone()),
                messages: vec![NewConversationMessage {
                    message_id: message_id.clone(),
                    role: "ui".to_string(),
                    created_at: now_rfc3339,
                    status: "completed".to_string(),
                    content: serde_json::json!([{ "type": "text", "text": text }]),
                    provider: None,
                    tool: None,
                    metadata,
                }],
            },
        )
        .await;
    if let Err(e) = append_result {
        return ApiError::from(e).into_response();
    }

    if let Err(e) = upsert_chat_index(ws_dir, &chat).await {
        return e.into_response();
    }

    let chat_val = serde_json::to_value(&chat).unwrap_or_default();
    let _ = publish_sync_fact(
        state,
        SyncPublicationFact::ChatIndexUpserted {
            address: address.clone(),
            chat: chat_val,
        },
    )
    .await;
    let _ = publish_sync_fact(
        state,
        SyncPublicationFact::StableConversationState {
            scope_root: ws_dir.to_path_buf(),
            address: address.clone(),
        },
    )
    .await;

    (
        StatusCode::OK,
        Json(AcpHistoryPreviewResponse {
            turn_id,
            message_id,
            created: true,
        }),
    )
        .into_response()
}

pub(crate) async fn get_chat_metadata_core(ws_dir: &Path, chat_id: &str) -> Response {
    if let Err(error) = crate::services::console_chat::resolve_chat_id(chat_id) {
        return ApiError::BadRequest(error).into_response();
    }
    if !ws_dir.join("chat-index.sqlite").is_file() {
        return ApiError::NotFound(format!("Chat '{chat_id}' not found")).into_response();
    }
    match get_chat_index_entry(ws_dir, chat_id).await {
        Ok(Some(entry)) => Json(chat_from_index_entry(entry)).into_response(),
        Ok(None) => ApiError::NotFound(format!("Chat '{chat_id}' not found")).into_response(),
        Err(error) => {
            ApiError::Internal(format!("Failed to get chat index: {error}")).into_response()
        }
    }
}

pub(crate) async fn get_chat_core(
    state: &AppState,
    address: &rua_core::ChatAddress,
    ws_dir: &Path,
    chat_id: &str,
    params: ChatHistoryParams,
) -> Response {
    let indexed_chat = match get_chat_index_entry(ws_dir, chat_id).await {
        Ok(Some(entry)) => Some(chat_from_index_entry(entry)),
        Ok(None) => None,
        Err(e) => {
            return ApiError::Internal(format!("Failed to get chat index: {e}")).into_response();
        }
    };
    let chat_service = rua_chat::ChatService::for_resolved_scope(ws_dir, address.clone());
    let current_chat = match chat_service.load(address).await {
        Ok(chat) => chat,
        Err(error) => return ApiError::from(error).into_response(),
    };
    let (messages, model, title, oldest_turn_ordinal, newest_turn_ordinal, has_more_before) =
        if let Some(current_chat) = current_chat.as_ref() {
            match chat_service
                .read_history(
                    address,
                    rua_chat::ChatHistoryQuery {
                        before_turn_ordinal: params.before_turn_ordinal,
                        turn_limit: params.turn_limit,
                        around_turn_id: params.around_turn_id.clone(),
                    },
                )
                .await
            {
                Ok(Some(page)) => {
                    let header_model = page
                        .chat
                        .model()
                        .cloned()
                        .and_then(|model| serde_json::from_value(model).ok());
                    let index_name = indexed_chat
                        .as_ref()
                        .map(|chat| chat.name.as_str())
                        .unwrap_or("");
                    let title = chat_history_title(page.chat.title(), index_name, chat_id);
                    let mut messages =
                        extract_messages_from_message_log(&page.messages, &page.turns);
                    if page.includes_latest_turn {
                        if let Some(pending) = pending_action_message_from_header(&page.header) {
                            messages.push(pending);
                        }
                    }
                    (
                        messages,
                        header_model,
                        title,
                        page.oldest_turn_ordinal,
                        page.newest_turn_ordinal,
                        page.has_more_before,
                    )
                }
                Ok(None) => (
                    Vec::new(),
                    None,
                    non_empty_string(current_chat.title().to_string()),
                    None,
                    None,
                    false,
                ),
                Err(error) => return ApiError::from(error).into_response(),
            }
        } else {
            (
                vec![],
                default_chat_model_slot(state).await,
                None,
                None,
                None,
                false,
            )
        };

    let mut meta = match indexed_chat.as_ref() {
        Some(chat) => chat.meta.clone(),
        None if address.agent_id().is_some() => {
            let Some(local_device_id) = device_id_from_sync_device(&state.sync_device) else {
                return ApiError::Internal("local Device identity is unavailable".to_string())
                    .into_response();
            };
            let placement = match new_chat_authority_placement(address, local_device_id) {
                Ok(placement) => placement,
                Err(error) => return error.into_response(),
            };
            serde_json::json!({
                CHAT_AUTHORITY_PLACEMENT_META_KEY: placement,
            })
        }
        None => serde_json::json!({}),
    };
    if let Some(current_chat) = current_chat.as_ref() {
        if let Some(extra) = meta.as_object_mut() {
            sync_chat_model_to_header_extra(&model, extra);
        }
        meta["chat_policy"] =
            serde_json::to_value(current_chat.policy()).expect("Chat Policy is serializable");
        if let Some(selected_agent_id) = current_chat.selected_agent_id() {
            meta["selected_agent_id"] = serde_json::json!(selected_agent_id);
        }
        if let Some(authority_placement) = current_chat.authority_placement() {
            meta[CHAT_AUTHORITY_PLACEMENT_META_KEY] = authority_placement.clone();
        }
        if let Some(thinking) = current_chat.thinking() {
            meta["thinking"] = thinking.clone();
        }
        if let Some(harness_configuration) = current_chat.harness_configuration() {
            meta["harness_configuration"] = harness_configuration.clone();
        }
    }
    if let Some(header) = match WorkspaceSessionStore::new(ws_dir)
        .load_chat_header(chat_id)
        .await
    {
        Ok(header) => header,
        Err(error) => return ApiError::from(error).into_response(),
    } {
        if let Some(configuration) = header
            .extra
            .get(crate::services::product_hooks::CHAT_HOOK_CONFIGURATION_META_KEY)
        {
            meta[crate::services::product_hooks::CHAT_HOOK_CONFIGURATION_META_KEY] =
                configuration.clone();
        }
        if let Some(mut collaboration) =
            collaboration_from_meta(&serde_json::Value::Object(header.extra.clone()))
        {
            let source_scope =
                rua_core::ChatScopeStore::resolve(&state.rua_root, collaboration.source.clone());
            collaboration.source_available = Some(
                match WorkspaceSessionStore::new(source_scope.root())
                    .load_chat_header(collaboration.source.chat_id().as_str())
                    .await
                {
                    Ok(header) => header.is_some(),
                    Err(error) => return ApiError::from(error).into_response(),
                },
            );
            meta["collaboration"] =
                serde_json::to_value(collaboration).expect("collaboration relation serializes");
        }
    }
    if let Some(project_id) = address.project_id() {
        if let Ok(agent_state) = state
            .conversation_input_intake
            .project_chat_agent_state(project_id, chat_id)
            .await
        {
            if let Ok(agent_state_value) = serde_json::to_value(&agent_state) {
                meta["project_chat_selected_agent_available"] =
                    serde_json::Value::Bool(agent_state.selected_agent_available);
                meta["project_chat_agent_state"] = agent_state_value;
            }
        }
    }
    let durable_coverage =
        match crate::services::conversation_manifest::load_conversation_manifest(ws_dir, address)
            .await
        {
            Ok(Some(manifest)) => match manifest.hash() {
                Ok(manifest_hash) => Some(DurableConversationCoverageDto {
                    latest_message_sequence: manifest.latest_sequence,
                    manifest_hash,
                }),
                Err(error) => return ApiError::from(error).into_response(),
            },
            Ok(None) => None,
            Err(error) => return ApiError::from(error).into_response(),
        };

    let history = ChatHistoryDto {
        messages,
        model,
        meta,
        title,
        oldest_turn_ordinal,
        newest_turn_ordinal,
        has_more_before,
        durable_coverage,
    };

    (StatusCode::OK, Json(history)).into_response()
}

fn non_empty_string(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn chat_history_title(header_title: &str, index_name: &str, chat_id: &str) -> Option<String> {
    let header = header_title.trim();
    let index = index_name.trim();
    if !index.is_empty() && (header.is_empty() || header == "New Chat" || header == chat_id) {
        return Some(index.to_string());
    }
    non_empty_string(header.to_string()).or_else(|| non_empty_string(index.to_string()))
}

async fn get_chat_index_entry(
    ws_dir: &Path,
    chat_id: &str,
) -> Result<Option<chat_index::ChatIndexEntry>, String> {
    chat_index::get(ws_dir, chat_id).await
}

fn pending_action_message_from_header(header: &ChatSessionHeader) -> Option<serde_json::Value> {
    let pending = header.context_snapshot.pending_action.as_ref()?;
    if pending.get("status").and_then(|value| value.as_str()) != Some("waiting") {
        return None;
    }
    let request_id = pending
        .get("request_id")
        .and_then(|value| value.as_str())
        .or_else(|| pending.get("call_id").and_then(|value| value.as_str()))?;
    let action_type = pending.get("action_type").and_then(|value| value.as_str());
    if action_type == Some("ask_user") {
        return Some(serde_json::json!({
            "id": request_id,
            "role": "assistant",
            "type": "ask_user_request",
            "status": "in_progress",
            "content": [{
                "type": "ask_user_request",
                "data": {
                    "id": request_id,
                    "request_id": request_id,
                    "prompt": pending.get("prompt").cloned().unwrap_or(serde_json::Value::Null),
                    "questions": pending.get("questions").cloned().unwrap_or_else(|| serde_json::json!([])),
                    "action_type": "ask_user",
                    "status": "waiting"
                }
            }]
        }));
    }
    None
}

fn extract_messages_from_message_log(
    messages: &[MessageLogEntry],
    turns: &[TurnIndexRecord],
) -> Vec<serde_json::Value> {
    if turns.is_empty() {
        return Vec::new();
    }

    let messages_by_sequence: HashMap<u64, &MessageLogEntry> = messages
        .iter()
        .map(|message| (message.sequence, message))
        .collect();
    let mut sorted_turns: Vec<&TurnIndexRecord> = turns.iter().collect();
    sorted_turns.sort_by_key(|turn| (turn.ordinal, turn.first_sequence));

    let mut results = Vec::new();
    for turn in sorted_turns {
        let lifecycle = &turn.lifecycle;
        if let Some(user) = messages_by_sequence
            .get(&turn.user_message_sequence)
            .filter(|message| message.role == "user")
        {
            let mut user_outputs = convert_message_log_entry(user);
            for output in &mut user_outputs {
                if let Some(object) = output.as_object_mut() {
                    object.insert("turn_id".to_string(), serde_json::json!(turn.turn_id));

                    object.insert(
                        "turn_status".to_string(),
                        serde_json::json!(lifecycle.status.clone()),
                    );

                    object.insert(
                        "created_at".to_string(),
                        serde_json::json!(lifecycle.user_submitted_at.clone()),
                    );
                }
            }
            results.extend(user_outputs);
        }

        let variants = turn
            .effective_assistant_response_variants()
            .into_iter()
            .filter_map(|variant| {
                let output = messages_for_variant(
                    &messages_by_sequence,
                    &variant,
                    (turn.selected_variant_id.as_deref() == Some(variant.variant_id.as_str()))
                        .then_some(lifecycle.assistant_completed_at.as_deref())
                        .flatten(),
                );
                if output.is_empty() {
                    return None;
                }
                Some(serde_json::json!({
                    "variant_id": variant.variant_id,
                    "output": output,
                }))
            })
            .collect::<Vec<_>>();
        if variants.is_empty() {
            results.extend(runtime_notes_for_turn(&messages_by_sequence, turn));
            continue;
        }

        let selected_variant_id = turn
            .selected_variant_id
            .as_deref()
            .filter(|selected| {
                variants
                    .iter()
                    .any(|variant| variant["variant_id"].as_str() == Some(*selected))
            })
            .or_else(|| {
                variants
                    .last()
                    .and_then(|variant| variant["variant_id"].as_str())
            })
            .unwrap_or_default();
        let selected_variant_record = turn
            .effective_assistant_response_variants()
            .into_iter()
            .find(|variant| variant.variant_id == selected_variant_id);
        let Some(selected_variant_record) = selected_variant_record else {
            continue;
        };
        let mut selected_outputs =
            messages_for_selected_variant(&messages_by_sequence, turn, &selected_variant_record);
        for (index, output) in selected_outputs.iter_mut().enumerate() {
            if let Some(object) = output.as_object_mut() {
                object.insert("turn_id".to_string(), serde_json::json!(turn.turn_id));

                object.insert(
                    "turn_status".to_string(),
                    serde_json::json!(lifecycle.status.clone()),
                );
                object.insert(
                    "variant_id".to_string(),
                    serde_json::json!(selected_variant_id),
                );
                object.insert(
                    "selected_variant_id".to_string(),
                    serde_json::json!(selected_variant_id),
                );

                if let Some(started) = &lifecycle.assistant_started_at {
                    object.insert("started_at".to_string(), serde_json::json!(started.clone()));
                }
                if let Some(completed) = &lifecycle.assistant_completed_at {
                    object.insert(
                        "finished_at".to_string(),
                        serde_json::json!(completed.clone()),
                    );
                }
                if index == 0 && variants.len() > 1 {
                    object.insert(
                        "assistant_response_variants".to_string(),
                        serde_json::Value::Array(variants.clone()),
                    );
                }
            }
        }
        results.extend(selected_outputs);
    }

    results
}

fn messages_for_variant(
    messages_by_sequence: &HashMap<u64, &MessageLogEntry>,
    variant: &rua_session::AssistantResponseVariantRecord,
    finished_at_override: Option<&str>,
) -> Vec<serde_json::Value> {
    let messages = (variant.first_sequence..=variant.last_sequence)
        .filter_map(|sequence| messages_by_sequence.get(&sequence))
        .filter(|message| message.role != "user")
        .filter(|message| rua_session::RuntimeNote::from_message_log_entry(message).is_none())
        .copied()
        .collect::<Vec<_>>();
    let started_at = messages
        .first()
        .map(|message| message.created_at.clone())
        .filter(|value| !value.is_empty())
        .or_else(|| (!variant.created_at.is_empty()).then(|| variant.created_at.clone()));
    let finished_at = finished_at_override
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| {
            messages
                .last()
                .map(|message| message.created_at.clone())
                .filter(|value| !value.is_empty())
        });
    let mut output = messages
        .into_iter()
        .flat_map(convert_message_log_entry)
        .collect::<Vec<_>>();
    if let Some(object) = output
        .first_mut()
        .and_then(serde_json::Value::as_object_mut)
    {
        if let Some(started_at) = started_at {
            object.insert("started_at".to_string(), serde_json::json!(started_at));
        }
        if let Some(finished_at) = finished_at {
            object.insert("finished_at".to_string(), serde_json::json!(finished_at));
        }
    }
    output
}

fn messages_for_selected_variant(
    messages_by_sequence: &HashMap<u64, &MessageLogEntry>,
    turn: &TurnIndexRecord,
    variant: &rua_session::AssistantResponseVariantRecord,
) -> Vec<serde_json::Value> {
    let mut selected = (variant.first_sequence..=variant.last_sequence)
        .filter_map(|sequence| messages_by_sequence.get(&sequence).copied())
        .filter(|message| message.role != "user")
        .filter(|message| rua_session::RuntimeNote::from_message_log_entry(message).is_none())
        .collect::<Vec<_>>();
    selected.extend(
        (turn.first_sequence..=turn.last_sequence)
            .filter_map(|sequence| messages_by_sequence.get(&sequence).copied())
            .filter(|message| rua_session::RuntimeNote::from_message_log_entry(message).is_some()),
    );
    selected.sort_by_key(|message| message.sequence);
    selected
        .into_iter()
        .flat_map(convert_message_log_entry)
        .collect()
}

fn runtime_notes_for_turn(
    messages_by_sequence: &HashMap<u64, &MessageLogEntry>,
    turn: &TurnIndexRecord,
) -> Vec<serde_json::Value> {
    (turn.first_sequence..=turn.last_sequence)
        .filter_map(|sequence| messages_by_sequence.get(&sequence).copied())
        .filter(|message| rua_session::RuntimeNote::from_message_log_entry(message).is_some())
        .flat_map(convert_message_log_entry)
        .collect()
}

fn convert_message_log_entry(message: &MessageLogEntry) -> Vec<serde_json::Value> {
    if let Some(note) = rua_session::RuntimeNote::from_message_log_entry(message) {
        return vec![serde_json::json!({
            "id": message.message_id,
            "role": "system",
            "type": "runtime_note",
            "status": message.status,
            "content": [],
            "runtime_note": note,
            "created_at": message.created_at,
            "turn_id": message.turn_id,
        })];
    }
    if is_acp_history_preview_message(message) {
        return vec![serde_json::json!({
            "id": message.message_id,
            "role": "ui",
            "type": ACP_HISTORY_PREVIEW_MESSAGE_TYPE,
            "status": message.status,
            "content": message_log_content_for_ui(message),
            "metadata": message.metadata,
            "created_at": message.created_at,
            "turn_id": message.turn_id,
        })];
    }

    let name = message
        .metadata
        .get("name")
        .and_then(|value| value.as_str())
        .unwrap_or(&message.role);
    let mut results = Vec::new();
    convert_agentscope_msg(
        &serde_json::json!({
            "id": message.message_id,
            "role": message.role,
            "name": name,
            "content": message.content,
            "metadata": message.metadata,

            "created_at": message.created_at,
        }),
        &mut results,
    );
    results
}

fn is_acp_history_preview_message(message: &MessageLogEntry) -> bool {
    message
        .metadata
        .get("message_type")
        .and_then(|value| value.as_str())
        == Some(ACP_HISTORY_PREVIEW_MESSAGE_TYPE)
        || message
            .metadata
            .get("acp_history_preview")
            .and_then(|value| value.as_bool())
            .unwrap_or(false)
}

fn message_log_content_for_ui(message: &MessageLogEntry) -> serde_json::Value {
    match &message.content {
        serde_json::Value::Array(parts) => serde_json::Value::Array(parts.clone()),
        serde_json::Value::String(text) => serde_json::json!([{ "type": "text", "text": text }]),
        serde_json::Value::Object(map) => {
            if let Some(text) = map.get("text").and_then(|value| value.as_str()) {
                serde_json::json!([{ "type": "text", "text": text }])
            } else {
                serde_json::json!([{ "type": "text", "text": message.content.to_string() }])
            }
        }
        other => serde_json::json!([{ "type": "text", "text": other.to_string() }]),
    }
}

fn msg_type_for_block(btype: &str) -> &'static str {
    match btype {
        "thinking" => "reasoning",
        "tool_use" => "plugin_call",
        "tool_result" => "plugin_call_output",
        _ => "message",
    }
}

fn canonical_history_message_id(
    original_id: &str,
    raw_metadata: Option<&serde_json::Value>,
    block: &serde_json::Value,
    block_type: &str,
    group_count: usize,
) -> Option<String> {
    if group_count == 1 && !original_id.is_empty() {
        return Some(original_id.to_string());
    }
    let metadata = raw_metadata.and_then(serde_json::Value::as_object);
    let metadata_id = |key: &str| {
        metadata
            .and_then(|metadata| metadata.get(key))
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    match block_type {
        "thinking" => metadata_id(REASONING_MESSAGE_ID_METADATA_KEY),
        "tool_use" => block
            .get("id")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.is_empty())
            .map(tool_use_message_id),
        "tool_result" => block
            .get("id")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.is_empty())
            .map(tool_result_message_id),
        _ => metadata_id(TEXT_MESSAGE_ID_METADATA_KEY)
            .or_else(|| metadata_id(CANONICAL_MESSAGE_ID_METADATA_KEY)),
    }
}

fn convert_agentscope_msg(msg: &serde_json::Value, results: &mut Vec<serde_json::Value>) {
    let role = msg
        .get("role")
        .and_then(|v| v.as_str())
        .unwrap_or("assistant");
    let original_id = msg.get("id").and_then(|v| v.as_str()).unwrap_or("");
    let original_name = msg.get("name").and_then(|v| v.as_str()).unwrap_or("");
    let raw_metadata = msg.get("metadata").cloned();
    let mut metadata = serde_json::json!({
        "original_id": original_id,
        "original_name": original_name,
        "metadata": raw_metadata.clone(),
    });

    if let Some(inner) = raw_metadata.as_ref().and_then(|v| v.as_object()) {
        if let Some(origin) = inner.get("origin").cloned() {
            metadata["origin"] = origin;
        }
    }
    if let Some(created_at) = msg.get("created_at") {
        metadata["created_at"] = created_at.clone();
    }

    let content = msg.get("content");

    let created_at = msg.get("created_at").cloned();
    if let Some(text) = content.and_then(|c| c.as_str()) {
        results.push(serde_json::json!({
            "type": "message",
            "role": role,
            "id": original_id,
            "status": "completed",
            "content": [{"type": "text", "text": text, "delta": false, "index": null}],
            "metadata": metadata,
            "created_at": created_at,
        }));
        return;
    }

    let Some(blocks) = content.and_then(|c| c.as_array()) else {
        return;
    };

    let group_count = blocks
        .iter()
        .enumerate()
        .filter(|(index, block)| {
            if *index == 0 {
                return true;
            }
            let block_type = block
                .get("type")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("text");
            let previous_type = blocks[*index - 1]
                .get("type")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("text");
            matches!(block_type, "tool_use" | "tool_result")
                || msg_type_for_block(block_type) != msg_type_for_block(previous_type)
        })
        .count();
    let mut current_type: Option<&str> = None;
    let mut current_message_id: Option<String> = None;
    let mut current_content: Vec<serde_json::Value> = Vec::new();

    for block in blocks {
        let btype = block.get("type").and_then(|v| v.as_str()).unwrap_or("text");
        let out_type = msg_type_for_block(btype);

        let need_new = match btype {
            "tool_use" | "tool_result" => true,
            "thinking" => current_type != Some("reasoning"),
            _ => current_type != Some("message"),
        };

        if need_new {
            if let Some(ct) = current_type {
                if let Some(id) = current_message_id.take() {
                    results.push(serde_json::json!({
                        "type": ct,
                        "role": role,
                        "id": id,
                        "status": "completed",
                        "content": serde_json::Value::Array(std::mem::take(&mut current_content)),
                        "metadata": metadata,
                        "created_at": created_at.clone(),
                    }));
                } else {
                    current_content.clear();
                }
            }
            current_type = Some(out_type);
            current_message_id = canonical_history_message_id(
                original_id,
                raw_metadata.as_ref(),
                block,
                btype,
                group_count,
            );
        }

        let output_block = match btype {
            "text" => {
                serde_json::json!({
                    "type": "text",
                    "text": block.get("text").and_then(|v| v.as_str()).unwrap_or(""),
                    "delta": false,
                    "index": null,
                })
            }
            "thinking" => {
                serde_json::json!({
                    "type": "text",
                    "text": block.get("thinking").and_then(|v| v.as_str()).unwrap_or(""),
                    "delta": false,
                    "index": null,
                })
            }
            "tool_use" => {
                let arguments = match block.get("input") {
                    Some(input) if input.is_object() || input.is_array() => {
                        serde_json::to_string(input).unwrap_or_default()
                    }
                    Some(input) => input.as_str().unwrap_or("").to_string(),
                    None => String::new(),
                };
                serde_json::json!({
                    "type": "data",
                    "delta": false,
                    "index": null,
                    "data": {
                        "call_id": block.get("id").and_then(|v| v.as_str()).unwrap_or(""),
                        "name": block.get("name").and_then(|v| v.as_str()).unwrap_or(""),
                        "arguments": arguments,
                    }
                })
            }
            "tool_result" => {
                let output = block
                    .get("output")
                    .map(render_tool_result_output)
                    .unwrap_or_default();
                serde_json::json!({
                    "type": "data",
                    "delta": false,
                    "index": null,
                    "data": {
                        "call_id": block.get("id").and_then(|v| v.as_str()).unwrap_or(""),
                        "name": block.get("name").and_then(|v| v.as_str()).unwrap_or(""),
                        "output": output,
                    }
                })
            }
            _ => block.clone(),
        };

        current_content.push(output_block);
    }

    if let Some(ct) = current_type {
        if let Some(id) = current_message_id {
            results.push(serde_json::json!({
                "type": ct,
                "role": role,
                "id": id,
                "status": "completed",
                "content": serde_json::Value::Array(std::mem::take(&mut current_content)),
                "metadata": metadata,
                "created_at": created_at.clone(),
            }));
        }
    }
}

fn render_tool_result_output(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Array(parts) => {
            let mut text_parts = Vec::with_capacity(parts.len());
            for part in parts {
                let Some(text) = text_from_tool_output_part(part) else {
                    return serde_json::to_string(value).unwrap_or_default();
                };
                text_parts.push(text);
            }
            text_parts.join("\n")
        }
        serde_json::Value::Object(_) => text_from_tool_output_part(value)
            .map(str::to_string)
            .unwrap_or_else(|| serde_json::to_string(value).unwrap_or_default()),
        other => other.to_string(),
    }
}

fn text_from_tool_output_part(value: &serde_json::Value) -> Option<&str> {
    let map = value.as_object()?;
    match map.get("type").and_then(|v| v.as_str()) {
        Some("text") | None => map.get("text").and_then(|v| v.as_str()),
        _ => None,
    }
}

async fn update_chat_core(
    agent_id: &str,
    ws_dir: &Path,
    chat_id: &str,
    body: ChatUpdateBody,
    state: &AppState,
) -> Response {
    if let Some(field) = body.read_only_field() {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "detail": format!("field '{field}' is read-only")
            })),
        )
            .into_response();
    }
    if body.update_configuration {
        return ApiError::BadRequest("Use the Chat configuration preview/apply endpoint with expected_epoch and candidate_epoch".to_string()).into_response();
    }
    let model_update_requested = body.model.is_some();
    let non_model_update_requested = body.name.is_some()
        || body.pinned.is_some()
        || body.pin_order.is_some()
        || body.collaboration_mode.is_some()
        || body.turn_id.is_some()
        || body.selected_variant_id.is_some();
    let mut model_persisted_by_authority = false;

    let mut chat = match chat_index::get(ws_dir, chat_id).await {
        Ok(Some(entry)) => chat_from_index_entry(entry),
        Ok(None) => {
            return ApiError::NotFound(format!("Chat not found: {chat_id}")).into_response()
        }
        Err(error) => return ApiError::Internal(error).into_response(),
    };
    if chat.model.is_none() {
        chat.model = load_chat_model_from_header(ws_dir, chat_id).await;
    }
    if let Some(ref name) = body.name {
        chat.name = name.clone();
    }
    if let Some(pinned) = body.pinned {
        chat.pinned = pinned;
    }
    if let Some(pin_order) = body.pin_order {
        chat.pin_order = pin_order;
    }
    if let Some(collaboration_mode) = body.collaboration_mode {
        chat.collaboration_mode = collaboration_mode;
    }
    if let Some(model) = body.model {
        let model_value = serde_json::to_value(&model).unwrap_or_default();
        match remote_conversation_control_if_needed(
            state,
            agent_id,
            ws_dir,
            chat_id,
            ConversationControlDeviceAction::UpdateModel {
                model: model_value.clone(),
            },
            None,
        )
        .await
        {
            Ok(Some(ConversationControlDeviceResponse::ModelUpdated {
                model: confirmed, ..
            })) => {
                model_persisted_by_authority = true;
                chat.model = match serde_json::from_value(confirmed) {
                    Ok(model) => Some(model),
                    Err(error) => {
                        return ApiError::Internal(format!(
                            "authority returned invalid chat model: {error}"
                        ))
                        .into_response();
                    }
                };
            }
            Ok(Some(_)) => {
                return ApiError::Internal("unexpected remote chat model response".to_string())
                    .into_response();
            }
            Ok(None) => {
                let confirmed = match state
                    .conversation_input_intake
                    .update_authority_chat_model(agent_id, chat_id, model_value)
                    .await
                {
                    Ok(model) => {
                        model_persisted_by_authority = true;
                        model
                    }
                    Err(error) => return ApiError::BadRequest(error).into_response(),
                };
                chat.model = match serde_json::from_value(confirmed) {
                    Ok(model) => Some(model),
                    Err(error) => {
                        return ApiError::Internal(format!(
                            "local authority returned invalid chat model: {error}"
                        ))
                        .into_response();
                    }
                };
            }
            Err(error) => return error.into_response(),
        }
    }
    let configuration_address = rua_core::ChatAddress::agent(agent_id, chat_id);
    let configuration_gate = state
        .conversation_authority
        .transition_gate(&configuration_address);
    let _configuration_transition = if body.collaboration_mode.is_some() {
        Some(configuration_gate.lock().await)
    } else {
        None
    };
    if let (Some(turn_id), Some(selected_variant_id)) =
        (body.turn_id.as_deref(), body.selected_variant_id.as_deref())
    {
        let address = rua_core::ChatAddress::agent(agent_id, chat_id);
        match rua_chat::ChatService::for_resolved_scope(ws_dir, address.clone())
            .select_variant(&address, turn_id, selected_variant_id)
            .await
        {
            Ok(_) => {}
            Err(rua_core::RuaError::Validation { .. }) => {
                return (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    Json(serde_json::json!({
                        "detail": "selected_variant_id is not valid for turn_id"
                    })),
                )
                    .into_response();
            }
            Err(error) => return ApiError::from(error).into_response(),
        }
    }
    let mut listing_persisted_by_common_chat = false;
    if body.name.is_some() || body.pinned.is_some() || body.tag.is_some() || body.archived.is_some()
    {
        let address = rua_core::ChatAddress::agent(agent_id, chat_id);
        match rua_chat::ChatService::for_resolved_scope(ws_dir, address.clone())
            .update_listing(
                &address,
                rua_chat::ChatListUpdate {
                    title: body.name.clone(),
                    pinned: body.pinned,
                    tag: body.tag,
                    archived: body.archived,
                },
            )
            .await
        {
            Ok(common_chat) => match chat_spec_from_common_chat(ws_dir, &common_chat).await {
                Ok(common_spec) => {
                    chat.name = common_spec.name;
                    chat.pinned = common_spec.pinned;
                    chat.tag = common_spec.tag;
                    chat.archived = common_spec.archived;
                    chat.updated_at = common_spec.updated_at;
                    listing_persisted_by_common_chat = true;
                }
                Err(error) => return ApiError::from(error).into_response(),
            },
            Err(rua_core::RuaError::NotFound { .. }) => {}
            Err(error) => return ApiError::from(error).into_response(),
        }
    }
    if !listing_persisted_by_common_chat {
        chat.updated_at = Utc::now();
    }
    let updated = chat.clone();

    if body.name.is_some()
        || body.pinned.is_some()
        || body.tag.is_some()
        || body.collaboration_mode.is_some()
        || updated.model.is_some()
    {
        let store = WorkspaceSessionStore::new(ws_dir);
        let mut header = match store.load_chat_header(chat_id).await {
            Ok(Some(header)) => header,
            Ok(None) => chat_session_header(&updated, agent_id),
            Err(e) => return ApiError::from(e).into_response(),
        };
        let previous_epoch = header
            .extra
            .get(crate::services::product_hooks::CHAT_HOOK_CONFIGURATION_META_KEY)
            .and_then(|value| value.get("epoch"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        if !listing_persisted_by_common_chat {
            if let Some(ref name) = body.name {
                header.metadata.title = name.clone();
            }
            if let Some(pinned) = body.pinned {
                header.metadata.pinned = pinned;
            }
            if let Some(tag) = body.tag {
                header.tag = tag;
            }
        }
        sync_chat_model_to_header_extra(&updated.model, &mut header.extra);
        header.extra.insert(
            rua_core::CHAT_COLLABORATION_MODE_META_KEY.to_string(),
            serde_json::json!(updated.collaboration_mode),
        );
        if body.collaboration_mode.is_some() {
            if let Some(configuration) = header
                .extra
                .get_mut(crate::services::product_hooks::CHAT_HOOK_CONFIGURATION_META_KEY)
                .and_then(serde_json::Value::as_object_mut)
            {
                configuration.insert(
                    "collaboration_mode".to_string(),
                    serde_json::json!(updated.collaboration_mode),
                );
                configuration.insert(
                    "epoch".to_string(),
                    serde_json::json!(format!("hook-config-{}", uuid::Uuid::new_v4())),
                );
            }
        }
        header.metadata.last_activity_at = updated.updated_at.to_rfc3339();
        if body.collaboration_mode.is_some() {
            if let (Some(epoch), Some(configuration)) = (
                previous_epoch.as_deref(),
                header
                    .extra
                    .get(crate::services::product_hooks::CHAT_HOOK_CONFIGURATION_META_KEY),
            ) {
                if let Err(error) = store
                    .replace_chat_configuration(chat_id, epoch, configuration.clone())
                    .await
                {
                    return ApiError::from(error).into_response();
                }
            }
        }
        if let Err(e) = store.save_chat_header(&header).await {
            return ApiError::from(e).into_response();
        }
        if body.collaboration_mode.is_some() {
            if let Some(value) = header
                .extra
                .get(crate::services::product_hooks::CHAT_HOOK_CONFIGURATION_META_KEY)
            {
                match serde_json::from_value(value.clone()) {
                    Ok(configuration) => {
                        state
                            .product_hooks
                            .adopt_chat_configuration(&configuration_address, configuration)
                            .await
                    }
                    Err(error) => {
                        return ApiError::Internal(format!("invalid Chat configuration: {error}"))
                            .into_response()
                    }
                }
            }
        }
    }

    if let Err(e) = upsert_chat_index(ws_dir, &updated).await {
        return e.into_response();
    }

    if non_model_update_requested || !model_update_requested || !model_persisted_by_authority {
        let updated_val = serde_json::to_value(&updated).unwrap_or_default();
        publish_chat_index_upsert(state, agent_id, chat_id, updated_val).await;
        publish_evergreen_chat_snapshot(state, ws_dir, agent_id, chat_id).await;
    }

    (StatusCode::OK, Json(updated)).into_response()
}

async fn update_authority_placement_core(
    agent_id: &str,
    ws_dir: &Path,
    chat_id: &str,
    body: ConversationAuthorityPlacementUpdateBody,
    state: &AppState,
) -> Response {
    let next_device_id = body.device_id.trim();
    if next_device_id.is_empty() {
        return ApiError::BadRequest("device_id is required".to_string()).into_response();
    }
    if state
        .conversation_authority
        .active_turn(&rua_core::ChatAddress::agent(agent_id, chat_id))
        .await
        .is_some()
    {
        return ApiError::Conflict(
            "cannot change authority placement while the chat has an active turn".to_string(),
        )
        .into_response();
    }
    let current = match crate::services::conversation_authority_target::load_optional_chat_authority_placement(
        ws_dir,
        chat_id,
    )
    .await
    {
        Ok(Some(placement)) => {
            let header_acp_active = match WorkspaceSessionStore::new(ws_dir)
                .load_chat_header(chat_id)
                .await
            {
                Ok(Some(header)) => {
                    chat_acp_binding_active(&serde_json::Value::Object(header.extra))
                }
                Ok(None) => false,
                Err(error) => return ApiError::from(error).into_response(),
            };
            if header_acp_active {
                return ApiError::Conflict(
                    "Current chat is in ACP mode. Exit ACP before changing the authority device."
                        .to_string(),
                )
                .into_response();
            }
            placement
        }
        Ok(None) => return ApiError::NotFound("Chat is not materialized".into()).into_response(),

        Err(error) => return ApiError::Internal(error).into_response(),
    };
    if current.device_id == next_device_id {
        return match chat_index::get(ws_dir, chat_id).await {
            Ok(Some(entry)) => (StatusCode::OK, Json(chat_from_index_entry(entry))).into_response(),
            Ok(None) => ApiError::NotFound(format!("chat not found: {chat_id}")).into_response(),
            Err(error) => ApiError::Internal(error.to_string()).into_response(),
        };
    }
    let address = rua_core::ChatAddress::agent(agent_id, chat_id);
    let required_manifest = match authority_switch_manifest(ws_dir, &address).await {
        Ok(manifest) => manifest,
        Err(error) => return error.into_response(),
    };
    let base_manifest_hash = match required_manifest.hash() {
        Ok(hash) => hash,
        Err(error) => return ApiError::from(error).into_response(),
    };
    let local_device_id = match device_id_from_sync_device(&state.sync_device) {
        Some(device_id) => device_id,
        None => {
            return ApiError::Unavailable("local Device identity is unavailable".to_string())
                .into_response();
        }
    };
    let remote_adoption = next_device_id != local_device_id;
    let target = if remote_adoption {
        let session_manager = match crate::services::device_session_access::current_session_manager(
            &state.device_network,
        )
        .await
        {
            Ok(manager) => manager,
            Err(error) => return ApiError::Unavailable(error).into_response(),
        };
        let response = deliver_conversation_authority_adopt_via_session(
            &session_manager,
            &ConversationAuthorityAdoptDeviceCommand {
                command_id: format!("authority-adopt-{}", uuid::Uuid::new_v4()),
                agent_id: agent_id.to_string(),
                address: rua_core::ChatAddress::agent(agent_id, chat_id),
                source_device_id: local_device_id.clone(),
                authority_source_device_id: current.device_id.clone(),
                target_device_id: next_device_id.to_string(),
                expected_authority_epoch: current.authority_epoch,
                required_manifest,
            },
            Duration::from_secs(35),
        )
        .await;
        match response {
            Ok(response)
                if response.placement.device_id == next_device_id
                    && response.placement.authority_epoch > current.authority_epoch
                    && response.placement.base_manifest_hash == base_manifest_hash =>
            {
                response.placement
            }
            Ok(_) => {
                return ApiError::Conflict(
                    "target Device confirmed a different Conversation Authority placement"
                        .to_string(),
                )
                .into_response();
            }
            Err(error) => return authority_adopt_delivery_error(error).into_response(),
        }
    } else {
        if let Err(error) = state
            .conversation_input_intake
            .validate_authority_adoption_model(agent_id, chat_id)
            .await
        {
            return ApiError::Conflict(error).into_response();
        }
        ConversationAuthorityPlacement {
            device_id: next_device_id.to_string(),
            authority_epoch: ConversationAuthorityEpoch::next_for_device(
                next_device_id,
                Some(current.authority_epoch),
            ),
            base_manifest_hash,
        }
    };
    let supersession_target = target.clone();
    let updated = match if remote_adoption {
        state
            .conversation_authority
            .apply_confirmed_authority_placement(
                ws_dir,
                &rua_core::ChatAddress::agent(agent_id, chat_id),
                target,
            )
            .await
    } else {
        state
            .conversation_authority
            .switch_authority(
                Some(state),
                ws_dir,
                &rua_core::ChatAddress::agent(agent_id, chat_id),
                target,
            )
            .await
    } {
        Ok(record) => chat_from_metadata_record(record),
        Err(error) => return ApiError::from(error).into_response(),
    };
    if !remote_adoption {
        spawn_conversation_authority_supersession(
            Some(&state.sync_transfer_session_manager),
            agent_id,
            chat_id,
            &current.device_id,
            current.authority_epoch,
            &supersession_target,
        );
    }

    (StatusCode::OK, Json(updated)).into_response()
}

async fn authority_switch_manifest(
    ws_dir: &Path,
    address: &rua_core::ChatAddress,
) -> Result<crate::services::conversation_manifest::ConversationManifest, ApiError> {
    let manifest =
        crate::services::conversation_manifest::load_conversation_manifest(ws_dir, address)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| {
                ApiError::Conflict("conversation manifest is unavailable".to_string())
            })?;
    if manifest
        .latest_turn_status
        .as_deref()
        .is_some_and(|status| {
            !rua_session::TurnLifecycleStatus::parse(status).is_syncable_conversation_state()
        })
    {
        return Err(ApiError::Conflict(
            "latest conversation turn is not terminal".to_string(),
        ));
    }
    Ok(manifest)
}

fn authority_adopt_delivery_error(error: DeviceCommandDeliveryError) -> ApiError {
    if error.is_session_version_incompatible() {
        return ApiError::UpgradeRequired(error.to_string());
    }
    match error {
        DeviceCommandDeliveryError::Rejected { reason, detail } => {
            if reason.contains("manifest")
                || reason.contains("history")
                || reason.contains("authority")
                || matches!(
                    reason.as_str(),
                    "active_turn"
                        | "acp_active"
                        | "model_unavailable"
                        | "duplicate_command_conflict"
                )
            {
                ApiError::Conflict(detail)
            } else {
                ApiError::Unavailable(format!("{reason}: {detail}"))
            }
        }
        DeviceCommandDeliveryError::Timeout => {
            ApiError::Unavailable("target Device authority adoption timed out".to_string())
        }
        other => ApiError::Unavailable(format!("target Device authority adoption failed: {other}")),
    }
}

fn chat_command_delivery_error(error: DeviceCommandDeliveryError, operation: &str) -> ApiError {
    if error.is_session_version_incompatible() {
        return ApiError::UpgradeRequired(error.to_string());
    }
    match error {
        DeviceCommandDeliveryError::Rejected { reason, detail } => {
            if reason.contains("not_found") {
                ApiError::NotFound(detail)
            } else if reason.contains("stale")
                || reason.contains("active_turn")
                || reason.contains("not_editable")
                || reason.contains("send_now")
                || reason.contains("queue")
                || reason.contains("redirect")
            {
                ApiError::Conflict(detail)
            } else if reason.contains("model_update_rejected")
                || reason.contains("running_config_update_rejected")
                || reason.contains("invalid")
                || reason.contains("wrong")
                || reason.contains("missing")
            {
                ApiError::BadRequest(detail)
            } else if reason.contains("unavailable") {
                ApiError::Unavailable(detail)
            } else {
                ApiError::Conflict(format!("{reason}: {detail}"))
            }
        }
        DeviceCommandDeliveryError::Json(error) => {
            ApiError::Internal(format!("{operation} response was invalid: {error}"))
        }
        DeviceCommandDeliveryError::Timeout => {
            ApiError::Unavailable(format!("{operation} timed out"))
        }
        other => ApiError::Unavailable(format!("{operation} failed: {other}")),
    }
}

async fn rollback_chat_core(
    agent_id: &str,
    ws_dir: &Path,
    chat_id: &str,
    body: BranchActionBody,
    state: &AppState,
) -> Response {
    match dispatch_agent_chat_control(
        state,
        agent_id,
        ws_dir,
        chat_id,
        ConversationControlDeviceAction::RollbackBeforeTurn {
            before_turn_id: body.before_turn_id,
        },
    )
    .await
    {
        Ok(ConversationControlDeviceResponse::RolledBack {
            chat,
            cancelled_tasks,
        }) => Json(serde_json::json!({
            "chat": chat,
            "cancelled_tasks": cancelled_tasks,
        }))
        .into_response(),
        Ok(_) => {
            ApiError::Internal("unexpected Chat rollback response".to_string()).into_response()
        }
        Err(ApiError::BadRequest(detail)) if detail.contains("middle of an ACP segment") => {
            interior_acp_segment_branch_point_response()
        }
        Err(ApiError::BadRequest(_)) => invalid_branch_point_response(),
        Err(error) => error.into_response(),
    }
}

async fn branch_chat_core(
    agent_id: &str,
    ws_dir: &Path,
    chat_id: &str,
    body: BranchActionBody,
    state: &AppState,
) -> Response {
    match chat_index::get(ws_dir, chat_id).await {
        Ok(Some(_)) => {}
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"detail": "chat not found"})),
            )
                .into_response();
        }
        Err(e) => {
            return ApiError::Internal(format!("Failed to get chat index: {e}")).into_response();
        }
    }

    let address = rua_core::ChatAddress::agent(agent_id, chat_id);
    let chat_service = rua_chat::ChatService::for_resolved_scope(ws_dir, address.clone());
    if let Err(error) = chat_service
        .resolve_history_boundary(&address, &body.before_turn_id)
        .await
    {
        return history_boundary_error_response(error);
    }
    let common_chat = match chat_service
        .branch_before(&address, &body.before_turn_id)
        .await
    {
        Ok(chat) => chat,
        Err(rua_core::RuaError::Validation { .. }) => return invalid_branch_point_response(),
        Err(error) => return ApiError::from(error).into_response(),
    };
    let branch_chat = match chat_spec_from_common_chat(ws_dir, &common_chat).await {
        Ok(chat) => chat,
        Err(error) => return ApiError::from(error).into_response(),
    };
    let chat_val = serde_json::to_value(&branch_chat).unwrap_or_default();
    publish_chat_index_upsert(state, agent_id, &branch_chat.id, chat_val).await;
    publish_evergreen_chat_snapshot(state, ws_dir, agent_id, &branch_chat.id).await;

    (
        StatusCode::OK,
        Json(BranchActionResponse { chat: branch_chat }),
    )
        .into_response()
}

fn invalid_branch_point_response() -> Response {
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        Json(serde_json::json!({
            "detail": "selected_variant_id is not valid for turn_id"
        })),
    )
        .into_response()
}

fn active_acp_branch_point_response() -> Response {
    (
        StatusCode::CONFLICT,
        Json(serde_json::json!({
            "detail": "Current chat is in ACP mode. Exit ACP before rollback or branch."
        })),
    )
        .into_response()
}

fn interior_acp_segment_branch_point_response() -> Response {
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        Json(serde_json::json!({
            "detail": "Cannot rollback or branch from the middle of an ACP segment. Choose the last response in the ACP segment."
        })),
    )
        .into_response()
}

fn history_boundary_error_response(error: rua_core::RuaError) -> Response {
    match error {
        rua_core::RuaError::Validation { message } if message.contains("ACP mode") => {
            active_acp_branch_point_response()
        }
        rua_core::RuaError::Validation { message }
            if message.contains("middle of an ACP segment") =>
        {
            interior_acp_segment_branch_point_response()
        }
        rua_core::RuaError::Validation { .. } => invalid_branch_point_response(),
        error => ApiError::from(error).into_response(),
    }
}

impl ChatUpdateBody {
    fn read_only_field(&self) -> Option<&'static str> {
        if self.id.is_some() {
            Some("id")
        } else if self.user_id.is_some() {
            Some("user_id")
        } else if self.channel.is_some() {
            Some("channel")
        } else if self.created_at.is_some() {
            Some("created_at")
        } else if self.updated_at.is_some() {
            Some("updated_at")
        } else if self.meta.is_some() {
            Some("meta")
        } else if self.status.is_some() {
            Some("status")
        } else {
            None
        }
    }
}

async fn cleanup_chat_runtime_state(
    state: &AppState,
    agent_id: &str,
    chat_id: &str,
) -> Result<(), ApiError> {
    let owner = rua_task::TaskOwner::new(agent_id, chat_id);
    let removed_tasks = state
        .task_runtime
        .remove_owner(&owner)
        .await
        .map_err(|error| {
            ApiError::Conflict(format!(
                "failed to stop Tasks before deleting Chat '{chat_id}': {error}"
            ))
        })?;
    tracing::info!(
        target: "diag.session",
        agent_id = %agent_id,
        chat_id = %chat_id,
        removed_tasks,
        "[DIAG-session] cleanup_chat_runtime_state cancel_background_tasks",
    );

    let stopped = state.console_facade.stop_chat(agent_id, chat_id).await;
    tracing::info!(
        target: "diag.session",
        agent_id = %agent_id,
        chat_id = %chat_id,
        stopped,
        "[DIAG-session] cleanup_chat_runtime_state stop_chat",
    );

    let cleared_pending = rua_tool::extended::ask_user::AskUserService::global()
        .clear_by_session(chat_id)
        .await;
    tracing::info!(
        target: "diag.session",
        agent_id = %agent_id,
        chat_id = %chat_id,
        cleared_pending,
        "[DIAG-session] cleanup_chat_runtime_state clear_by_session",
    );

    let cleared_approvals = state
        .security
        .approval_authority()
        .clear_by_address(&rua_core::ChatAddress::agent(agent_id, chat_id))
        .await;
    tracing::info!(
        target: "diag.session",
        agent_id = %agent_id,
        chat_id = %chat_id,
        cleared_approvals,
        "[DIAG-session] cleanup_chat_runtime_state clear_approvals_by_chat",
    );
    Ok(())
}

pub(crate) async fn delete_chat_core(
    agent_id: &str,
    ws_dir: &Path,
    chat_id: &str,
    state: &AppState,
) -> Response {
    tracing::info!(
        target: "diag.session",
        agent_id = %agent_id,
        chat_id = %chat_id,
        ws_dir = %ws_dir.display(),
        "[DIAG-session] delete_chat_core ENTER",
    );
    let chat = match chat_index::get(ws_dir, chat_id).await {
        Ok(Some(entry)) => chat_from_index_entry(entry),
        Ok(None) => {
            if let Err(error) = cleanup_chat_runtime_state(state, agent_id, chat_id).await {
                return error.into_response();
            }
            if let Err(error) = WorkspaceSessionStore::new(ws_dir)
                .delete_chat_storage_by_chat_id(chat_id)
                .await
            {
                return ApiError::from(error).into_response();
            }
            if let Err(error) = chat_index::delete(ws_dir, chat_id).await.map_err(|error| {
                ApiError::Internal(format!("Failed to delete chat index: {error}"))
            }) {
                return error.into_response();
            }
            tracing::info!(
                target: "diag.session",
                agent_id = %agent_id,
                chat_id = %chat_id,
                "[DIAG-session] delete_chat_core NOT_FOUND",
            );
            return ApiError::NotFound(format!("Chat '{chat_id}' not found")).into_response();
        }
        Err(e) => {
            return ApiError::Internal(format!("Failed to get chat index: {e}")).into_response();
        }
    };
    if chat.pinned {
        return ApiError::Conflict(format!(
            "Pinned Chat '{chat_id}' cannot be deleted; Unpin it first"
        ))
        .into_response();
    }
    if chat.id != chat_id {
        if let Err(error) = cleanup_chat_runtime_state(state, agent_id, chat_id).await {
            return error.into_response();
        }
        if let Err(e) = WorkspaceSessionStore::new(ws_dir)
            .delete_chat_storage_by_chat_id(chat_id)
            .await
        {
            return ApiError::from(e).into_response();
        }
        if let Err(e) = chat_index::delete(ws_dir, chat_id)
            .await
            .map_err(|e| ApiError::Internal(format!("Failed to delete chat index: {e}")))
        {
            return e.into_response();
        }
        tracing::info!(
            target: "diag.session",
            agent_id = %agent_id,
            chat_id = %chat_id,
            "[DIAG-session] delete_chat_core NOT_FOUND",
        );
        return ApiError::NotFound(format!("Chat '{chat_id}' not found")).into_response();
    }

    let address = rua_core::ChatAddress::agent(agent_id, &chat.id);
    if let Err(error) =
        crate::services::chat_deletion::ChatDeletionCoordinator::from_rua_root(&state.rua_root)
            .commit_intent(&address)
    {
        return ApiError::Internal(error).into_response();
    }

    if let Err(error) = cleanup_chat_runtime_state(state, agent_id, &chat.id).await {
        return error.into_response();
    }
    let deleting = std::collections::BTreeSet::from([chat.id.clone()]);
    if let Err(error) = crate::services::chat_storage::prepare_chat_deletion(
        state,
        ws_dir,
        rua_core::ChatScope::agent(agent_id),
        agent_id.to_string(),
        &deleting,
    )
    .await
    {
        return chat_deletion_preparation_error(error).into_response();
    }
    if let Err(e) = WorkspaceSessionStore::new(ws_dir)
        .delete_chat_storage_by_chat_id(&chat.id)
        .await
    {
        return ApiError::from(e).into_response();
    }

    if let Err(e) = chat_index::delete(ws_dir, chat_id)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to delete chat index: {e}")))
    {
        return e.into_response();
    }

    tracing::info!(
        target: "diag.session",
        agent_id = %agent_id,
        chat_id = %chat_id,
        "[DIAG-session] delete_chat_core emit_chat_delete OK",
    );

    tracing::info!(
        target: "diag.session",
        agent_id = %agent_id,
        chat_id = %chat_id,
        "[DIAG-session] delete_chat_core DONE",
    );

    let resp = serde_json::json!({ "deleted": true });
    (StatusCode::OK, Json(resp)).into_response()
}

async fn update_pin_order_core(
    agent_id: &str,
    ws_dir: &Path,
    items: Vec<PinOrderItem>,
    state: &AppState,
) -> Response {
    let index_items = items
        .into_iter()
        .filter_map(|item| Some((item.id?, item.pin_order?)))
        .collect::<Vec<_>>();
    if let Err(e) = chat_index::update_pin_order(ws_dir, index_items.clone())
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to update chat pin order index: {e}")))
    {
        return e.into_response();
    }

    for item in &index_items {
        if let Ok(Some(entry)) = chat_index::get(ws_dir, &item.0).await {
            let chat = chat_from_index_entry(entry);
            let chat_val = serde_json::to_value(&chat).unwrap_or_default();
            publish_chat_index_upsert(state, agent_id, &chat.id, chat_val).await;
        }
    }

    (StatusCode::OK, Json(serde_json::json!({"success": true}))).into_response()
}

pub(crate) async fn batch_delete_core(
    agent_id: &str,
    ws_dir: &Path,
    ids: Vec<String>,
    state: &AppState,
) -> Response {
    let mut existing = Vec::new();
    for id in &ids {
        match chat_index::get(ws_dir, id).await {
            Ok(Some(entry)) => existing.push(chat_from_index_entry(entry)),
            Ok(None) => {}
            Err(e) => {
                return ApiError::Internal(format!("Failed to get chat index: {e}"))
                    .into_response();
            }
        }
    }
    let deleted = existing.len();
    if let Some(chat) = existing.iter().find(|chat| chat.pinned) {
        return ApiError::Conflict(format!(
            "Pinned Chat '{}' cannot be deleted; Unpin it first",
            chat.id
        ))
        .into_response();
    }
    let deleting = existing
        .iter()
        .map(|chat| chat.id.clone())
        .collect::<std::collections::BTreeSet<_>>();

    for chat in &existing {
        if let Err(error) = cleanup_chat_runtime_state(state, agent_id, &chat.id).await {
            return error.into_response();
        }
    }
    if let Err(error) = crate::services::chat_storage::prepare_chat_deletion(
        state,
        ws_dir,
        rua_core::ChatScope::agent(agent_id),
        agent_id.to_string(),
        &deleting,
    )
    .await
    {
        return chat_deletion_preparation_error(error).into_response();
    }
    for chat in &existing {
        if let Err(e) = WorkspaceSessionStore::new(ws_dir)
            .delete_chat_storage_by_chat_id(&chat.id)
            .await
        {
            return ApiError::from(e).into_response();
        }
    }

    if let Err(e) = chat_index::delete_many(ws_dir, existing.iter().map(|c| c.id.clone()).collect())
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to delete chat index rows: {e}")))
    {
        return e.into_response();
    }

    let resp = serde_json::json!({ "success": true, "deleted": true, "deleted_count": deleted });
    (StatusCode::OK, Json(resp)).into_response()
}

fn chat_deletion_preparation_error(
    error: crate::services::chat_storage::ChatDeletionPreparationError,
) -> ApiError {
    match error {
        crate::services::chat_storage::ChatDeletionPreparationError::NotFound(detail) => {
            ApiError::NotFound(detail)
        }
        crate::services::chat_storage::ChatDeletionPreparationError::Pinned(detail) => {
            ApiError::Conflict(detail)
        }
        crate::services::chat_storage::ChatDeletionPreparationError::Failed(detail) => {
            ApiError::Unavailable(detail)
        }
    }
}
