use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};

use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{Map, Value};

use crate::android_runtime::scan_android_skill_runtime_value;
use crate::atomic::write_manifest_atomic;
use crate::channels::workspace_skills_dir;
use crate::error::SkillError;
use crate::frontmatter::{parse_skill_md_safe, ParsedSkillMd};
use crate::install_meta::read_install_meta;
use crate::lock::{lock_path_for, ManifestLock};
use crate::manifest::{PoolManifest, WorkspaceManifest};
use crate::signature::build_signature;

pub trait BuiltinSignatureProvider: Send + Sync {
    fn signatures(&self) -> BTreeMap<String, String>;

    fn invalidate_cache(&self);
}

fn provider_slot() -> &'static RwLock<Option<Box<dyn BuiltinSignatureProvider>>> {
    static SLOT: OnceLock<RwLock<Option<Box<dyn BuiltinSignatureProvider>>>> = OnceLock::new();
    SLOT.get_or_init(|| RwLock::new(None))
}

pub fn install_builtin_signature_provider(provider: Box<dyn BuiltinSignatureProvider>) {
    *provider_slot().write().unwrap() = Some(provider);
}

pub fn clear_builtin_signature_provider() {
    *provider_slot().write().unwrap() = None;
}

fn current_builtin_signatures() -> BTreeMap<String, String> {
    provider_slot()
        .read()
        .unwrap()
        .as_ref()
        .map(|p| p.signatures())
        .unwrap_or_default()
}

fn invalidate_builtin_signature_cache() {
    if let Some(p) = provider_slot().read().unwrap().as_ref() {
        p.invalidate_cache();
    }
}

pub fn reconcile_workspace_manifest(workspace_dir: &Path) -> Result<WorkspaceManifest, SkillError> {
    let builtin_sigs = current_builtin_signatures();
    reconcile_workspace_manifest_with(workspace_dir, &builtin_sigs)
}

