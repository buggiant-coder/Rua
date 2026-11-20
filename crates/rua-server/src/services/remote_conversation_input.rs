use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use rua_channel::core::console::ConsoleChatRequest;
use rua_core::{ChatAddress, ChatStore};
use serde::{Deserialize, Serialize};

use crate::services::conversation_authority::SUBMISSION_ID_KEY;
use crate::services::conversation_authority_placement::ConversationAuthorityEpoch;
use crate::services::device_command_bus::{
    deliver_conversation_control_via_session, deliver_remote_chat_control_via_session,
    deliver_remote_conversation_input_via_session, ConversationControlDeviceCommand,
    ConversationControlDeviceResponse, DeviceCommandDeliveryError, RemoteChatControlAction,
    RemoteChatControlCommand, RemoteChatControlResponse, RemoteConversationInputAck,
    RemoteConversationInputMediaRequirement, RemoteConversationInputRequest,
};
use crate::services::device_session_access::current_session_manager;

const SOURCE_PENDING_CONVERSATION_INPUTS_DIR: &str = "source_pending_conversation_inputs";
const DEVICE_COMMAND_DELIVERY_TIMEOUT: Duration = Duration::from_secs(10);
// Initial configuration capture is part of durable input admission. Its finite
// confirmation window starts after any independent media preparation.
const REMOTE_INPUT_CONFIRMATION_TIMEOUT: Duration = Duration::from_secs(30);
const REMOTE_CONVERSATION_INPUT_PRIORITY_SYNC_ATTEMPTS: usize = 3;
const REMOTE_CONVERSATION_INPUT_PRIORITY_SYNC_BACKOFF: Duration = Duration::from_millis(300);

#[derive(Clone)]
pub struct RemoteConversationInputDelivery {
    sync_node: Arc<tokio::sync::Mutex<Option<rua_sync::endpoint::SyncNode>>>,
    device_network: Arc<crate::services::device_session_access::DeviceNetworkAccess>,
    data_dir: PathBuf,
}

pub struct RemoteConversationInputParams<'a> {
    pub address: &'a ChatAddress,
    pub execution_agent_id: &'a str,
    pub request: &'a ConsoleChatRequest,
    pub source_device_id: &'a str,
    pub target_device_id: &'a str,
    pub authority_epoch: ConversationAuthorityEpoch,
    pub base_manifest_hash: &'a str,
    pub workspace_dir: Option<&'a Path>,
    pub materialize_conversation: bool,
}

