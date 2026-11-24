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

/// Chat-scoped readiness. When the Authority could publish its current rules, the
/// target must have applied exactly them (including an explicitly cleared set), so
/// its first Turn reuses the same grants. When Sync is off nothing was published to
/// wait for, and the target keeps whatever it already holds — its first Turn then
/// re-evaluates each grant against its own policy instead of inheriting a stale one.
/// Agent/root security is Device-local and never a switch prerequisite.
pub(crate) fn chat_rules_ready(
    root: &Path,
    address: &ChatAddress,
    expected: &HandoffSettingsSnapshot,
    published: bool,
) -> io::Result<bool> {
    if !published {
        return Ok(true);
    }
    let path = ChatStore::resolve(root, address.clone())
        .root()
        .join("sessions")
        .join(address.chat_id().as_str())
        .join("permission_rules.json");
    let rules: Value = match std::fs::File::open(&path) {
        Ok(file) => serde_json::from_reader(file).map_err(invalid)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => Value::Array(Vec::new()),
        Err(error) => return Err(error),
    };
    serde_json::from_value::<Vec<StoredApprovalReuseRule>>(rules.clone()).map_err(invalid)?;
    Ok(digest(rules)? == expected.chat_rules)
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

fn digest_sorted(value: &mut Value) {
    match value {
        Value::Object(object) => {
            for value in object.values_mut() {
                digest_sorted(value);
            }
            object.sort_keys();
        }
        Value::Array(array) => {
            for value in array {
                digest_sorted(value);
            }
        }
        _ => {}
    }
}

fn digest(mut value: Value) -> io::Result<String> {
    digest_sorted(&mut value);
    Ok(format!(
        "sha256:{}",
        hex::encode(Sha256::digest(serde_json::to_vec(&value).map_err(invalid)?))
    ))
}
