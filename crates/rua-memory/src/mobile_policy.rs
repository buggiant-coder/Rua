use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryMobilePolicy {
    pub scan_on_cold_start: bool,

    pub rebuild_index_on_cold_start: bool,

    pub auto_embed_on_cold_start: bool,

    pub allow_background_embedding: bool,

    pub require_charging_for_maintenance: bool,

    pub require_wifi_for_maintenance: bool,

    pub max_background_cpu_seconds: u32,

    pub lazy_index: bool,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

impl MemoryMobilePolicy {
    pub fn mobile_default() -> Self {
        Self {
            scan_on_cold_start: false,
            rebuild_index_on_cold_start: false,
            auto_embed_on_cold_start: false,
            allow_background_embedding: false,
            require_charging_for_maintenance: true,
            require_wifi_for_maintenance: true,
            max_background_cpu_seconds: 6,
            lazy_index: true,
            extra: HashMap::new(),
        }
    }

    pub fn mobile_full() -> Self {
        Self {
            scan_on_cold_start: false,
            rebuild_index_on_cold_start: false,
            auto_embed_on_cold_start: false,
            allow_background_embedding: true,
            require_charging_for_maintenance: true,
            require_wifi_for_maintenance: false,
            max_background_cpu_seconds: 30,
            lazy_index: true,
            extra: HashMap::new(),
        }
    }

    pub fn desktop() -> Self {
        Self {
            scan_on_cold_start: true,
            rebuild_index_on_cold_start: true,
            auto_embed_on_cold_start: true,
            allow_background_embedding: true,
            require_charging_for_maintenance: false,
            require_wifi_for_maintenance: false,
            max_background_cpu_seconds: u32::MAX,
            lazy_index: false,
            extra: HashMap::new(),
        }
    }

    pub fn can_initialize(&self) -> bool {
        self.scan_on_cold_start
    }

    pub fn can_run_maintenance(&self, is_charging: bool, has_wifi: bool) -> bool {
        if self.require_charging_for_maintenance && !is_charging {
            return false;
        }
        if self.require_wifi_for_maintenance && !has_wifi {
            return false;
        }
        true
    }

    pub fn can_embed(&self, is_cold_start: bool) -> bool {
        if is_cold_start && !self.auto_embed_on_cold_start {
            return false;
        }
        self.allow_background_embedding || !is_cold_start
    }
}

impl Default for MemoryMobilePolicy {
    fn default() -> Self {
        Self::desktop()
    }
}
