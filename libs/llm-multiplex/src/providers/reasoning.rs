//! Shared request normalization for provider SDK compatibility.
//! Behavior adapted from QwenPaw (Apache-2.0); see THIRD_PARTY_NOTICES.md.

#[cfg(any(feature = "openai-compatible", feature = "gemini"))]
use agent_types::ChatRequest;
use serde_json::{Map, Value};

pub(super) fn merge_options(base: &mut Map<String, Value>, overlay: &Map<String, Value>) {
    for (key, value) in overlay {
        match (base.get_mut(key), value) {
            (Some(Value::Object(base)), Value::Object(overlay)) => merge_options(base, overlay),
            _ => {
                base.insert(key.clone(), value.clone());
            }
        }
    }
}

#[cfg(any(
    feature = "openai-compatible",
    feature = "anthropic",
    feature = "gemini"
))]
pub(super) fn flatten_extra_body(body: &mut Map<String, Value>) {
    // `extra_body` is an SDK argument. The HTTP payload contains its fields at
    // the top level; preserve nested settings before applying typed controls.
    if let Some(Value::Object(extra)) = body.remove("extra_body") {
        merge_options(body, &extra);
    }
}

#[cfg(any(
    feature = "openai-compatible",
    feature = "anthropic",
    feature = "gemini"
))]
pub(super) fn apply_defaults(body: &mut Map<String, Value>, defaults: &Map<String, Value>) {
    let mut merged = defaults.clone();
    flatten_extra_body(&mut merged);
    merge_options(&mut merged, body);
    *body = merged;
}

#[cfg(any(
    feature = "openai-compatible",
    feature = "anthropic",
    feature = "gemini"
))]
pub(super) fn clear_controls(body: &mut Map<String, Value>) {
    for key in [
        "disable_thinking",
        "thinking_enabled",
        "thinking_enable",
        "enable_thinking",
        "thinking_budget",
        "reasoning_effort",
        "thinking",
        "reasoning",
        "thinking_config",
    ] {
        body.remove(key);
    }
}

#[cfg(any(feature = "openai-compatible", feature = "gemini"))]
pub(super) fn off_effort<'a>(request: &'a ChatRequest, default: &'a str) -> &'a str {
    request
        .metadata
        .get("rua_reasoning_off_effort")
        .and_then(Value::as_str)
        .unwrap_or(default)
}

#[cfg(feature = "openai-compatible")]
pub(super) fn normalize_output_limit(options: &mut Map<String, Value>, target: &str) {
    let limit = options
        .remove(target)
        .or_else(|| options.remove("max_tokens"))
        .or_else(|| options.remove("max_completion_tokens"))
        .or_else(|| options.remove("max_output_tokens"));
    for key in ["max_tokens", "max_completion_tokens", "max_output_tokens"] {
        options.remove(key);
    }
    if let Some(limit) = limit {
        options.insert(target.into(), limit);
    }
}

#[cfg(feature = "openai-compatible")]
pub(super) fn uses_max_completion_tokens(model: &str) -> bool {
    let name = model
        .trim()
        .rsplit('/')
        .next()
        .unwrap_or(model)
        .to_lowercase();
    name.starts_with("gpt-5")
        || name.starts_with("gpt-6")
        || (name.starts_with('o') && name.as_bytes().get(1).is_some_and(u8::is_ascii_digit))
}
