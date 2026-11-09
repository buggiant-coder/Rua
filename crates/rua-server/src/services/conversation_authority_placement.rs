use serde::{Deserialize, Deserializer, Serialize, Serializer};

use rua_sync::hlc::{HlcClock, HlcTimestamp};

pub const CHAT_AUTHORITY_PLACEMENT_META_KEY: &str = "authority_placement";
pub const CHAT_ACP_BINDING_META_KEY: &str = "acp_binding";
pub const CHAT_MODEL_META_KEY: &str = "model";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ConversationAuthorityEpoch(HlcTimestamp);

impl ConversationAuthorityEpoch {
    pub fn next_for_device(device_id: &str, observed: Option<Self>) -> Self {
        let device_id = authority_epoch_device_id(device_id);
        let mut clock = match observed {
            Some(observed) => HlcClock::resume(device_id, observed.0),
            None => HlcClock::new(device_id),
        };
        Self(clock.now())
    }

    pub fn from_hlc(timestamp: HlcTimestamp) -> Self {
        Self(timestamp)
    }

    pub fn from_parts(wall_ms: u64, counter: u32, device_id: [u8; 8]) -> Self {
        Self(HlcTimestamp {
            wall_ms,
            counter,
            device_id,
        })
    }

    pub fn as_hlc(self) -> HlcTimestamp {
        self.0
    }

    pub fn parse(value: &str) -> Result<Self, ConversationAuthorityPlacementError> {
        HlcTimestamp::from_sortable_string(value)
            .map(Self)
            .map_err(|error| {
                ConversationAuthorityPlacementError::InvalidAuthorityEpoch(error.to_string())
            })
    }
}

impl std::fmt::Display for ConversationAuthorityEpoch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0.to_sortable_string())
    }
}

impl Serialize for ConversationAuthorityEpoch {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0.to_sortable_string())
    }
}

impl<'de> Deserialize<'de> for ConversationAuthorityEpoch {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationAuthorityPlacement {
    pub device_id: String,
    pub authority_epoch: ConversationAuthorityEpoch,
    pub base_manifest_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConversationAuthorityPlacementError {
    InvalidShape(String),
    EmptyDeviceId,
    InvalidAuthorityEpoch(String),
    InvalidBaseManifestHash,
}

impl std::fmt::Display for ConversationAuthorityPlacementError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidShape(message) => write!(f, "invalid chat authority placement: {message}"),
            Self::EmptyDeviceId => write!(f, "chat authority placement device_id cannot be empty"),
            Self::InvalidAuthorityEpoch(message) => write!(
                f,
                "chat authority placement authority_epoch must be a sortable HLC: {message}"
            ),
            Self::InvalidBaseManifestHash => write!(
                f,
                "chat authority placement base_manifest_hash must be a sha256 hash"
            ),
        }
    }
}

impl std::error::Error for ConversationAuthorityPlacementError {}

impl ConversationAuthorityPlacement {
    pub fn validate(&self) -> Result<(), ConversationAuthorityPlacementError> {
        if self.device_id.trim().is_empty() {
            return Err(ConversationAuthorityPlacementError::EmptyDeviceId);
        }
        if self.base_manifest_hash.len() != 71
            || !self.base_manifest_hash.starts_with("sha256:")
            || !self.base_manifest_hash[7..]
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(ConversationAuthorityPlacementError::InvalidBaseManifestHash);
        }
        Ok(())
    }
}

fn authority_epoch_device_id(device_id: &str) -> [u8; 8] {
    if let Ok(bytes) = hex::decode(device_id) {
        if let Ok(device_id) = <[u8; 8]>::try_from(bytes) {
            return device_id;
        }
    }
    let mut derived = [0u8; 8];
    derived.copy_from_slice(&blake3::hash(device_id.as_bytes()).as_bytes()[..8]);
    derived
}

pub fn parse_conversation_authority_placement(
    meta: &serde_json::Value,
) -> Result<ConversationAuthorityPlacement, ConversationAuthorityPlacementError> {
    let value = meta.get(CHAT_AUTHORITY_PLACEMENT_META_KEY).ok_or_else(|| {
        ConversationAuthorityPlacementError::InvalidShape(
            "authority_placement is required".to_string(),
        )
    })?;
    let target = parse_authority_placement_value(value)?;
    target.validate()?;
    Ok(target)
}

fn parse_authority_placement_value(
    value: &serde_json::Value,
) -> Result<ConversationAuthorityPlacement, ConversationAuthorityPlacementError> {
    serde_json::from_value(value.clone())
        .map_err(|e| ConversationAuthorityPlacementError::InvalidShape(e.to_string()))
}

pub fn contains_conversation_authority_placement(meta: &serde_json::Value) -> bool {
    meta.get(CHAT_AUTHORITY_PLACEMENT_META_KEY).is_some()
}

pub fn chat_acp_binding_active(meta: &serde_json::Value) -> bool {
    meta.get(CHAT_ACP_BINDING_META_KEY)
        .and_then(|value| value.as_object())
        .is_some()
}

pub fn chat_acp_binding_device_id(meta: &serde_json::Value) -> Option<String> {
    meta.get(CHAT_ACP_BINDING_META_KEY)
        .and_then(|value| value.get("device_id"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}
