use std::collections::BTreeSet;
use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

use rua_core::{ChatAddress, ChatScopeStore, RunControl};
use rua_session::WorkspaceSessionStore;

use crate::services::chat_media::{ChatMediaService, MediaId, MAX_CHAT_MEDIA_BYTES};
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
use crate::services::sync_transfer::open_chat_media_read_stream;
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
    pub media_source_device_id: &'a str,
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

pub fn required_media_set(
    scope_root: &Path,
    address: &ChatAddress,
) -> Result<Vec<MediaId>, String> {
    let chat_dir = WorkspaceSessionStore::new(scope_root).chat_dir(address.chat_id().as_str());
    if !chat_dir.exists() {
        return Ok(Vec::new());
    }
    let mut media = BTreeSet::new();
    for entry in walkdir::WalkDir::new(chat_dir).follow_links(false) {
        let entry = entry.map_err(|error| error.to_string())?;
        if !entry.file_type().is_file() {
            continue;
        }
        let extension = entry.path().extension().and_then(|value| value.to_str());
        if !matches!(extension, Some("json" | "jsonl")) {
            continue;
        }
        let bytes = std::fs::read(entry.path()).map_err(|error| error.to_string())?;
        let values = serde_json::Deserializer::from_slice(&bytes).into_iter::<serde_json::Value>();
        for value in values {
            let value =
                value.map_err(|error| format!("parse stable Conversation State: {error}"))?;
            collect_media_refs(&value, &mut media)?;
        }
    }
    media
        .into_iter()
        .map(|value| {
            MediaId::from_str(&value).map_err(|_| format!("invalid Media Reference: media/{value}"))
        })
        .collect()
}

fn collect_media_refs(
    value: &serde_json::Value,
    media: &mut BTreeSet<String>,
) -> Result<(), String> {
    match value {
        serde_json::Value::Object(map) => {
            for (key, value) in map {
                if key == "media_ref" {
                    let media_ref = value
                        .as_str()
                        .ok_or_else(|| "Media Reference must be a string".to_string())?;
                    let media_id = media_ref
                        .strip_prefix("media/")
                        .ok_or_else(|| format!("invalid Media Reference: {media_ref}"))?;
                    uuid::Uuid::parse_str(media_id)
                        .map_err(|_| format!("invalid Media Reference: {media_ref}"))?;
                    media.insert(media_id.to_string());
                } else if key == "text" {
                    if let Some(text) = value.as_str() {
                        for reference in rua_session::display_media::managed_image_refs(text) {
                            let media_id = reference
                                .strip_prefix("media/")
                                .expect("managed image reference");
                            uuid::Uuid::parse_str(media_id)
                                .map_err(|_| format!("invalid Media Reference: {reference}"))?;
                            media.insert(media_id.to_string());
                        }
                    }
                } else {
                    collect_media_refs(value, media)?;
                }
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                collect_media_refs(value, media)?;
            }
        }
        _ => {}
    }
    Ok(())
}

async fn ensure_required_media_ready(
    request: &ConversationHistoryGapRequest<'_>,
    address: &ChatAddress,
) -> Result<(), String> {
    let scope = ChatScopeStore::resolve(request.data_dir, address.clone());
    let required = required_media_set(scope.root(), address)?;
    if required.is_empty() {
        return Ok(());
    }
    let session_manager = request
        .session_manager
        .ok_or_else(|| "target Device has no shared Device Session runtime".to_string())?;
    for media_id in required {
        if scope
            .root()
            .join("media")
            .join(media_id.to_string())
            .is_file()
        {
            continue;
        }
        let (mime_type, byte_size, mut reader) = open_chat_media_read_stream(
            session_manager,
            request.media_source_device_id,
            address.clone(),
            request.expected_authority_epoch,
            media_id.to_string(),
        )
        .await?;
        if byte_size == 0 || byte_size > MAX_CHAT_MEDIA_BYTES as u64 {
            return Err(format!("Required Media has invalid size: media/{media_id}"));
        }
        let mut bytes = Vec::with_capacity(byte_size as usize);
        tokio::io::AsyncReadExt::read_to_end(&mut reader, &mut bytes)
            .await
            .map_err(|error| error.to_string())?;
        if bytes.len() as u64 != byte_size {
            return Err(format!(
                "Required Media transfer was incomplete: media/{media_id}"
            ));
        }
        ChatMediaService
            .store_existing(
                request.data_dir,
                address.clone(),
                media_id,
                bytes,
                mime_type,
            )
            .await
            .map_err(|error| error.to_string())?;
    }
    let missing = required_media_set(scope.root(), address)?
        .into_iter()
        .filter(|media_id| {
            !scope
                .root()
                .join("media")
                .join(media_id.to_string())
                .is_file()
        })
        .collect::<Vec<_>>();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!("Required Media Set is incomplete: {missing:?}"))
    }
}

pub async fn verify_conversation_history_requirement(
    data_dir: &Path,
    address: &ChatAddress,
    requirement: &ConversationManifest,
) -> Result<(), String> {
    let scope_store = ChatScopeStore::resolve(data_dir, address.clone());
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
    let address = ChatAddress::agent(request.agent_id, request.chat_id);
    let workspace_dir = request.data_dir.join("workspaces").join(request.agent_id);
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
                ensure_required_media_ready(&request, &address).await?;
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
    ensure_required_media_ready(&request, &address).await?;
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
