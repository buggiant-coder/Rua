//! Readiness for logical Chat state. This reads synchronized records only;
//! neither work directories nor media bytes participate in a handoff.

use std::collections::BTreeMap;
use std::path::Path;

use rua_session::collaboration::CollaborationWakeState;
use rua_session::WorkspaceSessionStore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct HandoffHistorySnapshot {
    files: BTreeMap<String, Option<String>>,
    header: Value,
    pub wake_state: CollaborationWakeState,
    #[serde(default, flatten)]
    extra: BTreeMap<String, Value>,
}

impl HandoffHistorySnapshot {
    pub fn contains(&self, expected: &Self) -> bool {
        self.header == expected.header
            && expected.files.iter().all(|(path, digest)| {
                self.files.get(path).and_then(Option::as_ref) == digest.as_ref()
            })
    }
}

pub(super) async fn capture(
    root: &Path,
    chat_id: &str,
    candidate_epoch: &str,
) -> Result<HandoffHistorySnapshot, String> {
    let store = WorkspaceSessionStore::new(root);
    let header = store
        .load_chat_header(chat_id)
        .await
        .map_err(|error| error.to_string())?
        .ok_or("Chat history header unavailable")?;
    let mut files = BTreeMap::from([
        ("configuration_head".into(), None),
        ("projection_exclusions".into(), None),
        ("turn_index".into(), None),
        ("prompt_state".into(), None),
    ]);
    let candidate = format!("configurations/{candidate_epoch}.json");
    // The ordinary sync exporter tolerates malformed records so one broken
    // Chat cannot stop all synchronization. Handoff readiness must distinguish
    // absent records from existing records the next Turn cannot read.
    validate_readable_records(root, chat_id)?;
    for snapshot in
        rua_sync::evergreen_chat_adapter::collect_evergreen_chat_file_snapshots(root, chat_id, "")
    {
        if snapshot.relative == "session"
            || snapshot.relative == candidate
            || snapshot.relative.starts_with("task_refs/")
        {
            continue;
        }
        let digest = if snapshot.is_delete {
            None
        } else {
            let mut payload = snapshot.payload;
            if snapshot.relative.starts_with("messages/") {
                let content = payload.as_str().ok_or("Invalid Chat message chunk")?;
                payload = Value::Array(
                    content
                        .lines()
                        .filter(|line| !line.trim().is_empty())
                        .map(serde_json::from_str)
                        .collect::<Result<Vec<Value>, _>>()
                        .map_err(|error| error.to_string())?,
                );
            }
            if snapshot.relative.starts_with("media_refs/") {
                // Known copies may grow while Chat is idle. The immutable
                // identity and origin are required, not identical copy lists.
                if let Some(object) = payload.as_object_mut() {
                    object.remove("device_ids");
                }
            }
            sort_objects(&mut payload);
            Some(hex::encode(Sha256::digest(
                serde_json::to_vec(&payload).map_err(|error| error.to_string())?,
            )))
        };
        files.insert(snapshot.relative, digest);
    }
    if files.get("configuration_head").is_none_or(Option::is_none) {
        return Err("Chat configuration head unavailable".into());
    }
    Ok(HandoffHistorySnapshot {
        files,
        header: json!({
            "active_checkpoint_id": header.active_checkpoint_id,
            "message_log": header.message_log,
            "turn_index": header.turn_index,
            "projection_boundary": header.projection_boundary,
            "context_snapshot": header.context_snapshot,
        }),
        wake_state: store
            .load_collaboration_wake_state(chat_id)
            .await
            .map_err(|error| error.to_string())?,
        extra: BTreeMap::new(),
    })
}

fn validate_readable_records(root: &Path, chat_id: &str) -> Result<(), String> {
    let chat = root.join("sessions").join(chat_id);
    for name in [
        "configuration_head.json",
        "projection_exclusions.json",
        "turn_index.json",
    ] {
        validate_record(&chat.join(name), false)?;
    }
    validate_record(
        &root.join("prompt-state").join(format!("{chat_id}.json")),
        false,
    )?;
    for directory in [
        "messages",
        "media_refs",
        "checkpoints",
        "configurations",
        "history_replacements",
    ] {
        let entries = match std::fs::read_dir(chat.join(directory)) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(format!("Cannot read Chat {directory}: {error}")),
        };
        for entry in entries {
            let path = entry.map_err(|error| error.to_string())?.path();
            let jsonl = directory == "messages";
            if path.extension().and_then(|extension| extension.to_str())
                == Some(if jsonl { "jsonl" } else { "json" })
            {
                validate_record(&path, jsonl)?;
            }
        }
    }
    Ok(())
}

fn validate_record(path: &Path, jsonl: bool) -> Result<(), String> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(format!(
                "Cannot read Chat state {}: {error}",
                path.display()
            ))
        }
    };
    let records = if jsonl {
        content
            .lines()
            .filter(|line| !line.trim().is_empty())
            .collect::<Vec<_>>()
    } else {
        vec![content.as_str()]
    };
    for record in records {
        serde_json::from_str::<Value>(record)
            .map_err(|error| format!("Invalid Chat state {}: {error}", path.display()))?;
    }
    Ok(())
}

fn sort_objects(value: &mut Value) {
    match value {
        Value::Object(object) => {
            for value in object.values_mut() {
                sort_objects(value);
            }
            object.sort_keys();
        }
        Value::Array(array) => {
            for value in array {
                sort_objects(value);
            }
        }
        _ => {}
    }
}
