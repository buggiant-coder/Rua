use serde::{Deserialize, Serialize};

use crate::hlc::HlcTimestamp;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncMeta {
    pub device_id: String,
    pub hlc: HlcTimestamp,
    pub modified_at: String,
}