pub fn reconcile_workspace_manifest_with(
    workspace_dir: &Path,
    builtin_sigs: &BTreeMap<String, String>,
) -> Result<WorkspaceManifest, SkillError> {
    std::fs::create_dir_all(workspace_dir)?;
    let skills_root = workspace_skills_dir(workspace_dir);
    std::fs::create_dir_all(&skills_root)?;
    let manifest_path = workspace_dir.join("skill.json");

    if !manifest_path.exists() {
        let mut default = WorkspaceManifest::default_payload();
        write_manifest_atomic(&manifest_path, &mut default)?;
    }

    let _lock = ManifestLock::acquire(&lock_path_for(&manifest_path))?;
    let mut payload = read_json_or_default(&manifest_path, WorkspaceManifest::default_payload());

    let discovered = discover_skill_dirs(&skills_root)?;
    let skills_map = ensure_skills_map(&mut payload);

    for (skill_name, skill_dir) in &discovered {
        let existing = match skills_map.get(skill_name) {
            Some(Value::Object(o)) => Some(o.clone()),
            _ => None,
        };
        let enabled = existing
            .as_ref()
            .and_then(|o| o.get("enabled"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let channels = existing
            .as_ref()
            .and_then(|o| o.get("channels"))
            .and_then(|v| v.as_array())
            .filter(|a| !a.is_empty())
            .cloned()
            .unwrap_or_else(|| vec![Value::String("all".to_string())]);
        let source = if let Some(o) = existing.as_ref() {
            o.get("source")
                .and_then(|v| v.as_str())
                .unwrap_or("customized")
                .to_string()
        } else if builtin_sigs.contains_key(skill_name) {
            "builtin".to_string()
        } else {
            "customized".to_string()
        };

        let metadata = build_skill_metadata_value(skill_name, skill_dir, &source, false, false)?;

        let mut new_entry = Map::new();
        new_entry.insert("enabled".to_string(), Value::Bool(enabled));
        new_entry.insert("channels".to_string(), Value::Array(channels));
        new_entry.insert("source".to_string(), Value::String(source));
        new_entry.insert("metadata".to_string(), Value::Object(metadata.clone()));
        if let Some(req) = metadata.get("requirements") {
            new_entry.insert("requirements".to_string(), req.clone());
        }
        if let Some(updated) = metadata.get("updated_at") {
            new_entry.insert("updated_at".to_string(), updated.clone());
        }
        if let Some(meta) = read_install_meta(skill_dir) {
            if let Ok(meta_val) = serde_json::to_value(&meta) {
                new_entry.insert("install_meta".to_string(), meta_val);
            }
        }
        if let Some(o) = existing.as_ref() {
            if let Some(c) = o.get("config") {
                new_entry.insert("config".to_string(), c.clone());
            }

            for (k, v) in o.iter() {
                if matches!(
                    k.as_str(),
                    "enabled"
                        | "channels"
                        | "source"
                        | "metadata"
                        | "requirements"
                        | "updated_at"
                        | "config"
                        | "install_meta"
                        | "sync_to_hub"
                        | "sync_to_pool"
                ) {
                    continue;
                }
                new_entry.entry(k.clone()).or_insert_with(|| v.clone());
            }
        }
        skills_map.insert(skill_name.clone(), Value::Object(new_entry));
    }

    let to_remove: Vec<String> = skills_map
        .keys()
        .filter(|k| !discovered.contains_key(k.as_str()))
        .cloned()
        .collect();
    for k in to_remove {
        skills_map.remove(&k);
    }

    write_manifest_atomic(&manifest_path, &mut payload)?;

    let manifest: WorkspaceManifest = serde_json::from_value(Value::Object(payload))?;
    Ok(manifest)
}

pub fn reconcile_pool_manifest() -> Result<PoolManifest, SkillError> {
    invalidate_builtin_signature_cache();
    let builtin_sigs = current_builtin_signatures();
    reconcile_pool_manifest_with(&default_pool_dir(), &builtin_sigs)
}

pub fn reconcile_pool_manifest_with(
    pool_dir: &Path,
    builtin_sigs: &BTreeMap<String, String>,
) -> Result<PoolManifest, SkillError> {
    std::fs::create_dir_all(pool_dir)?;
    let manifest_path = pool_dir.join("skill.json");
    if !manifest_path.exists() {
        let mut default = PoolManifest::default_payload();
        write_manifest_atomic(&manifest_path, &mut default)?;
    }

    let mut builtin_names: Vec<String> = builtin_sigs.keys().cloned().collect();
    builtin_names.sort();

    let _lock = ManifestLock::acquire(&lock_path_for(&manifest_path))?;
    let mut payload = read_json_or_default(&manifest_path, PoolManifest::default_payload());

    payload.insert(
        "builtin_skill_names".to_string(),
        Value::Array(builtin_names.iter().cloned().map(Value::String).collect()),
    );

    let discovered = discover_skill_dirs(pool_dir)?;
    let skills_map = ensure_skills_map(&mut payload);

    for (skill_name, skill_dir) in &discovered {
        let existing = match skills_map.get(skill_name) {
            Some(Value::Object(o)) => Some(o.clone()),
            _ => None,
        };
        let (source, protected) =
            classify_pool_source(skill_name, skill_dir, existing.as_ref(), builtin_sigs)?;
        let mut metadata = build_skill_metadata_value(
            skill_name,
            skill_dir,
            &source,
            protected,
            source == "builtin",
        )?;
        if let Some(meta) = read_install_meta(skill_dir) {
            if let Ok(meta_val) = serde_json::to_value(&meta) {
                metadata.insert("install_meta".to_string(), meta_val);
            }
        }
        if let Some(o) = existing.as_ref() {
            if let Some(c) = o.get("config") {
                metadata.insert("config".to_string(), c.clone());
            }
        }
        skills_map.insert(skill_name.clone(), Value::Object(metadata));
    }

    let to_remove: Vec<String> = skills_map
        .keys()
        .filter(|k| !discovered.contains_key(k.as_str()))
        .cloned()
        .collect();
    for k in to_remove {
        skills_map.remove(&k);
    }

    write_manifest_atomic(&manifest_path, &mut payload)?;

    let manifest: PoolManifest = serde_json::from_value(Value::Object(payload))?;
    Ok(manifest)
}

fn default_pool_dir() -> PathBuf {
    rua_config::dirs::rua_root().join("skill_pool")
}

fn read_json_or_default(path: &Path, default: Map<String, Value>) -> Map<String, Value> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(_) => return default,
    };
    match serde_json::from_slice::<Value>(&bytes) {
        Ok(Value::Object(m)) => m,
        Ok(_) | Err(_) => {
            tracing::warn!("Malformed JSON in {}, resetting to default", path.display());
            default
        }
    }
}

