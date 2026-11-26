use super::*;
use serde_json::Value;

pub(super) fn owns_record(relative: &str) -> bool {
    matches!(
        relative,
        "turn_index" | "turn_index.json" | "projection_exclusions" | "projection_exclusions.json"
    ) || ["messages/", "checkpoints/", "history_replacements/"]
        .iter()
        .any(|prefix| relative.starts_with(prefix))
}

pub(super) fn placement(
    root: &Path,
    address: &ChatAddress,
) -> SyncResult<Option<(String, HlcTimestamp)>> {
    let path = root
        .join("sessions")
        .join(address.chat_id().as_str())
        .join("session.json");
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let header: Value = serde_json::from_reader(file)?;
    header_placement(&header)
}

pub(super) fn header_placement(header: &Value) -> SyncResult<Option<(String, HlcTimestamp)>> {
    let Some(placement) = header.get("authority_placement") else {
        return Ok(None);
    };
    let device = placement["device_id"].as_str().filter(|id| !id.is_empty());
    let epoch = placement["authority_epoch"]
        .as_str()
        .and_then(|epoch| HlcTimestamp::from_sortable_string(epoch).ok());
    device
        .zip(epoch)
        .map(|(device, epoch)| Some((device.to_owned(), epoch)))
        .ok_or_else(|| SyncError::InvalidPayload("Invalid Chat authority placement".into()))
}

pub(super) fn wrap(content: Value, epoch: HlcTimestamp) -> Value {
    serde_json::json!({"authority_epoch":epoch.to_sortable_string(),"content":content})
}

pub(super) async fn apply(
    adapter: &EvergreenChatSyncAdapter,
    address: &ChatAddress,
    relative: &str,
    change: &SyncChange,
    merge: &MergeStore,
) -> SyncResult<ApplyOutcome> {
    let store = rua_session::WorkspaceSessionStore::new(adapter.scope_root(address));
    store
        .apply_replicated_chat_state(address.chat_id().as_str(), || {
            apply_locked(adapter, address, relative, change, merge)
        })
        .await
}

