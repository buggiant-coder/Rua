use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::atomic::write_json_atomic;
use crate::error::SkillError;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InstallMeta {
    pub installed_from: String,
    pub source_url: String,
    pub installed_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    pub installed_version: String,
    pub content_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub market_meta: Option<serde_json::Value>,
}

const META_FILENAME: &str = "_meta.json";

pub fn write_install_meta(skill_dir: &Path, meta: &InstallMeta) -> Result<(), SkillError> {
    let path = skill_dir.join(META_FILENAME);
    write_json_atomic(&path, meta)
}

pub fn read_install_meta(skill_dir: &Path) -> Option<InstallMeta> {
    let path = skill_dir.join(META_FILENAME);
    let bytes = std::fs::read(&path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

pub fn compute_content_hash(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}
