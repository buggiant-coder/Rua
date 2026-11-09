use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tracing::warn;

use rua_config::dirs::rua_root;

use crate::types::{Finding, ScanResult};
use crate::whitelist::compute_skill_content_hash;

pub const BLOCKED_HISTORY_FILENAME: &str = "skill_scan_blocked.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockedSkillRecord {
    pub skill_name: String,
    pub blocked_at: String,
    pub max_severity: String,
    #[serde(default)]
    pub findings: Vec<serde_json::Value>,
    #[serde(default)]
    pub content_hash: String,
    #[serde(default = "default_action")]
    pub action: String,
}

fn default_action() -> String {
    "blocked".to_string()
}

static HISTORY_LOCK: Mutex<()> = Mutex::new(());

pub fn blocked_history_path() -> PathBuf {
    rua_root().join(BLOCKED_HISTORY_FILENAME)
}

pub fn get_blocked_history() -> Vec<BlockedSkillRecord> {
    get_blocked_history_at(&blocked_history_path())
}

pub fn get_blocked_history_at(path: &Path) -> Vec<BlockedSkillRecord> {
    if !path.is_file() {
        return Vec::new();
    }
    match std::fs::read_to_string(path) {
        Ok(s) => match serde_json::from_str::<Vec<BlockedSkillRecord>>(&s) {
            Ok(v) => v,
            Err(err) => {
                warn!(path = %path.display(), error = %err, "failed to parse blocked history");
                Vec::new()
            }
        },
        Err(err) => {
            warn!(path = %path.display(), error = %err, "failed to read blocked history");
            Vec::new()
        }
    }
}

pub fn clear_blocked_history() {
    clear_blocked_history_at(&blocked_history_path())
}

pub fn clear_blocked_history_at(path: &Path) {
    let _g = HISTORY_LOCK.lock().unwrap();
    if path.is_file() {
        if let Err(err) = std::fs::remove_file(path) {
            warn!(path = %path.display(), error = %err, "failed to clear blocked history");
        }
    }
}

pub fn remove_blocked_entry(skill_name: &str, content_hash: &str) -> bool {
    remove_blocked_entry_at(&blocked_history_path(), skill_name, content_hash)
}

pub fn remove_blocked_entry_at(path: &Path, skill_name: &str, content_hash: &str) -> bool {
    let _g = HISTORY_LOCK.lock().unwrap();
    if !path.is_file() {
        return false;
    }
    let mut records = match std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str::<Vec<BlockedSkillRecord>>(&s).ok())
    {
        Some(v) => v,
        None => return false,
    };
    let before = records.len();
    records.retain(|r| !(r.skill_name == skill_name && r.content_hash == content_hash));
    if records.len() == before {
        return false;
    }
    let payload = match serde_json::to_string_pretty(&records) {
        Ok(s) => s,
        Err(err) => {
            warn!(error = %err, "serialise blocked history");
            return false;
        }
    };
    if let Err(err) = std::fs::write(path, payload) {
        warn!(path = %path.display(), error = %err, "failed to rewrite blocked history");
        return false;
    }
    true
}

pub fn record_blocked_skill(result: &ScanResult, skill_dir: &Path, action: &str) {
    record_blocked_skill_at(&blocked_history_path(), result, skill_dir, action)
}

pub fn record_blocked_skill_at(path: &Path, result: &ScanResult, skill_dir: &Path, action: &str) {
    let _g = HISTORY_LOCK.lock().unwrap();
    let record = BlockedSkillRecord {
        skill_name: result.skill_name.clone(),
        blocked_at: chrono::Utc::now().to_rfc3339(),
        max_severity: result.max_severity.as_str().to_string(),
        findings: result.findings.iter().map(finding_to_record_json).collect(),
        content_hash: compute_skill_content_hash(skill_dir).unwrap_or_default(),
        action: action.to_string(),
    };
    let mut existing: Vec<BlockedSkillRecord> = if path.is_file() {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    existing.push(record);
    if let Some(parent) = path.parent() {
        if let Err(err) = std::fs::create_dir_all(parent) {
            warn!(path = %parent.display(), error = %err, "create history dir");
            return;
        }
    }
    match serde_json::to_string_pretty(&existing) {
        Ok(payload) => {
            if let Err(err) = std::fs::write(path, payload) {
                warn!(path = %path.display(), error = %err, "write blocked history");
            }
        }
        Err(err) => warn!(error = %err, "serialise blocked history"),
    }
}

fn finding_to_record_json(f: &Finding) -> serde_json::Value {
    serde_json::json!({
        "severity": f.severity.as_str(),
        "title": f.title,
        "description": f.description,
        "file_path": f.file_path,
        "line_number": f.line_number,
        "rule_id": f.rule_id,
    })
}
