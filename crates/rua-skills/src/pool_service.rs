use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::android_runtime::scan_android_skill_runtime_value;
use crate::atomic::write_manifest_atomic;
use crate::builtin::{
    builtin_signature, builtin_signatures, builtin_skill_names, default_pool_dir,
    ensure_skill_pool_initialized_in, BuiltinStatus,
};
use crate::channels::workspace_skills_dir;
use crate::copy::copy_skill_dir;
use crate::error::SkillError;
use crate::frontmatter::{parse_skill_md, parse_skill_md_safe, ParsedSkillMd};
use crate::install_meta::{read_install_meta, write_install_meta};
use crate::lock::{lock_path_for, ManifestLock};
use crate::manifest::{PoolManifest, WorkspaceManifest};
use crate::scanner_integration::{maybe_scan_skill, ScannerHandle};
use crate::service::{SkillBundleWrite, SkillService};
use crate::signature::build_signature;
use crate::types::{SkillInfo, SkillSource};

pub use crate::builtin::BuiltinStatus as SyncStatus;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadResult {
    pub name: String,
    pub signature: String,
    pub source: SkillSource,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PoolBuiltinNotice {
    pub has_updates: bool,
    pub fingerprint: String,
    pub total_changes: usize,
    pub actionable_skill_names: Vec<String>,
    pub added: Vec<PoolBuiltinNoticeItem>,
    pub missing: Vec<PoolBuiltinNoticeItem>,
    pub updated: Vec<PoolBuiltinNoticeItem>,
    pub removed: Vec<PoolBuiltinNoticeItem>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PoolBuiltinNoticeItem {
    pub name: String,
    pub version_text: String,
}

#[derive(Debug, Clone)]
pub struct SkillPoolService {
    pool_dir: PathBuf,
    scanner: Option<Arc<dyn ScannerHandle>>,
}

impl SkillPoolService {
    pub fn new() -> Result<Self, SkillError> {
        Self::new_in(default_pool_dir())
    }

    pub fn new_in(pool_dir: impl Into<PathBuf>) -> Result<Self, SkillError> {
        let svc = Self {
            pool_dir: pool_dir.into(),
            scanner: None,
        };
        svc.ensure_initialized()?;
        Ok(svc)
    }

    pub fn with_scanner(mut self, scanner: Arc<dyn ScannerHandle>) -> Self {
        self.scanner = Some(scanner);
        self
    }

    pub fn scanner(&self) -> Option<&Arc<dyn ScannerHandle>> {
        self.scanner.as_ref()
    }

    pub fn pool_dir(&self) -> &Path {
        &self.pool_dir
    }

    pub fn ensure_initialized(&self) -> Result<bool, SkillError> {
        ensure_skill_pool_initialized_in(&self.pool_dir)
    }

    pub fn list_skills(&self) -> Result<Vec<SkillInfo>, SkillError> {
        let manifest = self.read_manifest()?;
        let mut out = Vec::new();
        for (skill_name, entry) in manifest.skills.iter() {
            let skill_dir = self.pool_dir.join(skill_name);
            if let Some(info) = read_skill_from_dir(&skill_dir, entry.source.clone())? {
                out.push(info);
            }
        }
        Ok(out)
    }

    pub fn get_skill(&self, name: &str) -> Result<Option<SkillInfo>, SkillError> {
        let manifest = self.read_manifest()?;
        let Some(entry) = manifest.skills.get(name) else {
            return Ok(None);
        };
        read_skill_from_dir(&self.pool_dir.join(name), entry.source.clone())
    }

    pub fn create_skill(
        &self,
        name: &str,
        content: &str,
        config: Option<Map<String, Value>>,
        source: SkillSource,
    ) -> Result<Option<String>, SkillError> {
        validate_skill_content(content)?;
        let skill_name = normalize_skill_dir_name(name)?;
        std::fs::create_dir_all(&self.pool_dir)?;
        let skill_dir = self.pool_dir.join(&skill_name);

        let manifest = self.read_manifest()?;
        if let Some(existing) = manifest.skills.get(&skill_name) {
            if existing.source != source {
                return Err(SkillError::SourceConflict {
                    skill_name,
                    existing_source: existing.source.display_string(),
                    requested_source: source.display_string(),
                });
            }
            return Ok(None);
        }
        if skill_dir.exists() {
            return Ok(None);
        }

        let stage = tempfile::Builder::new()
            .prefix(&format!("rua_pool_create_{skill_name}_"))
            .tempdir()?;
        let staged_skill = stage.path().join(&skill_name);
        write_skill_md(&staged_skill, content)?;
        maybe_scan_skill(self.scanner.as_ref(), &staged_skill, &skill_name)?;
        copy_skill_dir(&staged_skill, &skill_dir)?;

        let source_str = source.display_string();
        self.mutate_manifest(|payload| {
            let skills = ensure_skills_map(payload);
            let mut entry = build_pool_entry(&skill_name, &skill_dir, &source_str, false)?;
            if let Some(c) = config.as_ref() {
                entry.insert("config".to_string(), Value::Object(c.clone()));
            }
            skills.insert(skill_name.clone(), Value::Object(entry));
            Ok(())
        })?;
        Ok(Some(skill_name))
    }

    pub fn create_skill_bundle(
        &self,
        bundle: SkillBundleWrite<'_>,
    ) -> Result<Option<String>, SkillError> {
        validate_skill_content(bundle.content)?;
        let skill_name = normalize_skill_dir_name(bundle.name)?;
        std::fs::create_dir_all(&self.pool_dir)?;
        let skill_dir = self.pool_dir.join(&skill_name);

        let manifest = self.read_manifest()?;
        if let Some(existing) = manifest.skills.get(&skill_name) {
            if existing.source != SkillSource::Customized {
                return Err(SkillError::SourceConflict {
                    skill_name,
                    existing_source: existing.source.display_string(),
                    requested_source: SkillSource::Customized.display_string(),
                });
            }
            return Ok(None);
        }
        if skill_dir.exists() {
            return Ok(None);
        }

        let stage = tempfile::Builder::new()
            .prefix(&format!("rua_pool_bundle_{skill_name}_"))
            .tempdir()?;
        let staged_skill = stage.path().join(&skill_name);
        write_skill_md(&staged_skill, bundle.content)?;
        write_value_tree(&staged_skill.join("references"), bundle.references)?;
        write_value_tree(&staged_skill.join("scripts"), bundle.scripts)?;
        write_value_tree(&staged_skill, bundle.extra_files)?;
        if let Some(meta) = bundle.install_meta {
            write_install_meta(&staged_skill, meta)?;
        }
        maybe_scan_skill(self.scanner.as_ref(), &staged_skill, &skill_name)?;
        copy_skill_dir(&staged_skill, &skill_dir)?;

        self.mutate_manifest(|payload| {
            let skills = ensure_skills_map(payload);
            let entry = build_pool_entry(&skill_name, &skill_dir, "customized", false)?;
            skills.insert(skill_name.clone(), Value::Object(entry));
            Ok(())
        })?;
        Ok(Some(skill_name))
    }

    pub fn delete_skill(&self, name: &str) -> Result<bool, SkillError> {
        let manifest = self.read_manifest()?;
        if !manifest.skills.contains_key(name) {
            return Ok(false);
        }
        let skill_dir = self.pool_dir.join(name);
        if skill_dir.exists() {
            std::fs::remove_dir_all(&skill_dir)?;
        }
        let target = name.to_string();
        self.mutate_manifest(|payload| {
            if let Some(Value::Object(skills)) = payload.get_mut("skills") {
                skills.remove(&target);
            }
            Ok(())
        })?;
        Ok(true)
    }

    pub fn upload_from_workspace(
        &self,
        workspace_dir: &Path,
        skill_name: &str,
        target_name: Option<&str>,
        overwrite: bool,
    ) -> Result<DownloadResult, SkillError> {
        let source_dir = workspace_skills_dir(workspace_dir).join(skill_name);
        if !source_dir.exists() {
            return Err(SkillError::InvalidSkillName(format!(
                "workspace skill '{skill_name}' not found"
            )));
        }
        let final_name = normalize_skill_dir_name(target_name.unwrap_or(skill_name))?;
        let target_dir = self.pool_dir.join(&final_name);
        let manifest = self.read_manifest()?;
        if let Some(existing) = manifest.skills.get(&final_name) {
            match &existing.source {
                SkillSource::Builtin => {
                    return Err(SkillError::Conflict(format!(
                        "pool slot '{final_name}' is a builtin — pick a different target name"
                    )));
                }
                SkillSource::Plugin(_) => {
                    return Err(SkillError::Conflict(format!(
                        "pool slot '{final_name}' is owned by a plugin — pick a different target name"
                    )));
                }
                SkillSource::Customized if !overwrite => {
                    return Err(SkillError::Conflict(format!(
                        "pool slot '{final_name}' already exists — pass overwrite=true to replace"
                    )));
                }
                SkillSource::Customized => {}
            }
        } else if target_dir.exists() && !overwrite {
            return Err(SkillError::Conflict(format!(
                "pool dir '{final_name}' already exists on disk — pass overwrite=true to replace"
            )));
        }

        maybe_scan_skill(self.scanner.as_ref(), &source_dir, &final_name)?;
        copy_skill_dir(&source_dir, &target_dir)?;

        let ws_config = read_workspace_skill_config(workspace_dir, skill_name);
        let final_name_for_update = final_name.clone();
        self.mutate_manifest(|payload| {
            let skills = ensure_skills_map(payload);
            let mut entry =
                build_pool_entry(&final_name_for_update, &target_dir, "customized", false)?;
            if let Some(cfg) = ws_config.as_ref() {
                entry.insert("config".to_string(), Value::Object(cfg.clone()));
            }
            skills.insert(final_name_for_update.clone(), Value::Object(entry));
            Ok(())
        })?;
        let signature = build_signature(&target_dir)?;
        Ok(DownloadResult {
            name: final_name,
            signature,
            source: SkillSource::Customized,
        })
    }

    pub fn download_to_workspace(
        &self,
        workspace_dir: &Path,
        skill_name: &str,
        target_name: Option<&str>,
        overwrite: bool,
    ) -> Result<DownloadResult, SkillError> {
        let manifest = self.read_manifest()?;
        let pool_entry = manifest.skills.get(skill_name).ok_or_else(|| {
            SkillError::InvalidSkillName(format!("pool skill '{skill_name}' not found"))
        })?;
        let source_dir = self.pool_dir.join(skill_name);
        if !source_dir.exists() {
            return Err(SkillError::InvalidSkillName(format!(
                "pool dir for '{skill_name}' is missing on disk"
            )));
        }

        let final_name = normalize_skill_dir_name(target_name.unwrap_or(skill_name))?;
        let workspace_skill_dir = workspace_skills_dir(workspace_dir).join(&final_name);
        let workspace_config = pool_entry
            .config
            .clone()
            .unwrap_or_else(serde_json::Map::new);

        if workspace_skill_dir.exists() {
            if !overwrite {
                return Err(SkillError::Conflict(format!(
                    "workspace skill '{final_name}' already exists — pass overwrite=true to replace"
                )));
            }
            std::fs::remove_dir_all(&workspace_skill_dir)?;
        }

        maybe_scan_skill(self.scanner.as_ref(), &source_dir, &final_name)?;

        let svc = match self.scanner.as_ref() {
            Some(s) => SkillService::new(workspace_dir.to_path_buf())?.with_scanner(Arc::clone(s)),
            None => SkillService::new(workspace_dir.to_path_buf())?,
        };

        let content = std::fs::read_to_string(source_dir.join("SKILL.md"))?;

        let created =
            svc.create_skill(&final_name, &content, Some(workspace_config.clone()), false)?;
        if created.is_none() {
            return Err(SkillError::Conflict(format!(
                "workspace already created '{final_name}' between overwrite + create"
            )));
        }
        copy_skill_dir(&source_dir, &workspace_skill_dir)?;

        copy_pool_install_meta_to_workspace(
            &self.pool_dir,
            workspace_dir,
            skill_name,
            &final_name,
        )?;
        let source = pool_entry.source.clone();
        let signature = build_signature(&workspace_skill_dir)?;
        Ok(DownloadResult {
            name: final_name,
            signature,
            source,
        })
    }

    pub fn update_pool_skill_tags(
        &self,
        skill_name: &str,
        tags: Vec<String>,
    ) -> Result<(), SkillError> {
        let manifest = self.read_manifest()?;
        if !manifest.skills.contains_key(skill_name) {
            return Err(SkillError::InvalidSkillName(format!(
                "pool skill '{skill_name}' not found"
            )));
        }
        let target = skill_name.to_string();
        let owned_tags = tags;
        self.mutate_manifest(|payload| {
            if let Some(Value::Object(skills)) = payload.get_mut("skills") {
                if let Some(Value::Object(entry)) = skills.get_mut(&target) {
                    entry.insert(
                        "tags".to_string(),
                        Value::Array(owned_tags.iter().cloned().map(Value::String).collect()),
                    );
                }
            }
            Ok(())
        })?;
        Ok(())
    }

    pub fn get_pool_builtin_sync_status(&self) -> Result<BTreeMap<String, SyncStatus>, SkillError> {
        let manifest = self.read_manifest()?;
        let pool_skills: BTreeMap<&String, &crate::manifest::PoolSkillEntry> =
            manifest.skills.iter().collect();
        let mut out: BTreeMap<String, SyncStatus> = BTreeMap::new();
        for name in builtin_skill_names() {
            let source_signature = builtin_signature(&name).unwrap_or_default();
            let status = match pool_skills.get(&name) {
                None => BuiltinStatus::Missing,
                Some(entry) => match &entry.source {
                    SkillSource::Builtin => {
                        if entry.signature == source_signature {
                            BuiltinStatus::Current
                        } else {
                            BuiltinStatus::Outdated
                        }
                    }
                    SkillSource::Customized | SkillSource::Plugin(_) => BuiltinStatus::Conflict,
                },
            };
            out.insert(name, status);
        }
        Ok(out)
    }

    pub fn get_pool_builtin_notice(&self) -> Result<PoolBuiltinNotice, SkillError> {
        let builtin_sigs = builtin_signatures();
        let builtin_names: BTreeSet<String> = builtin_sigs.keys().cloned().collect();
        let manifest = self.read_manifest()?;

        let mut added: Vec<PoolBuiltinNoticeItem> = Vec::new();
        let mut missing: Vec<PoolBuiltinNoticeItem> = Vec::new();
        let mut updated: Vec<PoolBuiltinNoticeItem> = Vec::new();
        let mut removed: Vec<PoolBuiltinNoticeItem> = Vec::new();

        for name in builtin_names.iter() {
            let version = packaged_builtin_version(name);
            match manifest.skills.get(name) {
                None => missing.push(PoolBuiltinNoticeItem {
                    name: name.clone(),
                    version_text: version,
                }),
                Some(entry) => match &entry.source {
                    SkillSource::Customized | SkillSource::Plugin(_) => {
                        added.push(PoolBuiltinNoticeItem {
                            name: name.clone(),
                            version_text: version,
                        })
                    }
                    SkillSource::Builtin => {
                        let pool_sig = entry.signature.clone();
                        let source_sig = builtin_sigs.get(name).cloned().unwrap_or_default();
                        if !pool_sig.is_empty() && pool_sig != source_sig {
                            updated.push(PoolBuiltinNoticeItem {
                                name: name.clone(),
                                version_text: version,
                            });
                        }
                    }
                },
            }
        }

        for (name, entry) in manifest.skills.iter() {
            if entry.source == SkillSource::Builtin && !builtin_names.contains(name) {
                removed.push(PoolBuiltinNoticeItem {
                    name: name.clone(),
                    version_text: String::new(),
                });
            }
        }

        let actionable: BTreeSet<String> = missing
            .iter()
            .chain(updated.iter())
            .map(|i| i.name.clone())
            .collect();

        let mut hasher = Sha256::new();
        let mut first = true;
        for item in added.iter().chain(&missing).chain(&updated).chain(&removed) {
            if !first {
                hasher.update(b"\n");
            }
            first = false;
            hasher.update(item.name.as_bytes());
            hasher.update(b"=");
            hasher.update(item.version_text.as_bytes());
        }
        let fingerprint = format!("{:x}", hasher.finalize());

        let total_changes = added.len() + missing.len() + updated.len() + removed.len();
        Ok(PoolBuiltinNotice {
            has_updates: total_changes > 0,
            fingerprint,
            total_changes,
            actionable_skill_names: actionable.into_iter().collect(),
            added,
            missing,
            updated,
            removed,
        })
    }

    fn read_manifest(&self) -> Result<PoolManifest, SkillError> {
        let path = self.pool_dir.join("skill.json");
        if !path.exists() {
            return Ok(PoolManifest::default());
        }
        let bytes = std::fs::read(&path)?;
        if bytes.is_empty() {
            return Ok(PoolManifest::default());
        }
        let manifest: PoolManifest = serde_json::from_slice(&bytes).unwrap_or_else(|err| {
            tracing::warn!(
                "Malformed pool manifest at {}: {err}, using default",
                path.display()
            );
            PoolManifest::default()
        });
        Ok(manifest)
    }

    fn mutate_manifest<F>(&self, f: F) -> Result<(), SkillError>
    where
        F: FnOnce(&mut Map<String, Value>) -> Result<(), SkillError>,
    {
        let manifest_path = self.pool_dir.join("skill.json");
        if let Some(parent) = manifest_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let _lock = ManifestLock::acquire(&lock_path_for(&manifest_path))?;
        let mut payload = read_manifest_payload(&manifest_path);
        f(&mut payload)?;
        write_manifest_atomic(&manifest_path, &mut payload)?;

        Ok(())
    }
}

pub fn suggest_conflict_name(base_name: &str, existing_names: &[String]) -> String {
    let taken: BTreeSet<&str> = existing_names.iter().map(|s| s.as_str()).collect();
    for i in 1u32..=1_000_000 {
        let candidate = format!("{base_name}-{i}");
        if !taken.contains(candidate.as_str()) {
            return candidate;
        }
    }
    format!("{base_name}-overflow")
}

fn validate_skill_content(content: &str) -> Result<(), SkillError> {
    let parsed = parse_skill_md(content)?;
    if parsed.name.trim().is_empty() || parsed.description.trim().is_empty() {
        return Err(SkillError::InvalidFrontmatter(
            "SKILL.md must include non-empty frontmatter `name` and `description`".to_string(),
        ));
    }
    Ok(())
}

fn normalize_skill_dir_name(name: &str) -> Result<String, SkillError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(SkillError::InvalidSkillName(
            "skill name cannot be empty".to_string(),
        ));
    }
    if trimmed == "." || trimmed == ".." {
        return Err(SkillError::InvalidSkillName(format!(
            "invalid skill name: {trimmed}"
        )));
    }
    if trimmed.contains('/') || trimmed.contains('\\') || trimmed.contains('\0') {
        return Err(SkillError::InvalidSkillName(
            "skill name cannot contain path separators or NUL bytes".to_string(),
        ));
    }
    Ok(trimmed.to_string())
}

