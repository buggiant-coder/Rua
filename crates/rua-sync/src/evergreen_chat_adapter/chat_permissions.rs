use super::*;
use crate::opaque_replica_log::AuthorId;
use rua_security::approval_authority::StoredApprovalReuseRule;
use serde_json::Value;

pub(super) const RECORD: &str = "permission_rules";

pub(super) fn is_rule_record(change: &SyncChange) -> bool {
    change.domain == EVERGREEN_CHAT_DOMAIN && change.record_id.rsplit('/').next() == Some(RECORD)
}

/// History and rules are written by the Chat authority. Bind the claimed writer to the
/// iroh-docs entry signature before using that identity for placement checks.
pub fn validate_chat_record_author(change: &SyncChange, author: AuthorId) -> SyncResult<()> {
    let history = change.domain == EVERGREEN_CHAT_DOMAIN
        && parse_file_record_id(&change.record_id).is_ok_and(|(_, relative)| {
            history_authority::owns_record(relative)
                || matches!(relative, "session" | "session.json")
        });
    if (is_rule_record(change) || history)
        && change.device_id != hex::encode(&author.as_bytes()[..8])
    {
        return Err(SyncError::InvalidPayload(
            "Chat authority record writer differs from the signed entry author".into(),
        ));
    }
    Ok(())
}

fn placement(directory: &Path, address: &ChatAddress) -> SyncResult<(String, HlcTimestamp)> {
    let file = match std::fs::File::open(directory.join("session.json")) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(SyncError::PendingChatPlacement {
                chat_id: address.chat_id().to_string(),
            });
        }
        Err(error) => return Err(error.into()),
    };
    let header: Value = serde_json::from_reader(file)?;
    if header["chat_id"].as_str() != Some(address.chat_id().as_str()) {
        return Err(SyncError::InvalidPayload(
            "Chat permission rule header ID mismatch".into(),
        ));
    }
    let placement = &header["authority_placement"];
    let device = placement["device_id"].as_str().filter(|id| !id.is_empty());
    let epoch = placement["authority_epoch"]
        .as_str()
        .and_then(|value| HlcTimestamp::from_sortable_string(value).ok());
    device
        .zip(epoch)
        .map(|(id, epoch)| (id.to_owned(), epoch))
        .ok_or(SyncError::PendingChatPlacement {
            chat_id: address.chat_id().to_string(),
        })
}

pub(super) fn snapshot(
    root: &Path,
    address: &ChatAddress,
    local_device: &str,
) -> SyncResult<Option<SyncChange>> {
    let directory = root.join("sessions").join(address.chat_id().as_str());
    let path = directory.join("permission_rules.json");
    let file = match std::fs::File::open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let (authority, epoch) = match placement(&directory, address) {
        Ok(placement) => placement,
        Err(SyncError::PendingChatPlacement { .. }) => return Ok(None),
        Err(error) => return Err(error),
    };
    if authority != local_device {
        return Ok(None);
    }
    // Read the timestamp and the rules from the same revision of the atomically replaced file.
    let modified = file
        .metadata()?
        .modified()?
        .duration_since(UNIX_EPOCH)
        .map_err(|error| SyncError::Store(error.to_string()))?;
    let revision = HlcTimestamp {
        wall_ms: modified.as_millis() as u64,
        counter: modified.subsec_nanos() % 1_000_000,
        device_id: epoch.device_id,
    };
    let rules: Value = serde_json::from_reader(file)?;
    serde_json::from_value::<Vec<StoredApprovalReuseRule>>(rules.clone())?;
    Ok(Some(SyncChange::new(
        EvergreenChatSyncAdapter::new_op_id(),
        EVERGREEN_CHAT_DOMAIN,
        file_record_id(address, RECORD),
        EVERGREEN_CHAT_KIND_FILE_SNAPSHOT,
        local_device,
        revision,
        serde_json::json!({"chat_id":address.chat_id(), "authority_epoch":epoch.to_sortable_string(), "rules":rules}),
    )))
}

pub(super) fn apply(
    root: &Path,
    address: &ChatAddress,
    change: &SyncChange,
) -> SyncResult<ApplyOutcome> {
    let payload = &change.payload;
    if change.kind == EVERGREEN_CHAT_KIND_FILE_DELETE
        || payload["chat_id"].as_str() != Some(address.chat_id().as_str())
    {
        return Err(SyncError::InvalidPayload(
            "Expected scoped Chat permission rules, including an empty list for a cleared Chat"
                .into(),
        ));
    }
    let epoch = payload["authority_epoch"]
        .as_str()
        .and_then(|value| HlcTimestamp::from_sortable_string(value).ok())
        .ok_or_else(|| {
            SyncError::InvalidPayload("Invalid Chat permission rule authority epoch".into())
        })?;
    let directory = root.join("sessions").join(address.chat_id().as_str());
    let (authority, current_epoch) = placement(&directory, address)?;
    if epoch < current_epoch {
        return Ok(ApplyOutcome::Skipped);
    }
    if epoch > current_epoch {
        return Err(SyncError::PendingChatPlacement {
            chat_id: address.chat_id().to_string(),
        });
    }
    if change.device_id != authority {
        return Err(SyncError::InvalidPayload(
            "Chat permission rules came from a device other than the Chat authority".into(),
        ));
    }
    let rules = &payload["rules"];
    serde_json::from_value::<Vec<StoredApprovalReuseRule>>(rules.clone()).map_err(|error| {
        SyncError::InvalidPayload(format!("Invalid Chat permission rules: {error}"))
    })?;
    write_json_payload(&directory.join("permission_rules.json"), rules)?;
    Ok(ApplyOutcome::Applied {
        cursor: live_cursor(change.hlc, None),
    })
}
