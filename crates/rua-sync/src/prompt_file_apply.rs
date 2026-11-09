//! Shared merge and persistence rules for the two Prompt file domains.
use crate::hlc::HlcTimestamp;
use crate::merge::three_way_merge;
use crate::merge_store::{live_cursor, tombstone_cursor, MergeStore};
use crate::opaque_replica_log::SyncChange;
use crate::sidecar::LwwDecision;
use crate::sync_registry::{sync_cursor_record_id, ApplyOutcome};
use crate::{SyncError, SyncResult};
use rua_prompts::file_store::{PromptFileGuard, PromptFileVersion};
use std::path::Path;

pub(crate) fn apply_prompt_file(
    change: &SyncChange,
    merge_store: &MergeStore,
    guard: &PromptFileGuard,
    relative: &str,
    device_id: &str,
) -> SyncResult<ApplyOutcome> {
    if !matches!(change.kind.as_str(), "delete" | "upsert" | "snapshot") {
        return Err(SyncError::Store(format!(
            "unsupported prompt change kind: {}",
            change.kind
        )));
    }
    let path = guard.file_path(relative)?;
    let local_revision = guard.local_revision(relative)?;
    let local_fence = guard
        .deletion_fence(relative)?
        .map(|version| committed_hlc(version, device_id));
    if local_fence.is_some_and(|fence| change.hlc <= fence) {
        return Ok(ApplyOutcome::Skipped);
    }
    let cursor_id = sync_cursor_record_id(&change.domain, &change.record_id);
    let is_delete = change.kind == "delete";
    // Local mutations need not have been published or reflected in Sidecar.
    // Its old tombstone must not veto a newer delete after local recreation.
    let decision = match local_revision {
        Some((version, deleted)) if deleted || is_delete => {
            let local_hlc = committed_hlc(version, device_id);
            if change.hlc > local_hlc {
                LwwDecision::Apply
            } else {
                LwwDecision::Skip
            }
        }
        Some((_, false)) => LwwDecision::Apply,
        _ => merge_store.resolve_lww(&cursor_id, change.hlc, is_delete)?,
    };
    if decision == LwwDecision::Skip {
        return Ok(ApplyOutcome::Skipped);
    }

    if is_delete {
        guard.apply_delete(relative, prompt_version(change.hlc))?;
        return Ok(ApplyOutcome::Applied {
            cursor: tombstone_cursor(change.hlc, None),
        });
    }

    let remote = change
        .payload
        .get("content")
        .and_then(|value| value.as_str())
        .ok_or_else(|| SyncError::Store("prompt file change missing content".to_string()))?;

    let incoming_fence: Option<HlcTimestamp> = change
        .payload
        .get("deleted_before")
        .filter(|value| !value.is_null())
        .map(|value| {
            serde_json::from_value(value.clone()).map_err(|error| {
                SyncError::Store(format!("invalid prompt deletion fence: {error}"))
            })
        })
        .transpose()?;
    if incoming_fence.is_some_and(|fence| fence >= change.hlc) {
        return Err(SyncError::Store(
            "prompt deletion fence must precede live version".into(),
        ));
    }
    let deleted_before = local_fence.max(incoming_fence).max(
        local_revision
            .filter(|(_, deleted)| *deleted)
            .map(|(version, _)| committed_hlc(version, device_id)),
    );

    let current_cursor = merge_store.get_record_cursor(&cursor_id)?;
    let mut base = current_cursor
        .as_ref()
        .and_then(|entry| entry.base_blob_hash.as_ref())
        .and_then(|hash| merge_store.get_base_snapshot(hash).ok().flatten())
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .unwrap_or_default();
    let mut local = std::fs::read_to_string(&path).unwrap_or_default();
    let mut local_hlc = current_cursor
        .as_ref()
        .map(|entry| entry.hlc)
        .unwrap_or(HlcTimestamp {
            wall_ms: 0,
            counter: 0,
            device_id: [0; 8],
        });
    if let Some((ms, false)) = local_revision {
        local_hlc = local_hlc.max(committed_hlc(ms, device_id));
    }
    if let Some(fence) = deleted_before {
        // A recreation cannot merge bytes from before its deletion, even
        // when this Device never received the deletion itself.
        if local_revision.is_none() && path.is_file() {
            local_hlc = local_hlc.max(export_hlc(&path, device_id));
        }
        if local_hlc <= fence {
            local.clear();
        }
        if current_cursor
            .as_ref()
            .is_none_or(|cursor| cursor.hlc <= fence)
        {
            base.clear();
        }
    }
    let merge = three_way_merge(&base, &local, remote, local_hlc, change.hlc);

    guard.apply_write(
        relative,
        &merge.merged,
        prompt_version(change.hlc.max(local_hlc)),
        deleted_before.map(prompt_version),
    )?;
    let hash = merge_store.put_base_snapshot(merge.merged.as_bytes())?;
    Ok(ApplyOutcome::Applied {
        cursor: live_cursor(change.hlc, Some(hash)),
    })
}
pub(crate) fn live_payload(
    content: String,
    deleted_before: Option<PromptFileVersion>,
    device_id: &str,
) -> serde_json::Value {
    let mut payload = serde_json::json!({"content": content});
    if let Some(version) = deleted_before {
        payload["deleted_before"] = serde_json::json!(committed_hlc(version, device_id));
    }
    payload
}

pub(crate) fn committed_hlc(version: PromptFileVersion, device_id: &str) -> HlcTimestamp {
    let mut device = [0u8; 8];
    device.copy_from_slice(&blake3::hash(device_id.as_bytes()).as_bytes()[..8]);
    HlcTimestamp {
        wall_ms: version.modified_ms,
        counter: version.counter,
        device_id: version.device_id.unwrap_or(device),
    }
}

fn prompt_version(hlc: HlcTimestamp) -> PromptFileVersion {
    PromptFileVersion {
        modified_ms: hlc.wall_ms,
        counter: hlc.counter,
        device_id: Some(hlc.device_id),
    }
}

pub(crate) fn export_hlc(path: &Path, device_id: &str) -> HlcTimestamp {
    let ms = path
        .metadata()
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    committed_hlc(PromptFileVersion::local(ms), device_id)
}