fn ensure_skills_map(payload: &mut Map<String, Value>) -> &mut Map<String, Value> {
    let entry = payload
        .entry("skills".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if !entry.is_object() {
        *entry = Value::Object(Map::new());
    }
    entry
        .as_object_mut()
        .expect("ensure_skills_map: just inserted an object")
}

fn discover_skill_dirs(root: &Path) -> Result<BTreeMap<String, PathBuf>, SkillError> {
    let mut out = BTreeMap::new();
    let entries = match std::fs::read_dir(root) {
        Ok(e) => e,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(err) => return Err(err.into()),
    };
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if !path.join("SKILL.md").exists() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        out.insert(name.to_string(), path);
    }
    Ok(out)
}

fn classify_pool_source(
    skill_name: &str,
    skill_dir: &Path,
    existing: Option<&Map<String, Value>>,
    builtin_sigs: &BTreeMap<String, String>,
) -> Result<(String, bool), SkillError> {
    if !builtin_sigs.contains_key(skill_name) {
        return Ok(("customized".to_string(), false));
    }
    if let Some(o) = existing {
        let is_builtin_entry = o.get("source").and_then(|v| v.as_str()) == Some("builtin");
        if is_builtin_entry {
            return Ok(("builtin".to_string(), false));
        }
        return Ok(("customized".to_string(), false));
    }
    let pool_sig = build_signature(skill_dir)?;
    let builtin_sig = builtin_sigs.get(skill_name).cloned().unwrap_or_default();
    if pool_sig == builtin_sig {
        Ok(("builtin".to_string(), false))
    } else {
        Ok(("customized".to_string(), false))
    }
}

fn build_skill_metadata_value(
    skill_name: &str,
    skill_dir: &Path,
    source: &str,
    protected: bool,
    compute_signature: bool,
) -> Result<Map<String, Value>, SkillError> {
    let parsed = parse_skill_md_for_metadata(skill_dir, skill_name);
    let signature = if compute_signature {
        build_signature(skill_dir)?
    } else {
        String::new()
    };
    let mut m = Map::new();
    m.insert("name".to_string(), Value::String(skill_name.to_string()));
    m.insert("description".to_string(), Value::String(parsed.description));
    m.insert("version_text".to_string(), Value::String(parsed.version));
    m.insert("commit_text".to_string(), Value::String(String::new()));
    m.insert("signature".to_string(), Value::String(signature));
    m.insert("source".to_string(), Value::String(source.to_string()));
    m.insert("protected".to_string(), Value::Bool(protected));
    m.insert(
        "requirements".to_string(),
        serde_json::to_value(&parsed.requirements)?,
    );
    m.insert(
        "updated_at".to_string(),
        Value::String(get_skill_mtime(skill_dir)),
    );
    m.insert(
        "tags".to_string(),
        Value::Array(parsed.tags.into_iter().map(Value::String).collect()),
    );
    m.insert("emoji".to_string(), Value::String(parsed.emoji));
    m.insert(
        "android_runtime".to_string(),
        scan_android_skill_runtime_value(skill_dir)?,
    );
    Ok(m)
}

fn parse_skill_md_for_metadata(skill_dir: &Path, skill_name: &str) -> ParsedSkillMd {
    let path = skill_dir.join("SKILL.md");
    let content = std::fs::read_to_string(&path).unwrap_or_default();
    parse_skill_md_safe(&content, skill_name)
}

fn get_skill_mtime(skill_dir: &Path) -> String {
    let dir_mtime = std::fs::metadata(skill_dir).and_then(|m| m.modified()).ok();
    let md_mtime = std::fs::metadata(skill_dir.join("SKILL.md"))
        .and_then(|m| m.modified())
        .ok();
    let mtime = match (dir_mtime, md_mtime) {
        (Some(a), Some(b)) => Some(if a > b { a } else { b }),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    };
    let Some(t) = mtime else {
        return String::new();
    };
    let dt: DateTime<Utc> = t.into();
    dt.to_rfc3339_opts(SecondsFormat::Micros, true)
}
