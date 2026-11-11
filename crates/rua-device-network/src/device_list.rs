use std::collections::BTreeMap;

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
pub struct DeviceNameRecord {
    pub friendly_name: String,
    /// Sortable HLC bytes of the explicit name edit, independent of presence.
    pub revision: [u8; 20],
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceList {
    pub devices: Vec<DeviceEntry>,
    #[serde(default)]
    pub name_schema_version: u32,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub name_records: BTreeMap<String, DeviceNameRecord>,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl Default for DeviceList {
    fn default() -> Self {
        Self {
            devices: Vec::new(),
            name_schema_version: 1,
            name_records: BTreeMap::new(),
            extra: BTreeMap::new(),
        }
    }
}

impl DeviceList {
    pub fn new() -> Self {
        Self::default()
    }

    /// Import names already saved before provenance was introduced. A fixed
    /// baseline preserves the value without pretending export/presence is an edit.
    pub fn migrate_saved_name_records(&mut self) -> bool {
        if self.name_schema_version != 0 {
            return false;
        }
        for entry in &self.devices {
            if entry.friendly_name.is_empty() || is_generated_friendly_name(entry) {
                continue;
            }
            self.name_records
                .entry(hex::encode(entry.device_id))
                .or_insert_with(|| {
                    let mut revision = [0; 20];
                    revision[12..].copy_from_slice(&entry.device_id);
                    DeviceNameRecord {
                        friendly_name: entry.friendly_name.clone(),
                        revision,
                        ..Default::default()
                    }
                });
        }
        self.name_schema_version = 1;
        true
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
        // Names describe a stable Device identity and survive re-pairing.
        // Retaining a name fact does not grant membership or export a row.
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

    /// Preserve edits committed while a pairing exchange was in progress.
    /// Pairing decides membership; name facts cannot restore a removed row.
    pub fn merge_name_facts_from(&mut self, current: &Self) {
        self.name_schema_version = self.name_schema_version.max(current.name_schema_version);
        for entry in &mut self.devices {
            let id = hex::encode(entry.device_id);
            if let Some(record) = current.name_records.get(&id) {
                if self
                    .name_records
                    .get(&id)
                    .is_none_or(|old| record.revision > old.revision)
                {
                    self.name_records.insert(id.clone(), record.clone());
                }
            }
            if let Some(record) = self.name_records.get(&id) {
                entry.friendly_name.clone_from(&record.friendly_name);
            } else if let Some(existing) = current
                .devices
                .iter()
                .find(|d| d.device_id == entry.device_id)
            {
                if !existing.friendly_name.is_empty() {
                    entry.friendly_name.clone_from(&existing.friendly_name);
                }
            }
        }
        for (key, value) in &current.extra {
            self.extra
                .entry(key.clone())
                .or_insert_with(|| value.clone());
        }
    }

    pub fn public_keys(&self) -> Vec<[u8; 32]> {
        self.devices
            .iter()
            .map(|device| device.public_key)
            .collect()
    }
}

/// Recognize only the exact generated key/id fallback, never arbitrary hostnames.
pub fn is_generated_friendly_name(entry: &DeviceEntry) -> bool {
    ["android", "ios", "macos", "windows", "linux"]
        .iter()
        .any(|platform| {
            [&entry.public_key[..4], &entry.device_id[..4]]
                .iter()
                .any(|prefix| {
                    entry
                        .friendly_name
                        .eq_ignore_ascii_case(&format!("{platform}-{}", hex::encode(prefix)))
                })
        })
}

impl DeviceNameRecord {
    pub fn is_saved_baseline(&self, device_id: &[u8; 8]) -> bool {
        self.revision[..12] == [0; 12] && self.revision[12..] == device_id[..]
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
