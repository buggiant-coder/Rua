use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use rua_channel::core::console::ConsoleChatRequest;
use rua_core::{ChatAddress, ChatScopeStore};
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
        let mut media_requirements = collect_remote_conversation_input_media_requirements(
            params.execution_agent_id,
            input.as_deref(),
        )?;
        if let Some(root) = params.workspace_dir {
            for media in &mut media_requirements {
                let path = root.join("media").join(&media.filename);
                if path.is_file() {
                    let (size, digest) = media_fingerprint(&path).await?;
                    media.byte_size = size;
                    media.sha256 = digest;
                    media.mime_type = crate::services::chat_media::ChatMediaService
                        .open(&self.data_dir, params.address.clone(), &media.filename)
                        .await
                        .map_err(|error| error.to_string())?
                        .mime_type;
                } else if params.materialize_conversation {
                    return Err(format!("staged attachment is missing: {}", media.media_ref));
                }
            }
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
        if params.materialize_conversation {
            for key in [
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
        self.transfer_staged_media(request).await?;
        let ack = self
            .deliver_remote_conversation_input_to_target(request)
            .await?;
        let outcome = RemoteConversationInputDeliveryOutcome::accepted(request, ack.clone());
        let scope = ChatScopeStore::resolve(&self.data_dir, request.address.clone());
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

    async fn transfer_staged_media(
        &self,
        request: &RemoteConversationInputRequest,
    ) -> Result<(), DeviceCommandDeliveryError> {
        use crate::services::sync_transfer::*;
        let first_submission = request
            .metadata
            .get("first_submission")
            .and_then(serde_json::Value::as_bool)
            == Some(true);
        let scope = ChatScopeStore::resolve(&self.data_dir, request.address.clone());
        for media in &request.media_requirements {
            let path = scope.root().join("media").join(&media.filename);
            if !path.is_file() {
                if first_submission {
                    return Err(DeviceCommandDeliveryError::Rejected {
                        reason: "staged_media_missing".into(),
                        detail: format!("Attachment unavailable: {}", media.media_ref),
                    });
                }
                continue;
            }
            let manager = current_session_manager(&self.device_network)
                .await
                .map_err(DeviceCommandDeliveryError::Connect)?;
            let operation_id = format!("input-media-{}", uuid::Uuid::new_v4());
            let prepared =
                crate::services::device_command_bus::deliver_chat_media_prepare_via_session(
                    &manager,
                    &crate::services::device_command_bus::ChatMediaPrepareCommand {
                        sha256: Some(media.sha256.clone()),
                        first_submission,
                        command_id: operation_id.clone(),
                        operation_id: operation_id.clone(),
                        address: request.address.clone(),
                        source_device_id: request.source_device_id.clone(),
                        target_device_id: request.target_device_id.clone(),
                        authority_epoch: request.authority_epoch,
                        media_id: media.filename.clone(),
                        byte_size: media.byte_size,
                        mime_type: media.mime_type.clone(),
                    },
                    DEVICE_COMMAND_DELIVERY_TIMEOUT,
                )
                .await?;
            if prepared.already_present {
                continue;
            }
            let (_control, receiver) = tokio::sync::watch::channel(rua_core::RunControl::Running);
            send_transfer_stream(
                &manager,
                TransferStreamSendRequest {
                    operation_id,
                    source_device_id: request.source_device_id.clone(),
                    target_device_id: request.target_device_id.clone(),
                    manifest: TransferManifest {
                        total_bytes: media.byte_size,
                        entries: vec![TransferManifestEntry {
                            source_path: path,
                            relative_path: PathBuf::from(&media.filename),
                            kind: TransferManifestEntryKind::File,
                            size: media.byte_size,
                        }],
                    },
                    target_dir: PathBuf::from("."),
                    conflict_policy: ReceiveConflictPolicy::Fail,
                    options: TransferExecutionOptions::default(),
                },
                receiver,
                |_| {},
            )
            .await
            .map_err(|error| DeviceCommandDeliveryError::Rejected {
                reason: "media_transfer_failed".into(),
                detail: error.to_string(),
            })?;
        }
        Ok(())
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
        let path = source_pending_conversation_input_path(&self.data_dir, address);
        match tokio::fs::read(&path).await {
            Ok(bytes) => {
                let pending: SourcePendingConversationInput = serde_json::from_slice(&bytes)
                    .map_err(|error| {
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
        let session_manager = current_session_manager(&self.device_network)
            .await
            .map_err(DeviceCommandDeliveryError::Connect)?;
        deliver_remote_conversation_input_via_session(
            &session_manager,
            request,
            DEVICE_COMMAND_DELIVERY_TIMEOUT,
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

pub(crate) async fn media_fingerprint(path: &Path) -> Result<(u64, String), String> {
    use sha2::{Digest, Sha256};
    use tokio::io::AsyncReadExt;
    let metadata = tokio::fs::symlink_metadata(path)
        .await
        .map_err(|error| error.to_string())?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("media must be a regular file".into());
    }
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|error| error.to_string())?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0; 64 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .await
            .map_err(|error| error.to_string())?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok((metadata.len(), format!("{:x}", digest.finalize())))
}

fn build_remote_conversation_input(request: &ConsoleChatRequest) -> Option<Vec<serde_json::Value>> {
    let input = request
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

    (!input.is_empty()).then_some(input)
}

fn collect_remote_conversation_input_media_requirements(
    agent_id: &str,
    input: Option<&[serde_json::Value]>,
) -> std::result::Result<Vec<RemoteConversationInputMediaRequirement>, String> {
    let Some(input) = input else {
        return Ok(Vec::new());
    };
    let mut requirements =
        std::collections::BTreeMap::<String, RemoteConversationInputMediaRequirement>::new();
    for message in input {
        let Some(content) = message
            .get("content")
            .and_then(|content| content.as_array())
        else {
            continue;
        };
        for part in content {
            if let Some(requirement) = remote_media_requirement_from_part(agent_id, part)? {
                requirements.insert(requirement.filename.clone(), requirement);
            }
        }
    }
    Ok(requirements.into_values().collect())
}

fn remote_media_requirement_from_part(
    agent_id: &str,
    part: &serde_json::Value,
) -> std::result::Result<Option<RemoteConversationInputMediaRequirement>, String> {
    let Some(part_type) = part.get("type").and_then(|value| value.as_str()) else {
        return Ok(None);
    };
    if !matches!(part_type, "image" | "file" | "audio" | "video") {
        return Ok(None);
    }
    let Some(media_ref) = part.get("media_ref").and_then(|value| value.as_str()) else {
        return Ok(None);
    };
    if !agent_types::content::is_managed_media_ref(media_ref) {
        return Err(format!("invalid managed media reference: {media_ref}"));
    }
    let filename = media_ref.trim_start_matches("media/").to_string();
    validate_media_filename_for_remote_input(&filename)?;
    Ok(Some(RemoteConversationInputMediaRequirement {
        mime_type: String::new(),
        byte_size: 0,
        sha256: String::new(),
        agent_id: agent_id.to_string(),
        media_ref: media_ref.to_string(),
        filename,
    }))
}

fn validate_media_filename_for_remote_input(filename: &str) -> std::result::Result<(), String> {
    if filename.is_empty()
        || filename.contains('/')
        || filename.contains('\\')
        || filename.contains("..")
        || filename == "."
        || filename == ".."
    {
        return Err(format!("invalid remote media filename: {filename}"));
    }
    Ok(())
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

fn source_pending_conversation_input_path(data_dir: &Path, address: &ChatAddress) -> PathBuf {
    ChatScopeStore::resolve(data_dir, address.clone())
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
