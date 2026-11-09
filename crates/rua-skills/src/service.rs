use std::collections::{BTreeMap, BTreeSet};
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Map, Value};
use tempfile::TempDir;
use zip::result::ZipError;
use zip::write::{SimpleFileOptions, ZipWriter};
use zip::{CompressionMethod, ZipArchive};

fn zip_err_to_skill(err: ZipError) -> SkillError {
    SkillError::InvalidFrontmatter(format!("zip error: {err}"))
}

use crate::android_runtime::scan_android_skill_runtime_value;
use crate::atomic::write_manifest_atomic;
use crate::channels::{workspace_skills_dir, ALL_SKILL_ROUTING_CHANNELS};
use crate::copy::{collect_skill_files, copy_skill_dir};
use crate::error::SkillError;
use crate::frontmatter::{parse_skill_md, parse_skill_md_safe, ParsedSkillMd};
use crate::install_meta::{write_install_meta, InstallMeta};
use crate::lock::{lock_path_for, ManifestLock};
use crate::manifest::WorkspaceManifest;
use crate::reconcile::reconcile_workspace_manifest;
use crate::scanner_integration::{maybe_scan_skill, ScannerHandle};
use crate::types::{SkillInfo, SkillSource};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveOutcome {
    NotFound,

    Conflict { suggested_name: String },

    Edit { name: String },

    Rename { name: String },

    Noop { name: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportConflict {
    pub skill_name: String,
    pub suggested_name: String,
}

pub struct SkillBundleWrite<'a> {
    pub name: &'a str,
    pub content: &'a str,
    pub references: &'a Map<String, Value>,
    pub scripts: &'a Map<String, Value>,
    pub extra_files: &'a Map<String, Value>,
    pub install_meta: Option<&'a InstallMeta>,
}

#[derive(Debug, Clone, Default)]
pub struct ImportOutcome {
    pub imported: Vec<String>,
    pub conflicts: Vec<ImportConflict>,
    pub enabled: bool,
}

#[derive(Debug, Clone, Default)]
pub struct ImportOptions {
    pub enable: bool,
    pub target_name: Option<String>,
    pub rename_map: BTreeMap<String, String>,
    pub overwrite: bool,
}

#[derive(Debug, Clone)]
pub struct SkillService {
    workspace_dir: PathBuf,
    scanner: Option<Arc<dyn ScannerHandle>>,
}

impl SkillService {
    pub fn new(workspace_dir: impl Into<PathBuf>) -> Result<Self, SkillError> {
        let workspace_dir = workspace_dir.into();
        std::fs::create_dir_all(&workspace_dir)?;
        Ok(Self {
            workspace_dir,
            scanner: None,
        })
    }

    pub fn with_scanner(mut self, scanner: Arc<dyn ScannerHandle>) -> Self {
        self.scanner = Some(scanner);
        self
    }

    pub fn scanner(&self) -> Option<&Arc<dyn ScannerHandle>> {
        self.scanner.as_ref()
    }

    pub fn workspace_dir(&self) -> &Path {
        &self.workspace_dir
    }

    pub fn list_all_skills(&self) -> Result<Vec<SkillInfo>, SkillError> {
        let manifest = self.read_manifest()?;
        let skills_root = workspace_skills_dir(&self.workspace_dir);
        let mut out = Vec::new();
        for (skill_name, entry) in manifest.skills.iter() {
            if let Some(info) =
                read_skill_from_dir(&skills_root.join(skill_name), entry.metadata.source.clone())?
            {
                out.push(info);
            }
        }
        Ok(out)
    }

    pub fn list_available_skills(&self) -> Result<Vec<SkillInfo>, SkillError> {
        let names = crate::channels::resolve_effective_skills(&self.workspace_dir, "console");
        if names.is_empty() {
            return Ok(Vec::new());
        }
        let manifest = self.read_manifest()?;
        let skills_root = workspace_skills_dir(&self.workspace_dir);
        let mut out = Vec::new();
        for skill_name in names {
            let source = manifest
                .skills
                .get(&skill_name)
                .map(|e| e.metadata.source.clone())
                .unwrap_or(SkillSource::Customized);
            if let Some(info) = read_skill_from_dir(&skills_root.join(&skill_name), source)? {
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
        let skills_root = workspace_skills_dir(&self.workspace_dir);
        read_skill_from_dir(&skills_root.join(name), entry.metadata.source.clone())
    }

    pub fn create_skill(
        &self,
        name: &str,
        content: &str,
        config: Option<Map<String, Value>>,
        enable: bool,
    ) -> Result<Option<String>, SkillError> {
        validate_skill_content(content)?;
        let skill_name = normalize_skill_dir_name(name)?;
        let skills_root = workspace_skills_dir(&self.workspace_dir);
        std::fs::create_dir_all(&skills_root)?;
        let skill_dir = skills_root.join(&skill_name);
        if skill_dir.exists() {
            return Ok(None);
        }

        let stage = stage_skill_dir(&skill_name)?;
        write_skill_md(&stage.skill_path, content)?;
        maybe_scan_skill(self.scanner.as_ref(), &stage.skill_path, &skill_name)?;
        copy_skill_dir(&stage.skill_path, &skill_dir)?;

        let workspace_dir = self.workspace_dir.clone();
        let final_name = skill_name.clone();
        mutate_workspace_manifest(&workspace_dir, |payload| {
            let skills = ensure_skills_map(payload);
            let prev = match skills.get(&final_name) {
                Some(Value::Object(o)) => Some(o.clone()),
                _ => None,
            };
            let metadata = build_skill_metadata(&final_name, &skill_dir, "customized")?;
            let channels = prev
                .as_ref()
                .and_then(|o| o.get("channels"))
                .cloned()
                .unwrap_or_else(|| Value::Array(vec![Value::String("all".to_string())]));
            let enabled_val = Value::Bool(
                prev.as_ref()
                    .and_then(|o| o.get("enabled"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(enable),
            );
            let config_val = if let Some(c) = config.as_ref() {
                Value::Object(c.clone())
            } else {
                prev.as_ref()
                    .and_then(|o| o.get("config"))
                    .cloned()
                    .unwrap_or_else(|| Value::Object(Map::new()))
            };
            let entry = build_workspace_entry(metadata, enabled_val, channels, config_val);
            skills.insert(final_name.clone(), Value::Object(entry));
            Ok(())
        })?;
        Ok(Some(skill_name))
    }

    pub fn create_skill_bundle(
        &self,
        bundle: SkillBundleWrite<'_>,
        enable: bool,
    ) -> Result<Option<String>, SkillError> {
        validate_skill_content(bundle.content)?;
        let skill_name = normalize_skill_dir_name(bundle.name)?;
        let skills_root = workspace_skills_dir(&self.workspace_dir);
        std::fs::create_dir_all(&skills_root)?;
        let skill_dir = skills_root.join(&skill_name);
        if skill_dir.exists() {
            return Ok(None);
        }

        let stage = stage_skill_dir(&skill_name)?;
        write_skill_md(&stage.skill_path, bundle.content)?;
        write_value_tree(&stage.skill_path.join("references"), bundle.references)?;
        write_value_tree(&stage.skill_path.join("scripts"), bundle.scripts)?;
        write_value_tree(&stage.skill_path, bundle.extra_files)?;
        if let Some(meta) = bundle.install_meta {
            write_install_meta(&stage.skill_path, meta)?;
        }
        maybe_scan_skill(self.scanner.as_ref(), &stage.skill_path, &skill_name)?;
        copy_skill_dir(&stage.skill_path, &skill_dir)?;

        let workspace_dir = self.workspace_dir.clone();
        let final_name = skill_name.clone();
        mutate_workspace_manifest(&workspace_dir, |payload| {
            let skills = ensure_skills_map(payload);
            let metadata = build_skill_metadata(&final_name, &skill_dir, "customized")?;
            let entry = build_workspace_entry(
                metadata,
                Value::Bool(enable),
                Value::Array(vec![Value::String("all".to_string())]),
                Value::Object(Map::new()),
            );
            skills.insert(final_name.clone(), Value::Object(entry));
            Ok(())
        })?;
        Ok(Some(skill_name))
    }

    pub fn save_skill(
        &self,
        skill_name: &str,
        content: &str,
        target_name: Option<&str>,
        config: Option<Map<String, Value>>,
        overwrite: bool,
    ) -> Result<SaveOutcome, SkillError> {
        validate_skill_content(content)?;
        let manifest = self.read_manifest()?;
        let Some(old_entry) = manifest.skills.get(skill_name).cloned() else {
            return Ok(SaveOutcome::NotFound);
        };

        let final_name = normalize_skill_dir_name(target_name.unwrap_or(skill_name))?;
        if final_name == skill_name {
            return self.save_skill_in_place(skill_name, content, config, &old_entry);
        }

        let skills_root = workspace_skills_dir(&self.workspace_dir);
        let target_dir = skills_root.join(&final_name);
        if target_dir.exists() && !overwrite {
            let existing: BTreeSet<String> = manifest.skills.keys().cloned().collect();
            return Ok(SaveOutcome::Conflict {
                suggested_name: suggest_conflict_name(&final_name, &existing),
            });
        }
        self.save_skill_as_rename(skill_name, &final_name, content, config, &old_entry)
    }

    fn save_skill_in_place(
        &self,
        skill_name: &str,
        content: &str,
        config: Option<Map<String, Value>>,
        old_entry: &crate::manifest::WorkspaceSkillEntry,
    ) -> Result<SaveOutcome, SkillError> {
        let skills_root = workspace_skills_dir(&self.workspace_dir);
        std::fs::create_dir_all(&skills_root)?;
        let skill_dir = skills_root.join(skill_name);
        let old_md_path = skill_dir.join("SKILL.md");
        let old_md = if old_md_path.exists() {
            std::fs::read_to_string(&old_md_path).unwrap_or_default()
        } else {
            String::new()
        };
        let content_changed = content != old_md;
        let new_config = config.clone().unwrap_or_else(|| old_entry.config.clone());
        let config_unchanged = new_config == old_entry.config;
        if !content_changed && config_unchanged {
            return Ok(SaveOutcome::Noop {
                name: skill_name.to_string(),
            });
        }

        if content_changed {
            let stage = stage_skill_dir(skill_name)?;
            if skill_dir.exists() {
                copy_skill_dir(&skill_dir, &stage.skill_path)?;
            }
            write_skill_md(&stage.skill_path, content)?;
            maybe_scan_skill(self.scanner.as_ref(), &stage.skill_path, skill_name)?;

            std::fs::create_dir_all(&skill_dir)?;
            std::fs::write(&old_md_path, content)?;
        }

        let workspace_dir = self.workspace_dir.clone();
        let final_name = skill_name.to_string();
        let owned_old = old_entry.clone();
        mutate_workspace_manifest(&workspace_dir, |payload| {
            let skills = ensure_skills_map(payload);
            let metadata = build_skill_metadata(&final_name, &skill_dir, "customized")?;
            let enabled = owned_old.enabled;
            let channels = if owned_old.channels.is_empty() {
                Value::Array(vec![Value::String("all".to_string())])
            } else {
                Value::Array(
                    owned_old
                        .channels
                        .iter()
                        .cloned()
                        .map(Value::String)
                        .collect(),
                )
            };
            let entry = build_workspace_entry(
                metadata,
                Value::Bool(enabled),
                channels,
                Value::Object(new_config.clone()),
            );
            skills.insert(final_name.clone(), Value::Object(entry));
            Ok(())
        })?;
        Ok(SaveOutcome::Edit {
            name: skill_name.to_string(),
        })
    }

    fn save_skill_as_rename(
        &self,
        skill_name: &str,
        final_name: &str,
        content: &str,
        config: Option<Map<String, Value>>,
        old_entry: &crate::manifest::WorkspaceSkillEntry,
    ) -> Result<SaveOutcome, SkillError> {
        let skills_root = workspace_skills_dir(&self.workspace_dir);
        let old_dir = skills_root.join(skill_name);
        let target_dir = skills_root.join(final_name);

        let stage = stage_skill_dir(final_name)?;
        if old_dir.exists() {
            copy_skill_dir(&old_dir, &stage.skill_path)?;
        }
        write_skill_md(&stage.skill_path, content)?;
        maybe_scan_skill(self.scanner.as_ref(), &stage.skill_path, final_name)?;
        copy_skill_dir(&stage.skill_path, &target_dir)?;

        let new_config = config.clone().unwrap_or_else(|| old_entry.config.clone());
        let workspace_dir = self.workspace_dir.clone();
        let owned_old = old_entry.clone();
        let from_name = skill_name.to_string();
        let to_name = final_name.to_string();
        mutate_workspace_manifest(&workspace_dir, |payload| {
            let skills = ensure_skills_map(payload);
            let metadata = build_skill_metadata(&to_name, &target_dir, "customized")?;
            let enabled = owned_old.enabled;
            let channels = if owned_old.channels.is_empty() {
                Value::Array(vec![Value::String("all".to_string())])
            } else {
                Value::Array(
                    owned_old
                        .channels
                        .iter()
                        .cloned()
                        .map(Value::String)
                        .collect(),
                )
            };
            let entry = build_workspace_entry(
                metadata,
                Value::Bool(enabled),
                channels,
                Value::Object(new_config.clone()),
            );
            skills.insert(to_name.clone(), Value::Object(entry));
            skills.remove(&from_name);
            Ok(())
        })?;

        if old_dir.exists() {
            std::fs::remove_dir_all(&old_dir)?;
        }

        Ok(SaveOutcome::Rename {
            name: final_name.to_string(),
        })
    }

    pub fn delete_skill(&self, name: &str) -> Result<bool, SkillError> {
        let manifest = self.read_manifest()?;
        let Some(entry) = manifest.skills.get(name) else {
            return Ok(false);
        };
        if entry.enabled {
            return Err(SkillError::Conflict(format!(
                "cannot delete enabled skill '{name}' — disable it first"
            )));
        }
        let skill_dir = workspace_skills_dir(&self.workspace_dir).join(name);
        if skill_dir.exists() {
            std::fs::remove_dir_all(&skill_dir)?;
        }
        let workspace_dir = self.workspace_dir.clone();
        let target = name.to_string();
        mutate_workspace_manifest(&workspace_dir, |payload| {
            if let Some(Value::Object(skills)) = payload.get_mut("skills") {
                skills.remove(&target);
            }
            Ok(())
        })?;
        Ok(true)
    }

    pub fn enable_skill(&self, name: &str) -> Result<bool, SkillError> {
        let skill_dir = workspace_skills_dir(&self.workspace_dir).join(name);
        if !skill_dir.exists() {
            return Ok(false);
        }
        maybe_scan_skill(self.scanner.as_ref(), &skill_dir, name)?;
        let target = name.to_string();
        let workspace_dir = self.workspace_dir.clone();
        let mut updated = false;
        mutate_workspace_manifest(&workspace_dir, |payload| {
            if let Some(Value::Object(skills)) = payload.get_mut("skills") {
                if let Some(Value::Object(entry)) = skills.get_mut(&target) {
                    entry.insert("enabled".to_string(), Value::Bool(true));
                    if !entry.get("channels").map(|v| v.is_array()).unwrap_or(false) {
                        entry.insert(
                            "channels".to_string(),
                            Value::Array(vec![Value::String("all".to_string())]),
                        );
                    }
                    updated = true;
                }
            }
            Ok(())
        })?;
        Ok(updated)
    }

    pub fn disable_skill(&self, name: &str) -> Result<bool, SkillError> {
        let target = name.to_string();
        let workspace_dir = self.workspace_dir.clone();
        let mut updated = false;
        mutate_workspace_manifest(&workspace_dir, |payload| {
            if let Some(Value::Object(skills)) = payload.get_mut("skills") {
                if let Some(Value::Object(entry)) = skills.get_mut(&target) {
                    entry.insert("enabled".to_string(), Value::Bool(false));
                    updated = true;
                }
            }
            Ok(())
        })?;
        Ok(updated)
    }

    pub fn set_skill_channels(
        &self,
        name: &str,
        channels: Vec<String>,
    ) -> Result<bool, SkillError> {
        let normalised = if channels.is_empty() {
            vec!["all".to_string()]
        } else {
            for c in &channels {
                if c != "all" && !ALL_SKILL_ROUTING_CHANNELS.contains(&c.as_str()) {
                    return Err(SkillError::InvalidSkillName(format!(
                        "unknown channel '{c}' (allowed: 'all' or one of {ALL_SKILL_ROUTING_CHANNELS:?})"
                    )));
                }
            }
            channels
        };
        let target = name.to_string();
        let workspace_dir = self.workspace_dir.clone();
        let mut updated = false;
        mutate_workspace_manifest(&workspace_dir, |payload| {
            if let Some(Value::Object(skills)) = payload.get_mut("skills") {
                if let Some(Value::Object(entry)) = skills.get_mut(&target) {
                    entry.insert(
                        "channels".to_string(),
                        Value::Array(normalised.iter().cloned().map(Value::String).collect()),
                    );
                    updated = true;
                }
            }
            Ok(())
        })?;
        Ok(updated)
    }

    pub fn set_skill_config(
        &self,
        name: &str,
        config: Map<String, Value>,
    ) -> Result<bool, SkillError> {
        let target = name.to_string();
        let workspace_dir = self.workspace_dir.clone();
        let mut updated = false;
        mutate_workspace_manifest(&workspace_dir, |payload| {
            if let Some(Value::Object(skills)) = payload.get_mut("skills") {
                if let Some(Value::Object(entry)) = skills.get_mut(&target) {
                    entry.insert("config".to_string(), Value::Object(config.clone()));
                    updated = true;
                }
            }
            Ok(())
        })?;
        Ok(updated)
    }

    pub fn import_from_zip(&self, data: &[u8], enable: bool) -> Result<ImportOutcome, SkillError> {
        self.import_from_zip_with_options(
            data,
            ImportOptions {
                enable,
                ..Default::default()
            },
        )
    }

    pub fn import_from_zip_with_options(
        &self,
        data: &[u8],
        options: ImportOptions,
    ) -> Result<ImportOutcome, SkillError> {
        let skills_root = workspace_skills_dir(&self.workspace_dir);
        std::fs::create_dir_all(&skills_root)?;

        let extracted = extract_zip_to_tempdir(data)?;
        let found = discover_skills_in_extracted(extracted.path())?;
        if found.is_empty() {
            return Err(SkillError::InvalidFrontmatter(
                "no valid skills found in zip".to_string(),
            ));
        }
        if options.target_name.is_some() && found.len() != 1 {
            return Err(SkillError::InvalidSkillName(
                "target_name requires a single-skill archive".to_string(),
            ));
        }

        let existing_on_disk: BTreeSet<String> = if skills_root.exists() {
            std::fs::read_dir(&skills_root)?
                .filter_map(|e| e.ok())
                .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
                .filter_map(|e| e.file_name().into_string().ok())
                .collect()
        } else {
            BTreeSet::new()
        };

        let mut conflicts = Vec::new();
        let mut planned: Vec<(PathBuf, String)> = Vec::new();
        let mut seen = BTreeSet::new();
        for (src_dir, discovered_name) in found {
            let discovered_name = normalize_skill_dir_name(&discovered_name)?;
            let requested_name = options
                .rename_map
                .get(&discovered_name)
                .cloned()
                .or_else(|| options.target_name.clone())
                .unwrap_or_else(|| discovered_name.clone());
            let normalised = normalize_skill_dir_name(&requested_name)?;
            maybe_scan_skill(self.scanner.as_ref(), &src_dir, &normalised)?;

            let md_path = src_dir.join("SKILL.md");
            let content = std::fs::read_to_string(&md_path)?;
            validate_skill_content(&content)?;
            if !seen.insert(normalised.clone()) {
                conflicts.push(ImportConflict {
                    skill_name: normalised.clone(),
                    suggested_name: suggest_conflict_name(&normalised, &existing_on_disk),
                });
                continue;
            }
            if !options.overwrite
                && (skills_root.join(&normalised).exists()
                    || existing_on_disk.contains(&normalised))
            {
                conflicts.push(ImportConflict {
                    skill_name: normalised.clone(),
                    suggested_name: suggest_conflict_name(&normalised, &existing_on_disk),
                });
                continue;
            }
            planned.push((src_dir, normalised));
        }

        if !conflicts.is_empty() {
            return Ok(ImportOutcome {
                imported: Vec::new(),
                conflicts,
                enabled: false,
            });
        }

        let mut imported = Vec::new();
        for (src_dir, skill_name) in planned {
            let target_dir = skills_root.join(&skill_name);
            if options.overwrite && target_dir.exists() {
                std::fs::remove_dir_all(&target_dir)?;
            }
            copy_skill_dir(&src_dir, &target_dir)?;
            imported.push(skill_name);
        }

        if !imported.is_empty() {
            reconcile_workspace_manifest(&self.workspace_dir)?;
            if options.enable {
                for name in &imported {
                    self.enable_skill(name)?;
                }
            }
        }

        Ok(ImportOutcome {
            enabled: options.enable && !imported.is_empty(),
            imported,
            conflicts: Vec::new(),
        })
    }

    pub fn export_to_zip(&self, names: &[&str]) -> Result<Vec<u8>, SkillError> {
        let skills_root = workspace_skills_dir(&self.workspace_dir);
        let buffer: Vec<u8> = Vec::new();
        let mut writer = ZipWriter::new(Cursor::new(buffer));
        let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

        for name in names {
            let skill_dir = skills_root.join(name);
            if !skill_dir.exists() {
                return Err(SkillError::InvalidSkillName(format!(
                    "skill '{name}' not found"
                )));
            }
            for rel in collect_skill_files(&skill_dir)? {
                let zip_path = format!(
                    "{}/{}",
                    name,
                    rel.components()
                        .filter_map(|c| c.as_os_str().to_str())
                        .collect::<Vec<_>>()
                        .join("/")
                );
                writer
                    .start_file(zip_path, opts)
                    .map_err(zip_err_to_skill)?;
                let bytes = std::fs::read(skill_dir.join(&rel))?;
                writer.write_all(&bytes)?;
            }
        }

        let cursor = writer.finish().map_err(zip_err_to_skill)?;
        Ok(cursor.into_inner())
    }

    fn read_manifest(&self) -> Result<WorkspaceManifest, SkillError> {
        let path = self.workspace_dir.join("skill.json");
        if !path.exists() {
            return Ok(WorkspaceManifest::default());
        }
        let bytes = std::fs::read(&path)?;
        if bytes.is_empty() {
            return Ok(WorkspaceManifest::default());
        }
        let manifest: WorkspaceManifest = serde_json::from_slice(&bytes).unwrap_or_else(|err| {
            tracing::warn!(
                "Malformed workspace manifest at {}: {err}, using default",
                path.display()
            );
            WorkspaceManifest::default()
        });
        Ok(manifest)
    }
}

fn validate_skill_content(content: &str) -> Result<(String, String), SkillError> {
    let parsed = parse_skill_md(content)?;
    let name = parsed.name.trim().to_string();
    let description = parsed.description.trim().to_string();
    if name.is_empty() || description.is_empty() {
        return Err(SkillError::InvalidFrontmatter(
            "SKILL.md must include non-empty frontmatter `name` and `description`".to_string(),
        ));
    }
    Ok((name, description))
}

fn normalize_skill_dir_name(name: &str) -> Result<String, SkillError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(SkillError::InvalidSkillName(
            "skill name cannot be empty".to_string(),
        ));
    }
    if trimmed.contains('\0') {
        return Err(SkillError::InvalidSkillName(
            "skill name cannot contain NUL bytes".to_string(),
        ));
    }
    if trimmed == "." || trimmed == ".." {
        return Err(SkillError::InvalidSkillName(format!(
            "invalid skill name: {trimmed}"
        )));
    }
    if trimmed.contains('/') || trimmed.contains('\\') {
        return Err(SkillError::InvalidSkillName(
            "skill name cannot contain path separators".to_string(),
        ));
    }
    Ok(trimmed.to_string())
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

fn build_skill_metadata(
    skill_name: &str,
    skill_dir: &Path,
    source: &str,
) -> Result<Map<String, Value>, SkillError> {
    let parsed: ParsedSkillMd = parse_skill_md_for(skill_dir, skill_name);
    let mut m = Map::new();
    m.insert("name".to_string(), Value::String(skill_name.to_string()));
    m.insert("description".to_string(), Value::String(parsed.description));
    m.insert("version_text".to_string(), Value::String(parsed.version));
    m.insert("commit_text".to_string(), Value::String(String::new()));
    m.insert("signature".to_string(), Value::String(String::new()));
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

fn parse_skill_md_for(skill_dir: &Path, skill_name: &str) -> ParsedSkillMd {
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
    let Some(t) = mtime else { return String::new() };
    let dt: DateTime<Utc> = t.into();
    dt.to_rfc3339_opts(SecondsFormat::Micros, true)
}

fn build_workspace_entry(
    metadata: Map<String, Value>,
    enabled: Value,
    channels: Value,
    config: Value,
) -> Map<String, Value> {
    let requirements = metadata
        .get("requirements")
        .cloned()
        .unwrap_or(Value::Object(Map::new()));
    let updated_at = metadata
        .get("updated_at")
        .cloned()
        .unwrap_or(Value::String(String::new()));
    let source = metadata
        .get("source")
        .cloned()
        .unwrap_or(Value::String("customized".to_string()));
    let mut out = Map::new();
    out.insert("enabled".to_string(), enabled);
    out.insert("channels".to_string(), channels);
    out.insert("source".to_string(), source);
    out.insert("config".to_string(), config);
    out.insert("metadata".to_string(), Value::Object(metadata));
    out.insert("requirements".to_string(), requirements);
    out.insert("updated_at".to_string(), updated_at);
    out
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

fn mutate_workspace_manifest<F>(workspace_dir: &Path, f: F) -> Result<(), SkillError>
where
    F: FnOnce(&mut Map<String, Value>) -> Result<(), SkillError>,
{
    let manifest_path = workspace_dir.join("skill.json");
    if let Some(parent) = manifest_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let _lock = ManifestLock::acquire(&lock_path_for(&manifest_path))?;
    let mut payload = read_manifest_payload(&manifest_path);
    f(&mut payload)?;
    write_manifest_atomic(&manifest_path, &mut payload)?;
    Ok(())
}

fn read_manifest_payload(path: &Path) -> Map<String, Value> {
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

fn suggest_conflict_name(skill_name: &str, taken: &BTreeSet<String>) -> String {
    use chrono::Utc;

    let stripped = strip_timestamp_suffix(skill_name);
    let base_ts = Utc::now().format("%Y%m%d%H%M%S").to_string();
    let mut candidate = format!("{stripped}-{base_ts}");
    let mut counter = 1u32;
    while taken.contains(&candidate) {
        candidate = format!("{stripped}-{base_ts}-{counter}");
        counter += 1;
        if counter > 100 {
            break;
        }
    }
    candidate
}

fn strip_timestamp_suffix(name: &str) -> String {
    let mut current = name.to_string();
    while let Some(idx) = current.rfind('-') {
        let tail = &current[idx + 1..];
        if tail.len() == 14 && tail.chars().all(|c| c.is_ascii_digit()) {
            current.truncate(idx);
        } else {
            break;
        }
    }
    if current.is_empty() {
        name.to_string()
    } else {
        current
    }
}

struct StagedSkill {
    _temp: TempDir,
    skill_path: PathBuf,
}

fn stage_skill_dir(skill_name: &str) -> Result<StagedSkill, SkillError> {
    let temp = tempfile::Builder::new()
        .prefix(&format!("rua_skill_stage_{skill_name}_"))
        .tempdir()?;
    let skill_path = temp.path().join(skill_name);
    std::fs::create_dir_all(&skill_path)?;
    Ok(StagedSkill {
        _temp: temp,
        skill_path,
    })
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

fn extract_zip_to_tempdir(data: &[u8]) -> Result<TempDir, SkillError> {
    let temp = tempfile::Builder::new()
        .prefix("rua_skill_upload_")
        .tempdir()?;
    let mut archive = ZipArchive::new(Cursor::new(data)).map_err(zip_err_to_skill)?;
    let root = temp.path().to_path_buf();
    for i in 0..archive.len() {
        let mut file = archive.by_index(i).map_err(zip_err_to_skill)?;

        let Some(rel_path) = file.enclosed_name() else {
            continue;
        };
        let outpath = root.join(rel_path);
        if file.is_dir() {
            std::fs::create_dir_all(&outpath)?;
            continue;
        }
        if let Some(parent) = outpath.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut out = std::fs::File::create(&outpath)?;
        let mut buf = Vec::new();
        file.read_to_end(&mut buf)?;
        out.write_all(&buf)?;
    }
    Ok(temp)
}

fn discover_skills_in_extracted(root: &Path) -> Result<Vec<(PathBuf, String)>, SkillError> {
    let real_entries: Vec<PathBuf> = std::fs::read_dir(root)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| !crate::signature::IGNORED_ARTIFACTS.contains(&n))
                .unwrap_or(false)
        })
        .collect();

    let extract_root = if real_entries.len() == 1 && real_entries[0].is_dir() {
        real_entries[0].clone()
    } else {
        root.to_path_buf()
    };

    if extract_root.join("SKILL.md").exists() {
        let resolved = resolve_zip_skill_name(&extract_root);
        return Ok(vec![(extract_root, resolved)]);
    }

    let mut found = Vec::new();
    let mut entries: Vec<_> = std::fs::read_dir(&extract_root)?
        .filter_map(|e| e.ok())
        .collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if crate::signature::IGNORED_ARTIFACTS.contains(&name) {
            continue;
        }
        if !path.is_dir() {
            continue;
        }
        if !path.join("SKILL.md").exists() {
            continue;
        }
        let resolved = resolve_zip_skill_name(&path);
        found.push((path, resolved));
    }
    Ok(found)
}

fn resolve_zip_skill_name(skill_dir: &Path) -> String {
    let dir_name = skill_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_string();
    let parsed = parse_skill_md_for(skill_dir, &dir_name);
    if !parsed.name.trim().is_empty() {
        parsed.name.trim().to_string()
    } else {
        dir_name
    }
}