pub struct RemoteConversationControlParams<'a> {
    pub command_id: &'a str,
    pub address: &'a ChatAddress,
    pub execution_agent_id: &'a str,
    pub source_device_id: &'a str,
    pub target_device_id: &'a str,
    pub authority_epoch: ConversationAuthorityEpoch,
    pub device_delivery_id: Option<String>,
    pub action: RemoteChatControlAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteConversationInputDeliveryOutcome {
    pub accepted: bool,
    pub detail: Option<String>,
    pub submission_id: Option<String>,
    pub input_id: Option<String>,
    pub mode: Option<crate::services::conversation_authority::ConversationInputAcceptanceMode>,
    pub device_delivery_id: Option<String>,
    pub target_device_id: Option<String>,
    pub device_delivery_sequence: Option<u64>,
    pub accepted_at: Option<String>,
    pub failure_kind: Option<String>,
    pub control: Option<Box<crate::services::conversation_authority::ConversationControlView>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourcePendingConversationInput {
    pub agent_id: String,
    pub authority_epoch: ConversationAuthorityEpoch,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_manifest_hash: Option<String>,
    pub address: ChatAddress,
    pub submission_id: String,
    pub input_id: String,
    pub mode: crate::services::conversation_authority::ConversationInputAcceptanceMode,
    pub device_delivery_id: String,
    pub device_delivery_sequence: u64,
    pub target_device_id: String,
    pub accepted_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_seen_at: Option<String>,
}

impl SourcePendingConversationInput {
    fn from_request(
        request: &RemoteConversationInputRequest,
        ack: &RemoteConversationInputAck,
    ) -> Self {
        Self {
            agent_id: request.agent_id.clone(),
            authority_epoch: request.authority_epoch,
            base_manifest_hash: Some(request.base_manifest_hash.clone()),
            address: request.address.clone(),
            submission_id: request.submission_id.clone(),
            input_id: ack.input_id.clone(),
            mode: ack.mode,
            device_delivery_id: request.device_delivery_id.clone(),
            device_delivery_sequence: request.device_delivery_sequence,
            target_device_id: request.target_device_id.clone(),
            accepted_at: ack.accepted_at.clone(),
            terminal_status: None,
            terminal_seen_at: None,
        }
    }
}

impl RemoteConversationInputDeliveryOutcome {
    fn accepted(request: &RemoteConversationInputRequest, ack: RemoteConversationInputAck) -> Self {
        Self {
            accepted: true,
            detail: None,
            submission_id: Some(request.submission_id.clone()),
            input_id: Some(ack.input_id),
            mode: Some(ack.mode),
            device_delivery_id: Some(request.device_delivery_id.clone()),
            target_device_id: Some(ack.target_device_id),
            device_delivery_sequence: Some(request.device_delivery_sequence),
            accepted_at: Some(ack.accepted_at),
            failure_kind: None,
            control: ack.control,
        }
    }

    fn build_failed(error: String, target_device_id: &str) -> Self {
        Self {
            accepted: false,
            detail: Some(error),
            submission_id: None,
            input_id: None,
            mode: None,
            device_delivery_id: None,
            target_device_id: Some(target_device_id.to_string()),
            device_delivery_sequence: None,
            accepted_at: None,
            failure_kind: Some("invalid_request".to_string()),
            control: None,
        }
    }

    fn delivery_failed(
        error: DeviceCommandDeliveryError,
        request: &RemoteConversationInputRequest,
    ) -> Self {
        Self {
            accepted: false,
            detail: Some(error.to_string()),
            submission_id: Some(request.submission_id.clone()),
            input_id: None,
            mode: None,
            device_delivery_id: Some(request.device_delivery_id.clone()),
            target_device_id: Some(request.target_device_id.clone()),
            device_delivery_sequence: Some(request.device_delivery_sequence),
            accepted_at: None,
            failure_kind: Some(error.failure_kind().to_string()),
            control: None,
        }
    }
}

impl RemoteConversationInputDelivery {
    pub fn new(
        sync_node: Arc<tokio::sync::Mutex<Option<rua_sync::endpoint::SyncNode>>>,
        device_network: Arc<crate::services::device_session_access::DeviceNetworkAccess>,
        data_dir: PathBuf,
    ) -> Self {
        Self {
            sync_node,
            device_network,
            data_dir,
        }
    }

    pub fn device_network(
        &self,
    ) -> &Arc<crate::services::device_session_access::DeviceNetworkAccess> {
        &self.device_network
    }

    pub async fn queue_remote_input(
        &self,
        params: RemoteConversationInputParams<'_>,
    ) -> RemoteConversationInputDeliveryOutcome {
        self.deliver_remote_input(params, "remote queue").await
    }

    pub async fn priority_sync_to_device(&self, device_id: &str) {
        for attempt in 1..=REMOTE_CONVERSATION_INPUT_PRIORITY_SYNC_ATTEMPTS {
            match crate::sync_cycle::run_sync_cycle_to_device_id(
                &self.sync_node,
                &self.data_dir.join("sync"),
                device_id,
            )
            .await
            {
                Ok(result) => {
                    tracing::info!(
                        device_id,
                        attempt,
                        peer_node_id = %result.peer_node_id,
                        events_sent = result.events_sent,
                        events_received = result.events_received,
                        "remote conversation input result priority sync completed"
                    );
                    return;
                }
                Err(error) if attempt < REMOTE_CONVERSATION_INPUT_PRIORITY_SYNC_ATTEMPTS => {
                    tracing::warn!(
                        device_id,
                        attempt,
                        attempts = REMOTE_CONVERSATION_INPUT_PRIORITY_SYNC_ATTEMPTS,
                        error = %error,
                        "remote conversation input result priority sync failed; retrying"
                    );
                    tokio::time::sleep(
                        REMOTE_CONVERSATION_INPUT_PRIORITY_SYNC_BACKOFF * attempt as u32,
                    )
                    .await;
                }
                Err(error) => {
                    tracing::warn!(
                        device_id,
                        attempt,
                        attempts = REMOTE_CONVERSATION_INPUT_PRIORITY_SYNC_ATTEMPTS,
                        error = %error,
                        "remote conversation input result priority sync failed"
                    );
                }
            }
        }
    }

    pub fn spawn_priority_sync_to_device(&self, device_id: impl Into<String>) {
        let delivery = self.clone();
        let device_id = device_id.into();
        tokio::spawn(async move {
            delivery.priority_sync_to_device(&device_id).await;
        });
    }

    pub async fn build_remote_conversation_input_request(
        &self,
        params: &RemoteConversationInputParams<'_>,
    ) -> std::result::Result<RemoteConversationInputRequest, String> {
        let sequence = match params.workspace_dir {
            Some(workspace_dir) => self
                .next_device_delivery_sequence(workspace_dir, params.address)
                .await
                .unwrap_or(1),
            None => 1,
        };
        let now = chrono::Utc::now();
        let input = build_remote_conversation_input(params.request);
        let mut media_requirements = Vec::new();
        for id in input_media_ids(input.as_deref())? {
            if let Some(dir) = params.workspace_dir {
                let project = params
                    .request
                    .extra
                    .get("project_id")
                    .and_then(|v| v.as_str());
                crate::services::first_chat_submission::validate_staged_media(
                    &dir.join("media"),
                    &id.to_string(),
                    params.address,
                    Some(project),
                )
                .await?;
            }
            let manifest = self
                .submission_manifest(
                    params.address,
                    &id.to_string(),
                    params.source_device_id,
                    Some(params.target_device_id),
                    None,
                )
                .await?;
            media_requirements.push(RemoteConversationInputMediaRequirement {
                media_ref: manifest.media_ref,
                filename: id.to_string(),
                mime_type: manifest.mime_type,
                byte_size: manifest.byte_size,
                sha256: manifest.sha256,
            });
        }
        let submission_id = params
            .request
            .extra
            .get(SUBMISSION_ID_KEY)
            .and_then(|value| value.as_str())
            .filter(|value| !value.trim().is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("submission-{}", uuid::Uuid::new_v4()));
        let mut metadata = serde_json::Map::new();
        if let Some(project_id) = params.request.extra.get("project_id") {
            metadata.insert("project_id".into(), project_id.clone());
        }
        if params.materialize_conversation {
            for key in [
                rua_session::EXECUTION_DIRECTORY_KEY,
                "collaboration_mode",
                "harness_configuration",
                "user_id",
                "channel",
            ] {
                if let Some(value) = params.request.extra.get(key) {
                    metadata.insert(key.into(), value.clone());
                }
            }
            metadata.insert("first_submission".into(), serde_json::Value::Bool(true));
            if let Some(acp) = params
                .request
                .extra
                .get(crate::services::conversation_input_intake::INITIAL_ACP_KEY)
            {
                metadata.insert(
                    crate::services::conversation_input_intake::INITIAL_ACP_KEY.into(),
                    acp.clone(),
                );
            }
            if let Some(model) = params.request.extra.get("chat_model_slot") {
                metadata.insert("chat_model_slot".to_string(), model.clone());
            }
        }
        Ok(RemoteConversationInputRequest {
            address: params.address.clone(),
            agent_id: params.execution_agent_id.to_string(),
            source_device_id: params.source_device_id.to_string(),
            target_device_id: params.target_device_id.to_string(),
            authority_epoch: params.authority_epoch,
            base_manifest_hash: params.base_manifest_hash.to_string(),
            submission_id,
            device_delivery_id: format!("device-delivery-{}", uuid::Uuid::new_v4()),
            device_delivery_sequence: sequence,
            text: params.request.message.clone(),
            input,
            media_requirements,
            created_at: now.to_rfc3339(),
            delivery_deadline_at: (now + chrono::Duration::seconds(30)).to_rfc3339(),
            metadata,
        })
    }

    pub async fn deliver_prepared_conversation_input(
        &self,
        request: &RemoteConversationInputRequest,
        log_context: &str,
    ) -> Result<RemoteConversationInputDeliveryOutcome, DeviceCommandDeliveryError> {
        let ack = self
            .deliver_remote_conversation_input_to_target(request)
            .await?;
        let outcome = RemoteConversationInputDeliveryOutcome::accepted(request, ack.clone());
        let scope = ChatStore::resolve(&self.data_dir, request.address.clone());
        let ids: Vec<String> = request
            .media_requirements
            .iter()
            .map(|media| media.filename.clone())
            .collect();
        if let Err(error) =
            crate::services::chat_media::ChatMediaService::confirm_staged_media(scope.root(), &ids)
                .await
        {
            tracing::warn!(%error, "accepted remote input media staging marker retained");
        }
        if let Err(error) = self.record_source_pending(request, &ack).await {
            tracing::warn!(
                chat_address = %request.address,
                device_delivery_id = %request.device_delivery_id,
                error = %error,
                "failed to record source-side pending {log_context} marker"
            );
        }
        Ok(outcome)
    }

    pub(crate) async fn submission_manifest(
        &self,
        address: &ChatAddress,
        media_id: &str,
        local_device: &str,
        source: Option<&str>,
        first_project: Option<Option<&str>>,
    ) -> Result<rua_session::chat_media::ChatMediaManifest, String> {
        use crate::services::chat_media::{
            chat_media_manifest, chat_media_manifest_from_device,
            chat_media_manifest_from_device_for_submission, local_chat_media_manifest,
            ChatMediaError,
        };
        if first_project.is_none() {
            match local_chat_media_manifest(&self.data_dir, address, media_id).await {
                Ok(manifest) => return Ok(manifest),
                Err(ChatMediaError::NotFound) => {}
                Err(error) => return Err(error.to_string()),
            }
        }
        let manager = self
            .device_network
            .lock()
            .await
            .as_ref()
            .map(|runtime| runtime.session_manager().clone());
        let manifest = match (source, manager.as_ref(), first_project) {
            (Some(holder), Some(manager), Some(project)) => {
                chat_media_manifest_from_device_for_submission(
                    manager, holder, address, media_id, project,
                )
                .await
            }
            (_, _, Some(_)) => {
                return Err("Cannot confirm staged Chat Media on the submitting Device".into())
            }
            (Some(holder), Some(manager), None) if holder != local_device => {
                chat_media_manifest_from_device(manager, holder, address, media_id).await
            }
            _ => {
                chat_media_manifest(
                    &self.data_dir,
                    address,
                    media_id,
                    local_device,
                    manager.as_ref(),
                )
                .await
            }
        }
        .map_err(|error| error.to_string())?;
        let media_dir = rua_core::RuaRoot::new(&self.data_dir)
            .chat_workspace()
            .as_path()
            .join("media");
        manifest
            .register(&media_dir)
            .map_err(|error| error.to_string())?;
        Ok(manifest)
    }

    pub async fn send_remote_control(
        &self,
        params: RemoteConversationControlParams<'_>,
    ) -> Result<RemoteChatControlResponse, DeviceCommandDeliveryError> {
        let command = RemoteChatControlCommand {
            command_id: params.command_id.to_string(),
            agent_id: params.execution_agent_id.to_string(),
            address: params.address.clone(),
            source_device_id: params.source_device_id.to_string(),
            target_device_id: params.target_device_id.to_string(),
            authority_epoch: params.authority_epoch,
            device_delivery_id: params.device_delivery_id,
            await_terminal: matches!(params.action, RemoteChatControlAction::StopCurrentTurn),
            action: params.action,
        };
        self.deliver_remote_chat_control_to_target(&command).await
    }

    pub async fn send_conversation_control(
        &self,
        command: ConversationControlDeviceCommand,
    ) -> Result<ConversationControlDeviceResponse, DeviceCommandDeliveryError> {
        let session_manager = current_session_manager(&self.device_network)
            .await
            .map_err(DeviceCommandDeliveryError::Connect)?;
        deliver_conversation_control_via_session(
            &session_manager,
            &command,
            DEVICE_COMMAND_DELIVERY_TIMEOUT,
        )
        .await
    }

    pub async fn source_pending_device_delivery_id(
        &self,
        address: &ChatAddress,
    ) -> std::result::Result<Option<String>, String> {
        Ok(self
            .read_source_pending(address)
            .await?
            .map(|pending| pending.device_delivery_id))
    }

    pub async fn clear_source_pending_if_stable_locally(
        &self,
        workspace_dir: &Path,
        address: &ChatAddress,
    ) -> std::result::Result<bool, String> {
        let Some(pending) = self.read_source_pending(address).await? else {
            return Ok(false);
        };
        if !device_delivery_finalized_locally(
            workspace_dir,
            address.chat_id().as_str(),
            &pending.device_delivery_id,
        )
        .await?
        {
            return Ok(false);
        }
        self.clear_source_pending(address).await?;
        Ok(true)
    }

    pub async fn record_source_pending(
        &self,
        request: &RemoteConversationInputRequest,
        ack: &RemoteConversationInputAck,
    ) -> std::result::Result<(), String> {
        let path = source_pending_conversation_input_path(&self.data_dir, &request.address);
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|error| error.to_string())?;
        }
        let value =
            serde_json::to_value(SourcePendingConversationInput::from_request(request, ack))
                .map_err(|error| error.to_string())?;
        rua_config::atomic_write_json(&path, &value).map_err(|error| error.to_string())
    }

    pub async fn mark_source_pending_terminal_seen(
        &self,
        address: &ChatAddress,
        device_delivery_id: &str,
        terminal_status: &str,
    ) -> std::result::Result<(), String> {
        let Some(mut pending) = self.read_source_pending(address).await? else {
            return Ok(());
        };
        if pending.device_delivery_id != device_delivery_id {
            return Ok(());
        }
        pending.terminal_status = Some(terminal_status.to_string());
        pending.terminal_seen_at = Some(chrono::Utc::now().to_rfc3339());
        let path = source_pending_conversation_input_path(&self.data_dir, address);
        let value = serde_json::to_value(pending).map_err(|error| error.to_string())?;
        rua_config::atomic_write_json(&path, &value).map_err(|error| error.to_string())
    }

    pub async fn read_source_pending(
        &self,
        address: &ChatAddress,
    ) -> std::result::Result<Option<SourcePendingConversationInput>, String> {
        read_source_pending_at(&self.data_dir, address).await
    }

    pub async fn clear_source_pending(
        &self,
        address: &ChatAddress,
    ) -> std::result::Result<(), String> {
        let path = source_pending_conversation_input_path(&self.data_dir, address);
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!(
                "remove pending remote conversation input marker {}: {error}",
                path.display()
            )),
        }
    }

    async fn deliver_remote_input(
        &self,
        params: RemoteConversationInputParams<'_>,
        log_context: &str,
    ) -> RemoteConversationInputDeliveryOutcome {
        if let Some(workspace_dir) = params.workspace_dir {
            if let Err(error) = self
                .clear_source_pending_if_stable_locally(workspace_dir, params.address)
                .await
            {
                tracing::warn!(
                    chat_address = %params.address,
                    error = %error,
                    "failed to clear finalized source-side pending {log_context} marker"
                );
            }
        }
        let remote_request = match self.build_remote_conversation_input_request(&params).await {
            Ok(request) => request,
            Err(error) => {
                return RemoteConversationInputDeliveryOutcome::build_failed(
                    error,
                    params.target_device_id,
                );
            }
        };
        match self
            .deliver_prepared_conversation_input(&remote_request, log_context)
            .await
        {
            Ok(outcome) => outcome,
            Err(error) => {
                RemoteConversationInputDeliveryOutcome::delivery_failed(error, &remote_request)
            }
        }
    }

    async fn next_device_delivery_sequence(
        &self,
        workspace_dir: &Path,
        address: &ChatAddress,
    ) -> Option<u64> {
        let store = rua_session::WorkspaceSessionStore::new(workspace_dir);
        let messages = store
            .read_message_range(address.chat_id().as_str(), 1, usize::MAX)
            .await
            .ok()?;
        let history_max = messages
            .iter()
            .filter_map(|message| {
                message
                    .metadata
                    .get("device_delivery_sequence")
                    .and_then(|value| value.as_u64())
            })
            .max()
            .unwrap_or(0);
        let pending_max = self
            .read_source_pending(address)
            .await
            .ok()
            .flatten()
            .map(|pending| pending.device_delivery_sequence)
            .unwrap_or(0);
        Some(history_max.max(pending_max) + 1)
    }

    async fn deliver_remote_conversation_input_to_target(
        &self,
        request: &RemoteConversationInputRequest,
    ) -> Result<RemoteConversationInputAck, DeviceCommandDeliveryError> {
        let deadline = tokio::time::Instant::now() + REMOTE_INPUT_CONFIRMATION_TIMEOUT;
        // Keep the prepared wire payload stable so domain idempotency can
        // recognize this same attempt. This local confirmation clock starts
        // after media preparation; it does not reset payload timestamps.
        let session_manager = self
            .device_network
            .session_manager(deadline)
            .await
            .map_err(DeviceCommandDeliveryError::Connect)?;
        deliver_remote_conversation_input_via_session(
            &session_manager,
            request,
            deadline.saturating_duration_since(tokio::time::Instant::now()),
        )
        .await
    }

    async fn deliver_remote_chat_control_to_target(
        &self,
        command: &RemoteChatControlCommand,
    ) -> Result<RemoteChatControlResponse, DeviceCommandDeliveryError> {
        let session_manager = current_session_manager(&self.device_network)
            .await
            .map_err(DeviceCommandDeliveryError::Connect)?;
        deliver_remote_chat_control_via_session(
            &session_manager,
            command,
            DEVICE_COMMAND_DELIVERY_TIMEOUT,
        )
        .await
    }
}

