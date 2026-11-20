use std::collections::BTreeSet;
use std::path::Path;
use std::time::Duration;

use rua_core::ChatAddress;
use rua_device_network::DeviceSessionManager;
use rua_session::chat_deletion::ChatDeleteIntentStore;
use rua_session::chat_media::ChatMediaManifest;
use uuid::Uuid;

use super::{media_dir, ChatMediaError, ChatMediaService, OpenedChatMedia};
use crate::services::conversation_authority_target::load_optional_chat_authority_placement;
use crate::services::remote_conversation_input::read_source_pending_at;
use crate::services::sync_transfer::{read_chat_media_bytes, request_chat_media_manifest};

const MEDIA_SOURCE_TIMEOUT: Duration = Duration::from_secs(30);

pub(crate) async fn local_chat_media_manifest(
    rua_root: &Path,
    address: &ChatAddress,
    media_id: &str,
) -> Result<ChatMediaManifest, ChatMediaError> {
    let id = Uuid::parse_str(media_id).map_err(|_| ChatMediaError::InvalidId)?;
    if ChatDeleteIntentStore::from_rua_root(rua_root)
        .load(address)
        .map_err(std::io::Error::other)?
        .is_some()
    {
        return Err(ChatMediaError::NotFound);
    }
    let dir = media_dir(rua_root);
    crate::services::first_chat_submission::validate_staged_media(
        &dir,
        &id.to_string(),
        address,
        None,
    )
    .await
    .map_err(|_| ChatMediaError::NotFound)?;
    let chat_id = address.chat_id().as_str();
    let scope = rua_core::ChatStore::resolve(rua_root, address.clone());
    let chat_dir = rua_session::WorkspaceSessionStore::new(scope.root()).chat_dir(chat_id);
    if let Some(reference) =
        ChatMediaManifest::load_reference(&chat_dir, id)?.filter(|m| m.chat_ids.contains(chat_id))
    {
        reference.register(&dir)?;
    }
    let mut manifest = ChatMediaManifest::load(&dir, id)?
        .filter(|manifest| manifest.chat_ids.contains(chat_id))
        .ok_or(ChatMediaError::NotFound)?;
    // This request proves only the association that the peer asked about.
    manifest
        .chat_ids
        .retain(|chat_id| chat_id == address.chat_id().as_str());
    Ok(manifest)
}

pub async fn chat_media_manifest_from_device(
    manager: &DeviceSessionManager,
    device_id: &str,
    address: &ChatAddress,
    media_id: &str,
) -> Result<ChatMediaManifest, ChatMediaError> {
    peer_manifest(manager, device_id, address, media_id, None).await
}

pub async fn chat_media_manifest_from_device_for_submission(
    manager: &DeviceSessionManager,
    device_id: &str,
    address: &ChatAddress,
    media_id: &str,
    project_id: Option<&str>,
) -> Result<ChatMediaManifest, ChatMediaError> {
    peer_manifest(manager, device_id, address, media_id, Some(project_id)).await
}

async fn peer_manifest(
    manager: &DeviceSessionManager,
    device_id: &str,
    address: &ChatAddress,
    media_id: &str,
    first_submission_project: Option<Option<&str>>,
) -> Result<ChatMediaManifest, ChatMediaError> {
    let id = Uuid::parse_str(media_id).map_err(|_| ChatMediaError::InvalidId)?;
    let mut manifest = tokio::time::timeout(
        MEDIA_SOURCE_TIMEOUT,
        request_chat_media_manifest(
            manager,
            device_id,
            address,
            media_id,
            first_submission_project,
        ),
    )
    .await
    .map_err(|_| ChatMediaError::Unavailable("source did not respond".into()))?
    .map_err(ChatMediaError::Unavailable)?
    .ok_or(ChatMediaError::NotFound)?;
    if manifest
        .id()
        .map_err(|e| ChatMediaError::Unavailable(e.to_string()))?
        != id
        || !manifest.chat_ids.contains(address.chat_id().as_str())
    {
        return Err(ChatMediaError::Unavailable(
            "source returned mismatched Chat Media metadata".into(),
        ));
    }
    manifest
        .chat_ids
        .retain(|chat_id| chat_id == address.chat_id().as_str());
    Ok(manifest)
}

