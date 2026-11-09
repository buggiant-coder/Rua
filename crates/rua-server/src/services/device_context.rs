use std::path::Path;
use std::sync::Arc;

use rua_sync::device_list::{DeviceEntry, DeviceList};

pub fn current_sync_device_id(
    sync_device: &Arc<std::sync::RwLock<Option<Arc<rua_sync::identity::DeviceIdentity>>>>,
) -> Option<String> {
    sync_device.read().ok().and_then(|guard| {
        guard
            .as_ref()
            .map(|device| hex::encode(device.device_id_short()))
    })
}

pub fn known_devices_for_runtime(
    data_dir: &Path,
    local_device_id: Option<&str>,
) -> Vec<serde_json::Value> {
    load_device_list(data_dir)
        .map(|device_list| {
            device_list
                .devices
                .iter()
                .map(|device| device_to_runtime_json(device, local_device_id))
                .collect()
        })
        .unwrap_or_default()
}

pub fn device_name_snapshot(data_dir: &Path, device_id: &str) -> Option<String> {
    load_device_list(data_dir).and_then(|device_list| {
        device_list
            .devices
            .iter()
            .find(|device| hex::encode(device.device_id) == device_id)
            .and_then(device_display_name)
    })
}

fn load_device_list(data_dir: &Path) -> Option<DeviceList> {
    let path = data_dir.join("sync").join("device_list.json");
    let content = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

fn device_to_runtime_json(
    device: &DeviceEntry,
    local_device_id: Option<&str>,
) -> serde_json::Value {
    let device_id = hex::encode(device.device_id);
    serde_json::json!({
        "device_id": device_id,
        "friendly_name": device.friendly_name,
        "label": device.label,
        "last_seen_ms": device.last_seen_ms,
        "is_self": local_device_id == Some(device_id.as_str()),
    })
}

fn device_display_name(device: &DeviceEntry) -> Option<String> {
    let friendly = device.friendly_name.trim();
    if !friendly.is_empty() {
        return Some(friendly.to_string());
    }
    let label = device.label.trim();
    if !label.is_empty() {
        return Some(label.to_string());
    }
    None
}
