use std::path::Path;
use std::time::Duration;

use rua_core::{ChatAddress, ChatStore, RunControl};
use rua_session::WorkspaceSessionStore;

use crate::services::chat_metadata_store::{
    load_chat_record, stage_synced_chat_metadata, ChatMetadataRecord,
};
use crate::services::conversation_authority_placement::{
    parse_conversation_authority_placement, ConversationAuthorityEpoch,
};
use crate::services::conversation_manifest::{
    load_conversation_manifest, ConversationManifest, ConversationManifestMismatch,
};
use crate::services::device_command_bus::{SyncTransferDeviceAction, SyncTransferDeviceCommand};
use crate::services::sync_transfer::{build_transfer_inventory, command_remote_chat_gap_to};

const HISTORY_SYNC_ATTEMPTS: usize = 3;
const HISTORY_SYNC_TIMEOUT: Duration = Duration::from_secs(10);

pub struct ConversationHistorySyncRequest {
    pub source_device_id: String,
    pub result_tx: tokio::sync::oneshot::Sender<Result<(), String>>,
}

pub struct ConversationHistoryReadinessRequest<'a> {
    pub data_dir: &'a Path,
    pub address: &'a ChatAddress,
    pub source_device_id: &'a str,
    pub requirement: &'a ConversationManifest,
    pub sync_tx: Option<&'a tokio::sync::mpsc::UnboundedSender<ConversationHistorySyncRequest>>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConversationHistoryReadinessError {
    #[error("conversation history sync runtime is unavailable")]
    SyncUnavailable,
    #[error("conversation history apply failed: {detail}")]
    ApplyFailed { detail: String },
    #[error("conversation history sync runtime dropped the result")]
    SyncResultDropped,
    #[error("conversation history sync timed out after {attempts} attempts")]
    Timeout { attempts: usize },
    #[error("conversation history requirement not met: {detail}")]
    RequirementNotMet { detail: String },
}

impl ConversationHistoryReadinessError {
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::SyncUnavailable => "sync_unavailable",
            Self::ApplyFailed { .. } => "apply_failed",
            Self::SyncResultDropped => "sync_result_dropped",
            Self::Timeout { .. } => "timeout",
            Self::RequirementNotMet { .. } => "requirement_not_met",
        }
    }
}

pub struct ConversationHistoryGapRequest<'a> {
    pub data_dir: &'a Path,
    pub agent_id: &'a str,
    pub chat_id: &'a str,
    pub source_device_id: &'a str,
    pub target_device_id: &'a str,
    pub expected_authority_epoch: ConversationAuthorityEpoch,
    pub requirement: &'a ConversationManifest,
    pub session_manager: Option<&'a rua_device_network::DeviceSessionManager>,
}

pub async fn build_conversation_history_requirement(
    scope_root: &Path,
    address: &ChatAddress,
) -> Result<ConversationManifest, String> {
    load_conversation_manifest(scope_root, address)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "conversation manifest is unavailable".to_string())
}

pub async fn verify_conversation_history_requirement(
    data_dir: &Path,
    address: &ChatAddress,
    requirement: &ConversationManifest,
) -> Result<(), String> {
    let scope_store = ChatStore::resolve(data_dir, address.clone());
    let actual = load_conversation_manifest(scope_store.root(), address)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "target conversation manifest is unavailable".to_string())?;
    let comparison = actual.compare(requirement);
    if comparison.matches {
        return Ok(());
    }
    Err(manifest_mismatch_detail(
        &actual,
        requirement,
        &comparison.mismatches,
    ))
}

pub async fn ensure_conversation_history_ready(
    request: ConversationHistoryReadinessRequest<'_>,
) -> Result<(), ConversationHistoryReadinessError> {
    if verify_conversation_history_requirement(
        request.data_dir,
        request.address,
        request.requirement,
    )
    .await
    .is_ok()
    {
        return Ok(());
    }

    let mut last_error = None;
    for attempt in 1..=HISTORY_SYNC_ATTEMPTS {
        let Some(tx) = request.sync_tx else {
            return Err(ConversationHistoryReadinessError::SyncUnavailable);
        };
        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        if tx
            .send(ConversationHistorySyncRequest {
                source_device_id: request.source_device_id.to_string(),
                result_tx,
            })
            .is_err()
        {
            return Err(ConversationHistoryReadinessError::SyncUnavailable);
        }
        match tokio::time::timeout(HISTORY_SYNC_TIMEOUT, result_rx).await {
            Ok(Ok(Ok(()))) => {}
            Ok(Ok(Err(detail))) => {
                last_error = Some(ConversationHistoryReadinessError::ApplyFailed { detail });
            }
            Ok(Err(_closed)) => {
                return Err(ConversationHistoryReadinessError::SyncResultDropped);
            }
            Err(_elapsed) => {
                last_error = Some(ConversationHistoryReadinessError::Timeout { attempts: attempt });
            }
        }
        match verify_conversation_history_requirement(
            request.data_dir,
            request.address,
            request.requirement,
        )
        .await
        {
            Ok(()) => return Ok(()),
            Err(detail) if last_error.is_none() => {
                last_error = Some(ConversationHistoryReadinessError::RequirementNotMet { detail });
            }
            Err(_) => {}
        }
    }

    Err(
        last_error.unwrap_or_else(|| ConversationHistoryReadinessError::RequirementNotMet {
            detail: "conversation history requirement did not match".to_string(),
        }),
    )
}