async fn authority_device(
    rua_root: &Path,
    address: &ChatAddress,
) -> Result<Option<String>, ChatMediaError> {
    let scope = rua_core::ChatStore::resolve(rua_root, address.clone());
    if let Some(placement) =
        load_optional_chat_authority_placement(scope.root(), address.chat_id().as_str())
            .await
            .map_err(std::io::Error::other)?
    {
        return Ok(Some(placement.device_id));
    }
    Ok(read_source_pending_at(rua_root, address)
        .await
        .map_err(std::io::Error::other)?
        .map(|p| p.target_device_id))
}

pub async fn chat_media_manifest(
    rua_root: &Path,
    address: &ChatAddress,
    media_id: &str,
    local_device_id: &str,
    manager: Option<&DeviceSessionManager>,
) -> Result<ChatMediaManifest, ChatMediaError> {
    match local_chat_media_manifest(rua_root, address, media_id).await {
        Err(ChatMediaError::NotFound) => {}
        result => return result,
    }
    if ChatDeleteIntentStore::from_rua_root(rua_root)
        .load(address)
        .map_err(std::io::Error::other)?
        .is_some()
    {
        return Err(ChatMediaError::NotFound);
    }
    let device = authority_device(rua_root, address)
        .await?
        .filter(|device| device != local_device_id)
        .ok_or(ChatMediaError::NotFound)?;
    let manager = manager.ok_or_else(|| {
        ChatMediaError::Unavailable("Device Session manager is unavailable".into())
    })?;
    let manifest = chat_media_manifest_from_device(manager, &device, address, media_id).await?;
    manifest.register(&media_dir(rua_root))?;
    Ok(manifest)
}

pub async fn resolve_chat_media(
    rua_root: &Path,
    address: &ChatAddress,
    media_id: &str,
    local_device_id: &str,
    manager: Option<&DeviceSessionManager>,
) -> Result<OpenedChatMedia, ChatMediaError> {
    let mut manifest =
        chat_media_manifest(rua_root, address, media_id, local_device_id, manager).await?;
    let mut detail = match ChatMediaService
        .open(rua_root, address.clone(), media_id)
        .await
    {
        Ok(opened) => return Ok(opened),
        Err(ChatMediaError::Unavailable(detail)) => detail,
        Err(error) => return Err(error),
    };
    let mut attempted = BTreeSet::from([local_device_id.to_owned()]);
    let mut refreshed = false;
    if let Some(manager) = manager {
        loop {
            let devices: Vec<_> = manifest
                .device_ids
                .difference(&attempted)
                .cloned()
                .collect();
            for device in devices {
                attempted.insert(device.clone());
                let bytes = match tokio::time::timeout(
                    MEDIA_SOURCE_TIMEOUT,
                    read_chat_media_bytes(manager, &device, address, media_id, &manifest),
                )
                .await
                {
                    Ok(Ok(bytes)) => bytes,
                    Ok(Err(error)) => {
                        detail = error;
                        continue;
                    }
                    Err(_) => {
                        detail = "source did not respond".into();
                        continue;
                    }
                };
                let mut cached = manifest.clone();
                cached.device_ids.insert(local_device_id.to_owned());
                let dir = media_dir(rua_root);
                match tokio::task::spawn_blocking(move || cached.store_copy(&dir, &bytes))
                    .await
                    .map_err(std::io::Error::other)?
                {
                    Ok(()) => {
                        return ChatMediaService
                            .open(rua_root, address.clone(), media_id)
                            .await
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::InvalidData => detail = e.to_string(),
                    Err(e) => return Err(e.into()),
                }
            }
            if refreshed {
                break;
            }
            refreshed = true;
            let Some(device) = authority_device(rua_root, address)
                .await?
                .filter(|id| id != local_device_id)
            else {
                break;
            };
            if let Ok(latest) =
                chat_media_manifest_from_device(manager, &device, address, media_id).await
            {
                latest.register(&media_dir(rua_root))?;
                manifest.device_ids.extend(latest.device_ids);
            }
        }
    } else if manifest
        .device_ids
        .iter()
        .any(|device| device != local_device_id)
    {
        detail = format!("Device Session manager is unavailable; local copy: {detail}");
    }
    Err(ChatMediaError::Unavailable(format!(
        "no verified copy of {} is available: {detail}",
        manifest.media_ref
    )))
}