fn build_remote_conversation_input(request: &ConsoleChatRequest) -> Option<Vec<serde_json::Value>> {
    let mut input = request
        .input
        .as_ref()
        .filter(|value| !value.is_empty())
        .or_else(|| request.messages.as_ref().filter(|value| !value.is_empty()))
        .cloned()
        .unwrap_or_else(|| {
            let mut content = Vec::new();
            if !request.message.is_empty() {
                content.push(serde_json::json!({ "type": "text", "text": request.message }));
            }
            vec![serde_json::json!({
                "role": "user",
                "content": content,
            })]
        });

    if !request.attachments.is_empty() {
        let parts = rua_channel::core::console::ordered_user_content_parts(
            "",
            None,
            None,
            &request.attachments,
        )
        .iter()
        .map(agent_types::content::content_part_to_stored_value)
        .collect::<Vec<_>>();
        if let Some(content) = input
            .iter_mut()
            .rev()
            .find(|m| m["role"] == "user")
            .and_then(|m| m["content"].as_array_mut())
        {
            content.extend(parts);
        } else {
            input.push(serde_json::json!({"role":"user","content":parts}));
        }
    }
    (!input.is_empty()).then_some(input)
}

pub(crate) fn input_media_ids(
    input: Option<&[serde_json::Value]>,
) -> Result<std::collections::BTreeSet<uuid::Uuid>, String> {
    let mut ids = std::collections::BTreeSet::new();
    for message in input.unwrap_or_default() {
        let Some(parts) = message.get("content").and_then(|v| v.as_array()) else {
            continue;
        };
        for part in parts {
            if !matches!(
                part.get("type").and_then(|v| v.as_str()),
                Some("image" | "file" | "audio" | "video")
            ) {
                continue;
            }
            if let Some(value) = part.get("media_ref") {
                let id = value
                    .as_str()
                    .and_then(rua_core::chat_media::reference_id)
                    .ok_or("Invalid Chat media reference")?;
                ids.insert(id);
            } else if let Some(id) = ["url", "file_url", "image_url", "audio_url", "video_url"]
                .iter()
                .filter_map(|key| part.get(key)?.as_str())
                .find_map(rua_core::chat_media::reference_id)
            {
                ids.insert(id);
            }
        }
    }
    Ok(ids)
}

