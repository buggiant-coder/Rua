use std::sync::Arc;

pub const DEVICE_ACTIVITY_KEY: &str = "device_activity";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatDeviceActivityUpdate {
    pub device_id: Option<String>,
    pub host_profile: String,
}

pub fn device_id_from_sync_device(
    sync_device: &std::sync::RwLock<Option<Arc<rua_sync::identity::DeviceIdentity>>>,
) -> Option<String> {
    sync_device.read().ok().and_then(|guard| {
        guard
            .as_ref()
            .map(|device| hex::encode(device.device_id_short()))
    })
}

pub fn apply_chat_created(meta: &mut serde_json::Value, update: &ChatDeviceActivityUpdate) {
    apply_chat_activity(meta, update, true);
}

pub fn apply_chat_activity(
    meta: &mut serde_json::Value,
    update: &ChatDeviceActivityUpdate,
    is_create: bool,
) {
    if !meta.is_object() {
        *meta = serde_json::json!({});
    }
    let Some(root) = meta.as_object_mut() else {
        return;
    };
    let activity = root
        .entry(DEVICE_ACTIVITY_KEY.to_string())
        .or_insert_with(|| serde_json::json!({}));
    if !activity.is_object() {
        *activity = serde_json::json!({});
    }
    let Some(activity) = activity.as_object_mut() else {
        return;
    };

    if let Some(device_id) = update
        .device_id
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        if is_create {
            activity
                .entry("created_on_device_id".to_string())
                .or_insert_with(|| serde_json::json!(device_id));
        }
        activity.insert(
            "last_active_device_id".to_string(),
            serde_json::json!(device_id),
        );
        upsert_participant_device(activity, device_id);
    }

    if !update.host_profile.is_empty() {
        activity.insert(
            "last_run_host_profile".to_string(),
            serde_json::json!(update.host_profile),
        );
    }
}

pub fn copy_chat_activity_to_header_extra(
    meta: &serde_json::Value,
    extra: &mut serde_json::Map<String, serde_json::Value>,
) {
    if let Some(activity) = meta.get(DEVICE_ACTIVITY_KEY) {
        extra.insert(DEVICE_ACTIVITY_KEY.to_string(), activity.clone());
    }
}

fn upsert_participant_device(
    activity: &mut serde_json::Map<String, serde_json::Value>,
    device_id: &str,
) {
    let participants = activity
        .entry("participant_device_ids".to_string())
        .or_insert_with(|| serde_json::Value::Array(Vec::new()));
    if !participants.is_array() {
        *participants = serde_json::Value::Array(Vec::new());
    }
    let Some(values) = participants.as_array_mut() else {
        return;
    };
    if !values.iter().any(|value| value.as_str() == Some(device_id)) {
        values.push(serde_json::json!(device_id));
    }
}
