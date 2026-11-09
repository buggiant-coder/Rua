use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConflictRecord {
    pub logical_target: String,
    pub local_hlc_hex: String,
    pub local_device_id_hex: String,
    pub remote_hlc_hex: String,
    pub remote_device_id_hex: String,
    pub local_value_summary: String,
    pub remote_value_summary: String,
    pub detected_at_ms: u64,
}
