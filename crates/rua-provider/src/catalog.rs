//! QwenPaw's versioned provider data, imported without a Python runtime dependency.
//! Copyright 2025 The QwenPaw Authors. Adapted under Apache-2.0; see THIRD_PARTY_NOTICES.md.

use std::sync::LazyLock;

use serde::Deserialize;

use crate::config_loader::ProviderConfig;

#[derive(Deserialize)]
struct Catalog {
    providers: Vec<ProviderConfig>,
    compatibility_rules: CompatibilityRules,
    #[serde(flatten)]
    _extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Deserialize)]
pub(crate) struct CompatibilityRules {
    pub opencode_unavailable_models: Vec<String>,
    pub non_chat_tokens: Vec<String>,
    pub non_chat_substrings: Vec<String>,
    pub modelscope_non_chat_markers: Vec<String>,
    pub dashscope_non_chat_markers: Vec<String>,
    pub openai_none_effort_models: Vec<String>,
    pub reasoning_budgets: std::collections::HashMap<String, i64>,
    #[serde(flatten)]
    _extra: serde_json::Map<String, serde_json::Value>,
}

static CATALOG: LazyLock<Catalog> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../data/qwenpaw.json"))
        .expect("validated QwenPaw provider catalog")
});

pub(crate) fn provider(id: &str) -> Option<&'static ProviderConfig> {
    let upstream_id = match id {
        "google" => "gemini",
        "rua-local" => "qwenpaw-local",
        other => other,
    };
    CATALOG
        .providers
        .iter()
        .find(|entry| entry.id == upstream_id)
}

pub(crate) fn rules() -> &'static CompatibilityRules {
    &CATALOG.compatibility_rules
}

pub(crate) fn supports_chat(provider_id: &str, model_id: &str) -> bool {
    let rules = rules();
    let normalized = model_id.trim().to_lowercase();
    let leaf = normalized.rsplit('/').next().unwrap_or(&normalized);
    match provider_id {
        "opencode" => !rules
            .opencode_unavailable_models
            .iter()
            .any(|id| id == model_id),
        "modelscope" => {
            !leaf.ends_with("-pt")
                && !rules
                    .modelscope_non_chat_markers
                    .iter()
                    .any(|marker| normalized.contains(marker))
        }
        "dashscope" => !rules
            .dashscope_non_chat_markers
            .iter()
            .any(|marker| normalized.contains(marker)),
        "mimo" => {
            !rules
                .non_chat_substrings
                .iter()
                .any(|marker| leaf.contains(marker))
                && !leaf
                    .split(|c: char| !c.is_ascii_alphanumeric())
                    .any(|token| rules.non_chat_tokens.iter().any(|marker| marker == token))
        }
        _ => true,
    }
}
