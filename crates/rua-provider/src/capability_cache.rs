use std::sync::OnceLock;

use dashmap::DashMap;

#[derive(Debug, Clone, Default)]
pub struct ModelCapabilities {
    pub needs_reasoning_content: bool,
    pub rejects_media: bool,
}

static CACHE: OnceLock<DashMap<String, ModelCapabilities>> = OnceLock::new();

fn global() -> &'static DashMap<String, ModelCapabilities> {
    CACHE.get_or_init(DashMap::new)
}

pub fn cache_key(provider_id: &str, model_id: &str) -> String {
    format!("{provider_id}:{model_id}")
}

pub fn learn_needs_reasoning_content(key: &str) {
    let caps = global();
    caps.entry(key.to_owned())
        .and_modify(|c| c.needs_reasoning_content = true)
        .or_insert_with(|| ModelCapabilities {
            needs_reasoning_content: true,
            ..Default::default()
        });
    tracing::info!(key, "learned: needs_reasoning_content=true");
}

pub fn learn_rejects_media(key: &str) {
    let caps = global();
    caps.entry(key.to_owned())
        .and_modify(|c| c.rejects_media = true)
        .or_insert_with(|| ModelCapabilities {
            rejects_media: true,
            ..Default::default()
        });
    tracing::info!(key, "learned: rejects_media=true");
}

pub fn get_capabilities(key: &str) -> Option<ModelCapabilities> {
    global().get(key).map(|r| r.clone())
}

pub fn needs_reasoning_content(key: &str) -> bool {
    global()
        .get(key)
        .map(|c| c.needs_reasoning_content)
        .unwrap_or(false)
}

pub fn rejects_media(key: &str) -> bool {
    global().get(key).map(|c| c.rejects_media).unwrap_or(false)
}

pub fn clear_for_testing() {
    global().clear();
}
