//! Scoped persisted settings readiness for a cooperative Chat handoff.
//! This never recaptures Agent defaults into the Chat configuration snapshot.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use rua_core::{ChatAddress, ChatStore};
use rua_security::approval_authority::StoredApprovalReuseRule;
use rua_sync::agent_security_adapter::AgentSecuritySyncAdapter;
use rua_sync::root_config_adapter::RootConfigSyncAdapter;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// Portable global settings projections the target Device must hold before it can
/// authorize like the current Authority. They travel inline with Prepare so a
/// handoff never depends on a live Sync round trip, but an existing local value is
/// never overwritten: a diverging target fails closed instead.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct HandoffSettingsPayload {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_security: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_security: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct HandoffSettingsSnapshot {
    pub chat_rules: String,
    pub agent_security: String,
    pub root_security: String,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, Value>,
}

pub(crate) fn paths(
    root: &Path,
    address: &ChatAddress,
    agent_id: &str,
) -> io::Result<Vec<PathBuf>> {
    for component in [address.chat_id().as_str(), agent_id] {
        if component.is_empty() || component.contains(['/', '\\', '\0']) || component.contains("..")
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid handoff settings identity",
            ));
        }
    }
    Ok(vec![
        ChatStore::resolve(root, address.clone())
            .root()
            .join("sessions")
            .join(address.chat_id().as_str())
            .join("permission_rules.json"),
        root.join("workspaces").join(agent_id).join("security.json"),
        root.join("config.json"),
    ])
}

pub(crate) fn capture(
    root: &Path,
    address: &ChatAddress,
    agent_id: &str,
) -> io::Result<HandoffSettingsSnapshot> {
    let paths = paths(root, address, agent_id)?;
    rua_config::with_commit_barrier(&paths, || capture_locked(root, agent_id, &paths[0]))
}

pub(crate) fn matches(
    root: &Path,
    address: &ChatAddress,
    agent_id: &str,
    expected: &HandoffSettingsSnapshot,
) -> io::Result<bool> {
    Ok(capture(root, address, agent_id)? == *expected)
}

/// Reread the synchronized projections and commit Released under the same short
/// path locks as every normal settings write/delete. The supplied closure must
/// only synchronously write the separate handoff journal; never await/network.
pub(crate) fn with_unchanged<T>(
    root: &Path,
    address: &ChatAddress,
    agent_id: &str,
    expected: &HandoffSettingsSnapshot,
    commit: impl FnOnce() -> io::Result<T>,
) -> io::Result<T> {
    let paths = paths(root, address, agent_id)?;
    rua_config::with_commit_barrier(&paths, || {
        if capture_locked(root, agent_id, &paths[0])? != *expected {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "handoff settings changed; refresh readiness before release",
            ));
        }
        commit()
    })
}

/// The current portable global projections, for inline delivery with Prepare.
pub(crate) fn payload(
    root: &Path,
    address: &ChatAddress,
    agent_id: &str,
) -> io::Result<HandoffSettingsPayload> {
    let paths = paths(root, address, agent_id)?;
    rua_config::with_commit_barrier(&paths, || {
        Ok(HandoffSettingsPayload {
            agent_security: AgentSecuritySyncAdapter::new(root.to_path_buf(), "")
                .snapshot_for_agent(agent_id)
                .map_err(invalid)?
                .map(|change| change.payload),
            root_security: RootConfigSyncAdapter::new(root.to_path_buf(), root.join("secret"), "")
                .security_snapshot()
                .map_err(invalid)?
                .map(|change| change.payload),
        })
    })
}

/// Install a delivered projection only where this Device has none. A Device that
/// already carries its own Agent/root security keeps it; `matches` then reports the
/// divergence and the handoff is refused rather than silently overridden.
pub(crate) fn apply_missing(
    root: &Path,
    address: &ChatAddress,
    agent_id: &str,
    payload: &HandoffSettingsPayload,
) -> io::Result<()> {
    let paths = paths(root, address, agent_id)?;
    rua_config::with_commit_barrier(&paths, || {
        if let Some(security) = payload.agent_security.as_ref() {
            let path = root.join("workspaces").join(agent_id).join("security.json");
            if !path.exists() {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                rua_config::atomic_write_json(&path, security)?;
            }
        }
        if let Some(security) = payload.root_security.as_ref() {
            let path = root.join("config.json");
            let mut config: Value = match std::fs::File::open(&path) {
                Ok(file) => serde_json::from_reader(file).map_err(invalid)?,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    Value::Object(Default::default())
                }
                Err(error) => return Err(error),
            };
            let object = config.as_object_mut().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "config.json root must be object")
            })?;
            if !object.contains_key("security") {
                object.insert("security".into(), security.clone());
                rua_config::atomic_write_json(&path, &config)?;
            }
        }
        Ok(())
    })
}

fn capture_locked(
    root: &Path,
    agent_id: &str,
    chat_rules_path: &Path,
) -> io::Result<HandoffSettingsSnapshot> {
    let rules: Value = match std::fs::File::open(chat_rules_path) {
        Ok(file) => serde_json::from_reader(file).map_err(invalid)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => Value::Array(Vec::new()),
        Err(error) => return Err(error),
    };
    serde_json::from_value::<Vec<StoredApprovalReuseRule>>(rules.clone()).map_err(invalid)?;
    let agent_security = AgentSecuritySyncAdapter::new(root.to_path_buf(), "")
        .snapshot_for_agent(agent_id)
        .map_err(invalid)?
        .map(|change| change.payload);
    let root_security = RootConfigSyncAdapter::new(root.to_path_buf(), root.join("secret"), "")
        .security_snapshot()
        .map_err(invalid)?
        .map(|change| change.payload);
    Ok(HandoffSettingsSnapshot {
        chat_rules: digest(rules)?,
        agent_security: digest(serde_json::to_value(agent_security).map_err(invalid)?)?,
        root_security: digest(serde_json::to_value(root_security).map_err(invalid)?)?,
        extra: BTreeMap::new(),
    })
}

fn invalid(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

fn digest(mut value: Value) -> io::Result<String> {
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
    sort_objects(&mut value);
    Ok(format!(
        "sha256:{}",
        hex::encode(Sha256::digest(serde_json::to_vec(&value).map_err(invalid)?))
    ))
}
