use std::path::Path;
use std::sync::Arc;

use rua_channel::core::console::ConsoleChatRequest;

use crate::services::acp_chat_registry::AcpChatRegistry;
use crate::services::conversation_authority_placement::{
    parse_conversation_authority_placement, ConversationAuthorityEpoch,
    ConversationAuthorityPlacement,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlacementTargetSource {
    ConversationAuthorityPlacement,
    DraftTarget,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacementTarget {
    pub device_id: String,
    pub authority_epoch: ConversationAuthorityEpoch,
    pub base_manifest_hash: String,
    pub source: PlacementTargetSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemotePlacementRoute {
    Local {
        authority_device_id: String,
        authority_epoch: ConversationAuthorityEpoch,
    },
    Remote {
        target_device_id: String,
        source_device_id: String,
        authority_epoch: ConversationAuthorityEpoch,
        source: PlacementTargetSource,
    },
    MissingSourceDevice {
        target_device_id: String,
        source: PlacementTargetSource,
    },
}

pub async fn resolve_submit_target(
    workspace_dir: Option<&Path>,
    chat_id: &str,
    request: &ConsoleChatRequest,
    agent_id: &str,
    local_device_id: Option<&str>,
    acp_chat_registry: Option<&Arc<AcpChatRegistry>>,
) -> std::result::Result<Option<PlacementTarget>, String> {
    let _ = acp_chat_registry;
    let workspace_dir =
        workspace_dir.ok_or_else(|| "conversation workspace is unavailable".to_string())?;
    validate_chat_submit_materialization(
        workspace_dir,
        chat_id,
        request
            .extra
            .get("expected_authority_epoch")
            .and_then(serde_json::Value::as_str),
    )
    .await?;
    let target = match load_optional_chat_authority_placement(workspace_dir, chat_id).await? {
        Some(placement) => PlacementTarget {
            device_id: placement.device_id,
            authority_epoch: placement.authority_epoch,
            base_manifest_hash: placement.base_manifest_hash,
            source: PlacementTargetSource::ConversationAuthorityPlacement,
        },
        None => {
            let device_id = request
                .extra
                .get("target_device_id")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .or_else(|| local_device_id.map(str::to_string))
                .ok_or_else(|| "draft target Device is unavailable".to_string())?;
            let address = rua_core::ChatAddress::agent(agent_id, chat_id);
            PlacementTarget {
                authority_epoch: request
                    .extra
                    .get("draft_authority_epoch")
                    .and_then(serde_json::Value::as_str)
                    .map(ConversationAuthorityEpoch::parse)
                    .transpose()
                    .map_err(|error| error.to_string())?
                    .unwrap_or_else(|| {
                        ConversationAuthorityEpoch::next_for_device(&device_id, None)
                    }),
                base_manifest_hash: request
                    .extra
                    .get("draft_base_manifest_hash")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
                    .unwrap_or(
                        crate::services::conversation_manifest::initial_conversation_manifest_hash(
                            &address,
                        )
                        .map_err(|error| error.to_string())?,
                    ),
                device_id,
                source: PlacementTargetSource::DraftTarget,
            }
        }
    };
    if let Some(expected_epoch) = request
        .extra
        .get("expected_authority_epoch")
        .and_then(serde_json::Value::as_str)
    {
        if expected_epoch != target.authority_epoch.to_string() {
            return Err(format!(
                "Conversation Authority epoch changed from '{expected_epoch}' to '{}'",
                target.authority_epoch
            ));
        }
    }
    if local_device_id.is_some_and(|device_id| device_id == target.device_id) {
        for attachment in &request.attachments {
            let media_id = attachment
                .stored_name
                .as_deref()
                .ok_or_else(|| "Chat attachment is missing its Media id".to_string())?;
            let media_id = uuid::Uuid::parse_str(media_id)
                .map_err(|_| "Chat attachment Media id must be a UUID".to_string())?;
            let path = workspace_dir.join("media").join(media_id.to_string());
            if !path.is_file() {
                return Err(format!(
                    "required Chat Media is unavailable: media/{media_id}"
                ));
            }
        }
    }
    Ok(Some(target))
}

pub async fn resolve_acp_route(
    workspace_dir: &Path,
    chat_id: &str,
    explicit_target_device_id: Option<&str>,
    local_device_id: Option<&str>,
) -> std::result::Result<RemotePlacementRoute, String> {
    let target = load_chat_authority_placement_target(workspace_dir, chat_id).await?;
    if let Some(explicit_target) = explicit_target_device_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if explicit_target != target.device_id {
            return Err(format!(
                "requested ACP device '{explicit_target}' is not the conversation authority '{}'",
                target.device_id
            ));
        }
    }
    Ok(route_for_local_device(target, local_device_id))
}

pub fn route_for_local_device(
    target: PlacementTarget,
    local_device_id: Option<&str>,
) -> RemotePlacementRoute {
    if local_device_id == Some(target.device_id.as_str()) {
        return RemotePlacementRoute::Local {
            authority_device_id: target.device_id,
            authority_epoch: target.authority_epoch,
        };
    }
    let Some(source_device_id) = local_device_id else {
        return RemotePlacementRoute::MissingSourceDevice {
            target_device_id: target.device_id,
            source: target.source,
        };
    };
    RemotePlacementRoute::Remote {
        target_device_id: target.device_id,
        source_device_id: source_device_id.to_string(),
        authority_epoch: target.authority_epoch,
        source: target.source,
    }
}

pub async fn chat_execution_runtime_metadata(
    workspace_dir: Option<&Path>,
    chat_id: &str,
    request_extra: &serde_json::Map<String, serde_json::Value>,
) -> serde_json::Value {
    let _ = request_extra;
    match workspace_dir {
        Some(workspace_dir) => {
            match load_chat_authority_placement_target(workspace_dir, chat_id).await {
                Ok(target) => serde_json::json!({
                    "mode": "explicit_authority",
                    "target_device_id": target.device_id,
                    "authority_epoch": target.authority_epoch,
                }),
                Err(_) => serde_json::json!({
                    "mode": "invalid",
                    "target_device_id": serde_json::Value::Null,
                    "authority_epoch": serde_json::Value::Null,
                }),
            }
        }
        None => serde_json::json!({
            "mode": "invalid",
            "target_device_id": serde_json::Value::Null,
            "authority_epoch": serde_json::Value::Null,
        }),
    }
}

pub async fn load_chat_authority_placement_target(
    workspace_dir: &Path,
    chat_id: &str,
) -> std::result::Result<PlacementTarget, String> {
    let placement = load_optional_chat_authority_placement(workspace_dir, chat_id)
        .await?
        .ok_or_else(|| {
            format!("conversation authority placement not found for Chat '{chat_id}'")
        })?;
    Ok(PlacementTarget {
        device_id: placement.device_id,
        authority_epoch: placement.authority_epoch,
        base_manifest_hash: placement.base_manifest_hash,
        source: PlacementTargetSource::ConversationAuthorityPlacement,
    })
}

pub async fn load_optional_chat_authority_placement(
    workspace_dir: &Path,
    chat_id: &str,
) -> std::result::Result<Option<ConversationAuthorityPlacement>, String> {
    let Some(meta) = load_chat_control_meta(workspace_dir, chat_id).await? else {
        return Ok(None);
    };
    if meta.get("authority_placement").is_none() {
        return Ok(None);
    }
    parse_conversation_authority_placement(&meta)
        .map(Some)
        .map_err(|error| error.to_string())
}

pub async fn validate_chat_submit_materialization(
    workspace_dir: &Path,
    chat_id: &str,
    expected_epoch: Option<&str>,
) -> std::result::Result<(), String> {
    if let Some(placement) = load_optional_chat_authority_placement(workspace_dir, chat_id).await? {
        if let Some(expected) = expected_epoch {
            if expected != placement.authority_epoch.to_string() {
                return Err(format!(
                    "Conversation Authority epoch changed from '{expected}' to '{}'",
                    placement.authority_epoch
                ));
            }
        }
        return Ok(());
    }
    let store = rua_session::WorkspaceSessionStore::new(workspace_dir);
    let chat_dir = store.chat_dir(chat_id);
    let has_state = store
        .load_chat_header(chat_id)
        .await
        .map_err(|e| e.to_string())?
        .is_some()
        || crate::services::chat_index::get(workspace_dir, chat_id)
            .await
            .map_err(|e| e.to_string())?
            .is_some()
        || store.configuration_head_path(chat_id).exists()
        || chat_dir.join("configurations").exists();
    if expected_epoch.is_some() || has_state {
        return Err("Chat state is not ready: Conversation Authority Placement has not arrived; retry after synchronization".to_string());
    }
    Ok(())
}

pub async fn load_chat_control_meta(
    workspace_dir: &Path,
    chat_id: &str,
) -> std::result::Result<Option<serde_json::Value>, String> {
    rua_session::WorkspaceSessionStore::new(workspace_dir)
        .load_chat_header(chat_id)
        .await
        .map(|header| {
            header.map(|header| {
                let model = rua_chat::model_slot_from_header(&header);
                let mut extra = header.extra;
                match model {
                    Some(model) => {
                        extra.insert("model".to_string(), model);
                    }
                    None => {
                        extra.remove("model");
                    }
                }
                serde_json::Value::Object(extra)
            })
        })
        .map_err(|error| error.to_string())
}

pub async fn load_chat_authority_placement_device(
    workspace_dir: &Path,
    chat_id: &str,
) -> std::result::Result<Option<String>, String> {
    Ok(Some(
        load_chat_authority_placement_target(workspace_dir, chat_id)
            .await?
            .device_id,
    ))
}