async fn device_delivery_finalized_locally(
    workspace_dir: &Path,
    chat_id: &str,
    device_delivery_id: &str,
) -> std::result::Result<bool, String> {
    let store = rua_session::WorkspaceSessionStore::new(workspace_dir);
    let turns = store
        .read_turn_range(chat_id, 1, usize::MAX)
        .await
        .map_err(|error| error.to_string())?;
    for turn in turns {
        let messages = store
            .read_turn_messages(chat_id, &turn.turn_id)
            .await
            .map_err(|error| error.to_string())?;
        let belongs_to_device_delivery = messages.iter().any(|message| {
            message
                .metadata
                .get("device_delivery_id")
                .and_then(|value| value.as_str())
                == Some(device_delivery_id)
        });
        if belongs_to_device_delivery && turn.lifecycle.status != "in_progress" {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(crate) async fn read_source_pending_at(
    rua_root: &Path,
    address: &ChatAddress,
) -> Result<Option<SourcePendingConversationInput>, String> {
    let path = source_pending_conversation_input_path(rua_root, address);
    match tokio::fs::read(&path).await {
        Ok(bytes) => {
            let pending: SourcePendingConversationInput =
                serde_json::from_slice(&bytes).map_err(|error| {
                    format!(
                        "read pending remote conversation input marker {}: {error}",
                        path.display()
                    )
                })?;
            if pending.address != *address {
                return Err(format!(
                    "pending remote conversation input marker {} belongs to {}, not {}",
                    path.display(),
                    pending.address,
                    address
                ));
            }
            Ok(Some(pending))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!(
            "read pending remote conversation input marker {}: {error}",
            path.display()
        )),
    }
}

fn source_pending_conversation_input_path(data_dir: &Path, address: &ChatAddress) -> PathBuf {
    ChatStore::resolve(data_dir, address.clone())
        .root()
        .join("device_commands")
        .join(SOURCE_PENDING_CONVERSATION_INPUTS_DIR)
        .join(format!(
            "{}.json",
            sanitize_local_marker_id(address.chat_id().as_str())
        ))
}

fn sanitize_local_marker_id(value: &str) -> String {
    value
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_') {
                byte as char
            } else {
                '_'
            }
        })
        .collect()
}