fn apply_locked(
    adapter: &EvergreenChatSyncAdapter,
    address: &ChatAddress,
    relative: &str,
    change: &SyncChange,
    merge: &MergeStore,
) -> SyncResult<ApplyOutcome> {
    let is_delete = change.kind == EVERGREEN_CHAT_KIND_FILE_DELETE;
    let path = adapter.path_for_record(address, relative)?;
    if rua_session::chat_deletion::ChatDeleteIntentStore::from_rua_root(&adapter.data_dir)
        .load(address)
        .map_err(SyncError::Store)?
        .is_some()
        || adapter.project_is_deleted(address, None)
    {
        if is_delete {
            match std::fs::remove_file(path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        return Ok(ApplyOutcome::Applied {
            cursor: tombstone_cursor(change.hlc, None),
        });
    }
    if is_delete
        && !(relative.starts_with("history_replacements/")
            && change.payload.get("authority_epoch").is_some())
    {
        return Err(SyncError::PendingChatPlacement {
            chat_id: address.chat_id().to_string(),
        });
    }
    if !is_delete
        && !matches!(
            change.kind.as_str(),
            EVERGREEN_CHAT_KIND_FILE_SNAPSHOT | EVERGREEN_CHAT_KIND_FILE_UPSERT
        )
    {
        return Err(SyncError::InvalidPayload(
            "Unsupported Chat history change kind".into(),
        ));
    }
    let Some((epoch, content)) = admitted_content(
        &adapter.scope_root(address),
        address,
        change,
        &adapter.device_id,
    )?
    else {
        return Ok(ApplyOutcome::Skipped);
    };
    let key = format!(
        "{}:authority:{}",
        sync_cursor_record_id(EVERGREEN_CHAT_DOMAIN, &change.record_id),
        epoch.to_sortable_string()
    );
    if merge.resolve_lww(&key, change.hlc, is_delete)? == LwwDecision::Skip {
        return Ok(ApplyOutcome::Skipped);
    }
    if !is_delete
        && !relative.starts_with("messages/")
        && merge
            .get_record_cursor(&key)?
            .is_some_and(|cursor| cursor.state == RecordState::Live && change.hlc <= cursor.hlc)
    {
        return Ok(ApplyOutcome::Skipped);
    }
    if is_delete {
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        return Ok(history_apply_outcome(change.hlc, true, Some(key)));
    }
    if let Some(record_id) = history_replacement_record_id(relative) {
        if rua_session::history_replacement_version_is_superseded(content) {
            return Ok(history_apply_outcome(change.hlc, false, Some(key)));
        }
        let record: rua_session::HistoryReplacementRecord = serde_json::from_value(content.clone())
            .map_err(|error| {
                SyncError::InvalidPayload(format!("Invalid history replacement: {error}"))
            })?;
        rua_session::validate_history_replacement_record(address, record_id, &record).map_err(
            |error| SyncError::InvalidPayload(format!("Invalid history replacement: {error}")),
        )?;
    }
    if relative.starts_with("messages/") {
        write_message_log_payload(&path, content)?;
    } else {
        write_json_payload(&path, content)?;
    }
    Ok(history_apply_outcome(change.hlc, false, Some(key)))
}

/// Return None only for a superseded writer. Future placement remains retryable
/// in the existing inbound inbox, without advancing its record cursor.
pub(super) fn admitted_content<'a>(
    root: &Path,
    address: &ChatAddress,
    change: &'a SyncChange,
    local_device: &str,
) -> SyncResult<Option<(HlcTimestamp, &'a Value)>> {
    let epoch = change.payload["authority_epoch"]
        .as_str()
        .and_then(|epoch| HlcTimestamp::from_sortable_string(epoch).ok())
        .ok_or_else(|| {
            SyncError::InvalidPayload("Chat history is missing its authority epoch".into())
        })?;
    let content = change
        .payload
        .get("content")
        .ok_or_else(|| SyncError::InvalidPayload("Chat history is missing its content".into()))?;
    let (authority, current_epoch) =
        placement(root, address)?.ok_or(SyncError::PendingChatPlacement {
            chat_id: address.chat_id().to_string(),
        })?;
    if epoch < current_epoch {
        return Ok(None);
    }
    if epoch > current_epoch {
        return Err(SyncError::PendingChatPlacement {
            chat_id: address.chat_id().to_string(),
        });
    }
    if change.device_id != authority {
        return Err(SyncError::InvalidPayload(
            "Chat history writer differs from its authority placement".into(),
        ));
    }
    if authority == local_device {
        return Ok(None);
    }
    Ok(Some((epoch, content)))
}

pub(super) fn runtime_header_epoch(
    local: Option<&Value>,
    incoming: &Value,
    writer: &str,
    local_device: &str,
) -> SyncResult<Option<HlcTimestamp>> {
    let incoming_placement = header_placement(incoming)?;
    let current_placement = local.map(header_placement).transpose()?.flatten();
    let authorized = incoming_placement
        .as_ref()
        .is_some_and(|(authority, epoch)| {
            authority == writer
                && current_placement
                    .as_ref()
                    .is_none_or(|(current, current_epoch)| {
                        epoch > current_epoch
                            || (epoch == current_epoch
                                && current == authority
                                && current != local_device)
                    })
        });
    Ok(incoming_placement
        .filter(|_| authorized)
        .map(|(_, epoch)| epoch))
}

pub(super) fn preserve_runtime_header(
    local: Option<&Value>,
    incoming: &mut Value,
    writer: &str,
    local_device: &str,
) -> rua_core::Result<()> {
    if runtime_header_epoch(local, incoming, writer, local_device)
        .map_err(|error| rua_core::RuaError::Validation {
            message: error.to_string(),
        })?
        .is_some()
    {
        return Ok(());
    }
    let Some(local) = local else {
        return Ok(());
    };
    if local.get("project_id").and_then(Value::as_str)
        != incoming.get("project_id").and_then(Value::as_str)
    {
        return Err(rua_core::RuaError::Validation {
            message: "Chat Project membership is immutable".into(),
        });
    }
    let mut merged = local.clone();
    overlay_listing(incoming, &mut merged);
    // Placement has its own epoch merge. A replica may relay a confirmed handoff,
    // but cannot relay its stale runtime header as a new runtime mutation.
    if let Some(placement) = incoming.get("authority_placement") {
        merged["authority_placement"] = placement.clone();
    }
    *incoming = merged;
    Ok(())
}

pub(super) fn overlay_listing(from: &Value, into: &mut Value) {
    for key in ["pin_order", "tag"] {
        if let Some(value) = from.get(key) {
            into[key] = value.clone();
        }
    }
    for key in ["title", "pinned", "archived", "last_activity_at"] {
        if let Some(value) = from.get("metadata").and_then(|metadata| metadata.get(key)) {
            into["metadata"][key] = value.clone();
        }
    }
}