fn write_skill_md(skill_dir: &Path, content: &str) -> Result<(), SkillError> {
    std::fs::create_dir_all(skill_dir)?;
    std::fs::write(skill_dir.join("SKILL.md"), content)?;
    Ok(())
}

fn write_value_tree(root: &Path, tree: &Map<String, Value>) -> Result<(), SkillError> {
    if tree.is_empty() {
        return Ok(());
    }
    std::fs::create_dir_all(root)?;
    for (key, value) in tree {
        if key == "." || key == ".." || key.contains('/') || key.contains('\\') {
            continue;
        }
        let path = root.join(key);
        match value {
            Value::Object(child) => write_value_tree(&path, child)?,
            Value::String(body) => {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(path, body)?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn read_skill_from_dir(
    skill_dir: &Path,
    source: SkillSource,
) -> Result<Option<SkillInfo>, SkillError> {
    if !skill_dir.is_dir() {
        return Ok(None);
    }
    let skill_md = skill_dir.join("SKILL.md");
    if !skill_md.exists() {
        return Ok(None);
    }
    let content = match std::fs::read_to_string(&skill_md) {
        Ok(c) => c,
        Err(err) => {
            tracing::error!("failed to read skill {}: {err}", skill_dir.display());
            return Ok(None);
        }
    };
    let parsed = parse_skill_md_safe(
        &content,
        skill_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default(),
    );
    let references = directory_tree(&skill_dir.join("references"));
    let scripts = directory_tree(&skill_dir.join("scripts"));
    Ok(Some(SkillInfo {
        name: skill_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string(),
        description: parsed.description,
        version_text: parsed.version,
        content,
        source,
        references,
        scripts,
        tags: parsed.tags,
        emoji: parsed.emoji,
    }))
}

fn directory_tree(root: &Path) -> Value {
    if !root.is_dir() {
        return Value::Object(Map::new());
    }
    let mut entries: Vec<_> = match std::fs::read_dir(root) {
        Ok(it) => it.filter_map(|e| e.ok()).collect(),
        Err(_) => return Value::Object(Map::new()),
    };
    entries.sort_by_key(|e| e.file_name());
    let mut map = Map::new();
    for entry in entries {
        let name = entry.file_name().to_string_lossy().to_string();
        let path = entry.path();
        if path.is_file() {
            map.insert(name, Value::Null);
        } else if path.is_dir() {
            map.insert(name, directory_tree(&path));
        }
    }
    Value::Object(map)
}

fn build_pool_entry(
    skill_name: &str,
    skill_dir: &Path,
    source: &str,
    compute_signature: bool,
) -> Result<Map<String, Value>, SkillError> {
    let parsed: ParsedSkillMd = parse_packaged_skill_md(skill_dir, skill_name);
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
    m.insert("protected".to_string(), Value::Bool(false));
    m.insert(
        "requirements".to_string(),
        serde_json::to_value(&parsed.requirements)?,
    );
    m.insert(
        "updated_at".to_string(),
        Value::String(skill_mtime(skill_dir)),
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

fn parse_packaged_skill_md(skill_dir: &Path, skill_name: &str) -> ParsedSkillMd {
    let path = skill_dir.join("SKILL.md");
    let content = std::fs::read_to_string(&path).unwrap_or_default();
    parse_skill_md_safe(&content, skill_name)
}

fn skill_mtime(skill_dir: &Path) -> String {
    use chrono::{DateTime, SecondsFormat, Utc};
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

fn read_manifest_payload(path: &Path) -> Map<String, Value> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(_) => return PoolManifest::default_payload(),
    };
    if bytes.is_empty() {
        return PoolManifest::default_payload();
    }
    match serde_json::from_slice::<Value>(&bytes) {
        Ok(Value::Object(m)) => m,
        _ => {
            tracing::warn!("Malformed JSON in {}, resetting to default", path.display());
            PoolManifest::default_payload()
        }
    }
}

fn read_workspace_manifest_payload(path: &Path) -> Map<String, Value> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(_) => return WorkspaceManifest::default_payload(),
    };
    if bytes.is_empty() {
        return WorkspaceManifest::default_payload();
    }
    match serde_json::from_slice::<Value>(&bytes) {
        Ok(Value::Object(m)) => m,
        _ => {
            tracing::warn!("Malformed JSON in {}, resetting to default", path.display());
            WorkspaceManifest::default_payload()
        }
    }
}

fn copy_pool_install_meta_to_workspace(
    pool_dir: &Path,
    workspace_dir: &Path,
    pool_skill_name: &str,
    skill_name: &str,
) -> Result<(), SkillError> {
    let Some(meta) = read_install_meta(&pool_dir.join(pool_skill_name)) else {
        return Ok(());
    };
    let Ok(meta_val) = serde_json::to_value(meta) else {
        return Ok(());
    };

    let manifest_path = workspace_dir.join("skill.json");
    let _lock = ManifestLock::acquire(&lock_path_for(&manifest_path))?;
    let mut payload = read_workspace_manifest_payload(&manifest_path);
    let skills = ensure_skills_map(&mut payload);
    let Some(Value::Object(entry)) = skills.get_mut(skill_name) else {
        return Ok(());
    };

    entry.insert("install_meta".to_string(), meta_val);

    write_manifest_atomic(&manifest_path, &mut payload)?;
    Ok(())
}

fn read_workspace_skill_config(
    workspace_dir: &Path,
    skill_name: &str,
) -> Option<Map<String, Value>> {
    let path = workspace_dir.join("skill.json");
    let bytes = std::fs::read(&path).ok()?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    let cfg = value
        .get("skills")?
        .get(skill_name)?
        .get("config")?
        .as_object()?
        .clone();
    if cfg.is_empty() {
        None
    } else {
        Some(cfg)
    }
}

fn packaged_builtin_version(name: &str) -> String {
    let dir = match crate::builtin::BUILTIN_SKILLS.get_dir(name) {
        Some(d) => d,
        None => return String::new(),
    };
    let skill_md = dir.entries().iter().find_map(|e| match e {
        include_dir::DirEntry::File(f) => f
            .path()
            .file_name()
            .and_then(|n| n.to_str())
            .filter(|n| *n == "SKILL.md")
            .map(|_| f),
        _ => None,
    });
    let Some(file) = skill_md else {
        return String::new();
    };
    let content = std::str::from_utf8(file.contents()).unwrap_or("");
    parse_skill_md_safe(content, name).version
}