pub async fn ensure_conversation_history_ready_via_device_session(
    request: ConversationHistoryGapRequest<'_>,
) -> Result<ChatMetadataRecord, String> {
    let address = ChatAddress::new(request.chat_id);
    let workspace_dir = rua_core::ChatStore::resolve(request.data_dir, address.clone())
        .root()
        .to_path_buf();
    let store = WorkspaceSessionStore::new(&workspace_dir);
    tokio::fs::create_dir_all(store.sessions_dir())
        .await
        .map_err(|error| format!("create target sessions directory: {error}"))?;
    let _chat_guard = store.lock_chat_storage(request.chat_id).await;

    let local_manifest = load_conversation_manifest(&workspace_dir, &address)
        .await
        .map_err(|error| error.to_string())?;
    reject_ahead_or_divergent_target(local_manifest.as_ref(), request.requirement)?;

    let local_chat = load_chat_record(&workspace_dir, request.chat_id)
        .await
        .map_err(|error| error.to_string())?;
    if let Some(chat) = local_chat.as_ref() {
        let placement =
            crate::services::conversation_authority_target::load_optional_chat_authority_placement(
                &workspace_dir,
                request.chat_id,
            )
            .await?;
        if let Some(placement) = placement {
            if placement.authority_epoch > request.expected_authority_epoch {
                return Err(format!(
                    "target already knows newer authority Epoch {}",
                    placement.authority_epoch
                ));
            }
            if placement.authority_epoch == request.expected_authority_epoch
                && local_manifest
                    .as_ref()
                    .is_some_and(|manifest| manifest.compare(request.requirement).matches)
            {
                return Ok(chat.clone());
            }
        }
    }

    let target_inventory = if store.chat_dir(request.chat_id).exists() {
        build_transfer_inventory(&[store.chat_dir(request.chat_id)])
            .map_err(|error| error.to_string())?
    } else {
        Vec::new()
    };
    let session_manager = request
        .session_manager
        .ok_or_else(|| "target Device has no shared Device Session runtime".to_string())?;
    let operation_id = format!("conversation-history-gap-{}", uuid::Uuid::new_v4());
    let command = SyncTransferDeviceCommand {
        command_id: format!("conversation-history-gap-command-{}", uuid::Uuid::new_v4()),
        source_device_id: request.target_device_id.to_string(),
        target_device_id: request.source_device_id.to_string(),
        action: SyncTransferDeviceAction::StartChatGapSend {
            operation_id,
            agent_id: request.agent_id.to_string(),
            chat_id: request.chat_id.to_string(),
            expected_authority_epoch: request.expected_authority_epoch,
            required_manifest: Box::new(request.requirement.clone()),
            transfer_target_device_id: request.target_device_id.to_string(),
            target_dir: store.sessions_dir().to_string_lossy().into_owned(),
            target_inventory,
        },
    };
    let (_cancel_tx, cancel_rx) = tokio::sync::watch::channel(RunControl::Running);
    let (_, source_chat) = command_remote_chat_gap_to(session_manager, command, cancel_rx)
        .await
        .map_err(|error| error.to_string())?;
    if source_chat.id != request.chat_id {
        return Err("source Chat metadata targets a different Chat".to_string());
    }
    let source_placement = parse_conversation_authority_placement(&source_chat.meta)
        .map_err(|error| error.to_string())?;
    if source_placement.authority_epoch != request.expected_authority_epoch {
        return Err(format!(
            "source Chat metadata has authority Epoch {}, expected {}",
            source_placement.authority_epoch, request.expected_authority_epoch
        ));
    }

    verify_conversation_history_requirement(request.data_dir, &address, request.requirement)
        .await?;
    stage_synced_chat_metadata(&workspace_dir, &source_chat)
        .await
        .map_err(|error| error.to_string())?;
    Ok(source_chat)
}

fn reject_ahead_or_divergent_target(
    actual: Option<&ConversationManifest>,
    required: &ConversationManifest,
) -> Result<(), String> {
    let Some(actual) = actual else {
        return Ok(());
    };
    if actual.address != required.address {
        return Err("target conversation identity differs from source".to_string());
    }
    if actual.latest_turn > required.latest_turn
        || actual.latest_sequence > required.latest_sequence
        || actual.turn_count > required.turn_count
        || actual.message_count > required.message_count
    {
        return Err("target conversation is ahead of the requested switch boundary".to_string());
    }
    if actual.latest_turn == required.latest_turn
        && actual.latest_sequence == required.latest_sequence
        && actual.turn_count == required.turn_count
        && actual.message_count == required.message_count
        && !actual.compare(required).matches
    {
        return Err("target conversation boundary conflicts with source".to_string());
    }
    Ok(())
}

fn manifest_mismatch_detail(
    actual: &ConversationManifest,
    required: &ConversationManifest,
    mismatches: &[ConversationManifestMismatch],
) -> String {
    if actual.latest_sequence < required.latest_sequence {
        return format!(
            "target is behind source: sequence {} < {}",
            actual.latest_sequence, required.latest_sequence
        );
    }
    if actual.latest_sequence > required.latest_sequence {
        return format!(
            "target is ahead of source: sequence {} > {}",
            actual.latest_sequence, required.latest_sequence
        );
    }
    format!("target conversation boundary differs from source: {mismatches:?}")
}
