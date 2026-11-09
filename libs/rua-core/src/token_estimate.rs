use serde::Serialize;
use serde_json::{Map, Value};

pub const DEFAULT_RESIZED_IMAGE_TOKEN_ESTIMATE: u64 = 1_844;

#[derive(Default)]
struct ImageAdjustment {
    payload_bytes: u64,
    image_count: u64,
}

pub fn estimate_serialized_tokens<T: Serialize>(value: &T, divisor: f64) -> u64 {
    let Ok(json) = serde_json::to_value(value) else {
        return 1;
    };
    estimate_json_tokens(&json, divisor)
}

pub fn estimate_json_tokens(value: &Value, divisor: f64) -> u64 {
    let divisor = divisor.max(1.0);
    let raw_bytes = serde_json::to_vec(value)
        .map(|bytes| bytes.len() as u64)
        .unwrap_or(1);
    let mut adjustment = ImageAdjustment::default();
    collect_image_adjustment(value, &mut adjustment);

    let payload_tokens = if adjustment.payload_bytes == 0 {
        0
    } else {
        tokens_for_bytes(adjustment.payload_bytes, divisor)
    };
    tokens_for_bytes(raw_bytes, divisor)
        .saturating_sub(payload_tokens)
        .saturating_add(
            adjustment
                .image_count
                .saturating_mul(DEFAULT_RESIZED_IMAGE_TOKEN_ESTIMATE),
        )
        .max(1)
}

fn tokens_for_bytes(bytes: u64, divisor: f64) -> u64 {
    ((bytes as f64 / divisor).ceil() as u64).max(1)
}

fn collect_image_adjustment(value: &Value, adjustment: &mut ImageAdjustment) {
    match value {
        Value::Array(items) => {
            for item in items {
                collect_image_adjustment(item, adjustment);
            }
        }
        Value::Object(object) => {
            if let Some(payload_bytes) = image_payload_bytes(object) {
                adjustment.image_count = adjustment.image_count.saturating_add(1);
                adjustment.payload_bytes = adjustment.payload_bytes.saturating_add(payload_bytes);
                return;
            }
            if is_image_object(object) {
                adjustment.image_count = adjustment.image_count.saturating_add(1);
                return;
            }
            for child in object.values() {
                collect_image_adjustment(child, adjustment);
            }
        }
        Value::String(text) => {
            if let Some(payload_bytes) = base64_image_payload_bytes(text) {
                adjustment.image_count = adjustment.image_count.saturating_add(1);
                adjustment.payload_bytes = adjustment.payload_bytes.saturating_add(payload_bytes);
                return;
            }
            let trimmed = text.trim_start();
            if (trimmed.starts_with('[') || trimmed.starts_with('{')) && trimmed.contains("image") {
                if let Ok(parsed) = serde_json::from_str::<Value>(text) {
                    collect_image_adjustment(&parsed, adjustment);
                }
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

fn image_payload_bytes(object: &Map<String, Value>) -> Option<u64> {
    if !is_image_object(object) {
        return None;
    }
    image_url(object)
        .and_then(base64_image_payload_bytes)
        .or_else(|| base64_source_payload_bytes(object))
}

fn is_image_object(object: &Map<String, Value>) -> bool {
    matches!(
        object.get("type").and_then(Value::as_str),
        Some("image" | "input_image" | "image_url")
    )
}

fn image_url(object: &Map<String, Value>) -> Option<&str> {
    object
        .get("url")
        .and_then(Value::as_str)
        .or_else(|| object.get("image_url").and_then(Value::as_str))
        .or_else(|| {
            object
                .get("image_url")
                .and_then(Value::as_object)
                .and_then(|value| value.get("url"))
                .and_then(Value::as_str)
        })
        .or_else(|| {
            object
                .get("source")
                .and_then(Value::as_object)
                .and_then(|source| source.get("url"))
                .and_then(Value::as_str)
        })
}

fn base64_source_payload_bytes(object: &Map<String, Value>) -> Option<u64> {
    let source = object.get("source")?.as_object()?;
    let is_base64 = source.get("type").and_then(Value::as_str) == Some("base64");
    let is_image = source
        .get("media_type")
        .and_then(Value::as_str)
        .is_some_and(|media_type| media_type.starts_with("image/"));
    (is_base64 && is_image)
        .then(|| source.get("data").and_then(Value::as_str))
        .flatten()
        .map(|data| data.len() as u64)
}

fn base64_image_payload_bytes(url: &str) -> Option<u64> {
    let comma = url.find(',')?;
    let metadata = &url[..comma];
    let payload = &url[comma + 1..];
    let metadata_lower = metadata.to_ascii_lowercase();
    (metadata_lower.starts_with("data:image/") && metadata_lower.contains(";base64"))
        .then_some(payload.len() as u64)
}
