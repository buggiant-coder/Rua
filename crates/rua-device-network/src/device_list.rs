use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceEntry {
    pub device_id: [u8; 8],
    pub public_key: [u8; 32],
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub friendly_name: String,
    pub added_at_ms: u64,
    #[serde(default)]
    pub last_seen_ms: u64,
    #[serde(default)]
    pub direct_addrs: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceList {
    pub devices: Vec<DeviceEntry>,
}

impl DeviceList {
    pub fn new() -> Self {
        Self {
            devices: Vec::new(),
        }
    }

    pub fn contains_device(&self, device_id: &[u8; 8]) -> bool {
        self.devices
            .iter()
            .any(|device| &device.device_id == device_id)
    }

    pub fn add_device(&mut self, entry: DeviceEntry) {
        if !self.contains_device(&entry.device_id) {
            self.devices.push(entry);
        }
    }

    pub fn remove_device(&mut self, device_id: &[u8; 8]) {
        self.devices.retain(|device| &device.device_id != device_id);
    }

    pub fn rename_device(&mut self, device_id: &[u8; 8], friendly_name: &str) {
        if let Some(entry) = self
            .devices
            .iter_mut()
            .find(|device| &device.device_id == device_id)
        {
            entry.friendly_name = friendly_name.to_string();
        }
    }

    pub fn public_keys(&self) -> Vec<[u8; 32]> {
        self.devices
            .iter()
            .map(|device| device.public_key)
            .collect()
    }
}

pub fn default_friendly_name(platform: &str, public_key: &[u8; 32]) -> String {
    if let Ok(hostname) = hostname::get() {
        if let Some(name) = hostname.to_str() {
            if !name.is_empty() && name != "localhost" {
                return name.to_string();
            }
        }
    }
    let short_hex = hex::encode(&public_key[..4]);
    format!("{platform}-{short_hex}")
}
