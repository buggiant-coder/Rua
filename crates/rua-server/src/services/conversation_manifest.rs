use std::collections::BTreeMap;
use std::path::Path;

use rua_core::{ChatAddress, ChatScope, Result, RuaError};
use rua_session::WorkspaceSessionStore;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConversationManifest {
    pub address: ChatAddress,
    pub latest_turn: u64,
    pub latest_sequence: u64,
    pub turn_count: u64,
    pub message_count: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_turn_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_message_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_turn_status: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConversationManifestMismatch {
    Address,
    LatestTurn,
    LatestSequence,
    TurnCount,
    MessageCount,
    LatestTurnId,
    LatestMessageId,
    LatestTurnStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationManifestComparison {
    pub matches: bool,
    pub mismatches: Vec<ConversationManifestMismatch>,
}

impl ConversationManifest {
    pub fn compare(&self, other: &Self) -> ConversationManifestComparison {
        let mut mismatches = Vec::new();
        if self.address != other.address {
            mismatches.push(ConversationManifestMismatch::Address);
        }
        if self.latest_turn != other.latest_turn {
            mismatches.push(ConversationManifestMismatch::LatestTurn);
        }
        if self.latest_sequence != other.latest_sequence {
            mismatches.push(ConversationManifestMismatch::LatestSequence);
        }
        if self.turn_count != other.turn_count {
            mismatches.push(ConversationManifestMismatch::TurnCount);
        }
        if self.message_count != other.message_count {
            mismatches.push(ConversationManifestMismatch::MessageCount);
        }
        if self.latest_turn_id != other.latest_turn_id {
            mismatches.push(ConversationManifestMismatch::LatestTurnId);
        }
        if self.latest_message_id != other.latest_message_id {
            mismatches.push(ConversationManifestMismatch::LatestMessageId);
        }
        if self.latest_turn_status != other.latest_turn_status {
            mismatches.push(ConversationManifestMismatch::LatestTurnStatus);
        }
        ConversationManifestComparison {
            matches: mismatches.is_empty(),
            mismatches,
        }
    }

    pub fn hash(&self) -> Result<String> {
        let mut payload = serde_json::Map::new();
        match self.address.scope() {
            ChatScope::Agent { agent_id, .. } => {
                payload.insert(
                    "agent_id".to_string(),
                    serde_json::Value::String(agent_id.to_string()),
                );
                payload.insert(
                    "chat_id".to_string(),
                    serde_json::Value::String(self.address.chat_id().to_string()),
                );
            }
            ChatScope::Project { .. } => {
                payload.insert(
                    "address".to_string(),
                    serde_json::to_value(&self.address).map_err(|error| RuaError::Io {
                        message: format!("failed to serialize conversation address: {error}"),
                    })?,
                );
            }
        }
        payload.insert("latest_turn".to_string(), self.latest_turn.into());
        payload.insert("latest_sequence".to_string(), self.latest_sequence.into());
        payload.insert("turn_count".to_string(), self.turn_count.into());
        payload.insert("message_count".to_string(), self.message_count.into());
        if let Some(value) = &self.latest_turn_id {
            payload.insert(
                "latest_turn_id".to_string(),
                serde_json::Value::String(value.clone()),
            );
        }
        if let Some(value) = &self.latest_message_id {
            payload.insert(
                "latest_message_id".to_string(),
                serde_json::Value::String(value.clone()),
            );
        }
        if let Some(value) = &self.latest_turn_status {
            payload.insert(
                "latest_turn_status".to_string(),
                serde_json::Value::String(value.clone()),
            );
        }
        canonical_sha256(&serde_json::Value::Object(payload))
    }
}

pub async fn load_conversation_manifest(
    scope_root: &Path,
    address: &ChatAddress,
) -> Result<Option<ConversationManifest>> {
    let store = WorkspaceSessionStore::new(scope_root);
    let chat_id = address.chat_id().as_str();
    let Some(header) = store.load_chat_header(chat_id).await? else {
        return Ok(None);
    };
    if header.chat_id != chat_id
        || address
            .agent_id()
            .is_some_and(|agent_id| header.agent_id != agent_id.as_str())
    {
        return Err(RuaError::Io {
            message: format!("conversation header identity does not match Chat Address {address}"),
        });
    }
    let latest_turn = if header.turn_index.latest_turn == 0 {
        None
    } else {
        store
            .read_turn_range(chat_id, header.turn_index.latest_turn, 1)
            .await?
            .into_iter()
            .next()
    };
    let latest_message = store
        .read_latest_message_from_header(chat_id, &header.message_log)
        .await?;

    Ok(Some(ConversationManifest {
        address: address.clone(),
        latest_turn: header.turn_index.latest_turn,
        latest_sequence: header.message_log.latest_sequence,
        turn_count: header.turn_index.latest_turn,
        message_count: header.message_log.latest_sequence,
        latest_turn_id: latest_turn.as_ref().map(|turn| turn.turn_id.clone()),
        latest_message_id: latest_message.map(|message| message.message_id),
        latest_turn_status: latest_turn.map(|turn| turn.lifecycle.status),
        extra: BTreeMap::new(),
    }))
}

pub fn initial_conversation_manifest_hash(address: &ChatAddress) -> Result<String> {
    ConversationManifest {
        address: address.clone(),
        latest_turn: 0,
        latest_sequence: 0,
        turn_count: 0,
        message_count: 0,
        latest_turn_id: None,
        latest_message_id: None,
        latest_turn_status: None,
        extra: BTreeMap::new(),
    }
    .hash()
}

fn canonical_sha256(value: &Value) -> Result<String> {
    let canonical = canonicalize_json(value);
    let bytes = serde_json::to_vec(&canonical).map_err(|error| RuaError::Io {
        message: format!("failed to serialize canonical manifest payload: {error}"),
    })?;
    Ok(format!("sha256:{}", hex::encode(Sha256::digest(bytes))))
}

fn canonicalize_json(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let sorted = map
                .iter()
                .map(|(key, value)| (key.clone(), canonicalize_json(value)))
                .collect();
            Value::Object(sorted)
        }
        Value::Array(values) => Value::Array(values.iter().map(canonicalize_json).collect()),
        other => other.clone(),
    }
}
