use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use include_dir::{include_dir, Dir, DirEntry};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::SkillError;
use crate::reconcile::{install_builtin_signature_provider, BuiltinSignatureProvider};
use crate::signature::is_ignored_skill_path;

pub static BUILTIN_SKILLS: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/builtin_skills");

pub fn builtin_skill_names() -> Vec<String> {
    let mut names: Vec<String> = BUILTIN_SKILLS
        .dirs()
        .filter(|d| has_skill_md(d))
        .filter_map(|d| {
            d.path()
                .file_name()
                .and_then(|s| s.to_str())
                .map(|s| s.to_string())
        })
        .collect();
    names.sort();
    names
}

fn has_skill_md(dir: &Dir<'_>) -> bool {
    dir.entries().iter().any(|e| match e {
        DirEntry::File(f) => f
            .path()
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n == "SKILL.md")
            .unwrap_or(false),
        _ => false,
    })
}

fn signature_cache() -> &'static BTreeMap<String, String> {
    static CACHE: OnceLock<BTreeMap<String, String>> = OnceLock::new();
    CACHE.get_or_init(|| {
        let mut out = BTreeMap::new();
        for name in builtin_skill_names() {
            if let Ok(sig) = compute_builtin_signature(&name) {
                out.insert(name, sig);
            }
        }
        out
    })
}

pub fn builtin_signature(name: &str) -> Option<String> {
    signature_cache().get(name).cloned()
}

pub fn builtin_signatures() -> BTreeMap<String, String> {
    signature_cache().clone()
}

fn compute_builtin_signature(name: &str) -> Result<String, SkillError> {
    let dir = BUILTIN_SKILLS
        .get_dir(name)
        .ok_or_else(|| SkillError::InvalidSkillName(name.to_string()))?;

    let mut files: Vec<&include_dir::File<'_>> = collect_files(dir);
    let prefix_len = dir.path().components().count();

    files.sort_by(|a, b| {
        let ra = relative_components(a.path(), prefix_len);
        let rb = relative_components(b.path(), prefix_len);
        ra.cmp(&rb)
    });

    let mut digest = Sha256::new();
    for file in files {
        let rel = relative_components(file.path(), prefix_len);
        let rel_path = PathBuf::from(rel.join("/"));
        if is_ignored_skill_path(&rel_path) {
            continue;
        }
        digest.update(rel.join("/").as_bytes());
        digest.update(file.contents());
    }
    Ok(hex_encode(&digest.finalize()))
}

fn collect_files<'a>(dir: &'a Dir<'a>) -> Vec<&'a include_dir::File<'a>> {
    let mut out = Vec::new();
    push_files(dir, &mut out);
    out
}

fn push_files<'a>(dir: &'a Dir<'a>, out: &mut Vec<&'a include_dir::File<'a>>) {
    for entry in dir.entries() {
        match entry {
            DirEntry::File(f) => out.push(f),
            DirEntry::Dir(d) => push_files(d, out),
        }
    }
}

fn relative_components(path: &Path, prefix_len: usize) -> Vec<String> {
    path.components()
        .skip(prefix_len)
        .filter_map(|c| c.as_os_str().to_str())
        .map(|s| s.to_string())
        .collect()
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

pub fn extract_builtin_to(name: &str, target: &Path) -> Result<(), SkillError> {
    let dir = BUILTIN_SKILLS
        .get_dir(name)
        .ok_or_else(|| SkillError::InvalidSkillName(name.to_string()))?;

    if target.exists() {
        std::fs::remove_dir_all(target)?;
    }
    std::fs::create_dir_all(target)?;

    let prefix_len = dir.path().components().count();
    write_dir_recursive(dir, target, prefix_len)
}

fn write_dir_recursive(
    dir: &Dir<'_>,
    target_root: &Path,
    prefix_len: usize,
) -> Result<(), SkillError> {
    for entry in dir.entries() {
        match entry {
            DirEntry::File(f) => {
                let rel = relative_components(f.path(), prefix_len);
                let rel_path = PathBuf::from(rel.join("/"));
                if is_ignored_skill_path(&rel_path) {
                    continue;
                }
                let dest = target_root.join(&rel_path);
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&dest, f.contents())?;
            }
            DirEntry::Dir(d) => {
                let rel = relative_components(d.path(), prefix_len);
                let rel_path = PathBuf::from(rel.join("/"));
                if is_ignored_skill_path(&rel_path) {
                    continue;
                }
                let dest = target_root.join(&rel_path);
                std::fs::create_dir_all(&dest)?;
                write_dir_recursive(d, target_root, prefix_len)?;
            }
        }
    }
    Ok(())
}

pub fn default_pool_dir() -> PathBuf {
    rua_config::dirs::rua_root().join("skill_pool")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BuiltinStatus {
    Missing,

    Current,

    Outdated,

    Conflict,
}

impl BuiltinStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            BuiltinStatus::Missing => "missing",
            BuiltinStatus::Current => "current",
            BuiltinStatus::Outdated => "outdated",
            BuiltinStatus::Conflict => "conflict",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuiltinCandidate {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub version_text: String,
    #[serde(default)]
    pub current_version_text: String,
    #[serde(default)]
    pub current_source: String,
    pub status: BuiltinStatus,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ImportBuiltinReport {
    pub imported: Vec<String>,
    pub updated: Vec<String>,
    pub unchanged: Vec<String>,
    pub conflicts: Vec<BuiltinCandidate>,
}

pub fn list_builtin_import_candidates() -> Result<Vec<BuiltinCandidate>, SkillError> {
    list_builtin_import_candidates_in(&default_pool_dir())
}

pub fn list_builtin_import_candidates_in(
    pool_dir: &Path,
) -> Result<Vec<BuiltinCandidate>, SkillError> {
    let pool_skills = read_pool_skills_map(pool_dir);
    let mut out = Vec::new();
    for name in builtin_skill_names() {
        let source_signature = builtin_signature(&name).unwrap_or_default();
        let parsed = parse_packaged_skill_md(&name);
        let current = pool_skills.get(&name).cloned();
        let (current_version_text, current_source, status) = match &current {
            None => (String::new(), String::new(), BuiltinStatus::Missing),
            Some(entry) => {
                let cur_sig = entry
                    .get("signature")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let cur_src = entry
                    .get("source")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let cur_ver = entry
                    .get("version_text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let st = if cur_src == "builtin" {
                    if cur_sig == source_signature {
                        BuiltinStatus::Current
                    } else {
                        BuiltinStatus::Outdated
                    }
                } else {
                    BuiltinStatus::Conflict
                };
                (cur_ver, cur_src, st)
            }
        };
        out.push(BuiltinCandidate {
            name,
            description: parsed.description,
            version_text: parsed.version,
            current_version_text,
            current_source,
            status,
        });
    }
    Ok(out)
}

pub fn import_builtin_skills(
    skill_names: Option<&[String]>,
    overwrite_conflicts: bool,
) -> Result<ImportBuiltinReport, SkillError> {
    import_builtin_skills_in(&default_pool_dir(), skill_names, overwrite_conflicts)
}

pub fn import_builtin_skills_in(
    pool_dir: &Path,
    skill_names: Option<&[String]>,
    overwrite_conflicts: bool,
) -> Result<ImportBuiltinReport, SkillError> {
    std::fs::create_dir_all(pool_dir)?;

    let candidates = list_builtin_import_candidates_in(pool_dir)?;
    let by_name: BTreeMap<String, BuiltinCandidate> = candidates
        .into_iter()
        .map(|c| (c.name.clone(), c))
        .collect();

    let selected: Vec<String> = match skill_names {
        Some(names) => {
            let mut v: Vec<String> = names.to_vec();
            v.sort();
            v.dedup();
            let unknown: Vec<String> = v
                .iter()
                .filter(|n| !by_name.contains_key(n.as_str()))
                .cloned()
                .collect();
            if !unknown.is_empty() {
                return Err(SkillError::InvalidSkillName(format!(
                    "unknown builtin skill(s): {}",
                    unknown.join(", ")
                )));
            }
            v
        }
        None => by_name.keys().cloned().collect(),
    };

    let conflicts: Vec<BuiltinCandidate> = selected
        .iter()
        .filter_map(|n| by_name.get(n).cloned())
        .filter(|c| c.status == BuiltinStatus::Conflict)
        .collect();

    if !conflicts.is_empty() && !overwrite_conflicts {
        return Ok(ImportBuiltinReport {
            imported: Vec::new(),
            updated: Vec::new(),
            unchanged: Vec::new(),
            conflicts,
        });
    }

    let mut report = ImportBuiltinReport {
        conflicts: conflicts.clone(),
        ..Default::default()
    };

    for name in &selected {
        let cand = match by_name.get(name) {
            Some(c) => c,
            None => continue,
        };
        let target = pool_dir.join(name);
        match cand.status {
            BuiltinStatus::Missing => {
                extract_builtin_to(name, &target)?;
                report.imported.push(name.clone());
            }
            BuiltinStatus::Outdated => {
                extract_builtin_to(name, &target)?;
                report.updated.push(name.clone());
            }
            BuiltinStatus::Current => {
                report.unchanged.push(name.clone());
            }
            BuiltinStatus::Conflict => {
                extract_builtin_to(name, &target)?;
                report.updated.push(name.clone());
            }
        }
    }

    install_default_builtin_signature_provider();
    crate::reconcile::reconcile_pool_manifest_with(pool_dir, &builtin_signatures())?;

    Ok(report)
}

pub fn update_single_builtin(name: &str) -> Result<BuiltinCandidate, SkillError> {
    update_single_builtin_in(&default_pool_dir(), name)
}

pub fn update_single_builtin_in(
    pool_dir: &Path,
    name: &str,
) -> Result<BuiltinCandidate, SkillError> {
    if builtin_signature(name).is_none() {
        return Err(SkillError::InvalidSkillName(format!(
            "'{name}' is not a builtin skill"
        )));
    }
    let candidates = list_builtin_import_candidates_in(pool_dir)?;
    let cand = candidates
        .into_iter()
        .find(|c| c.name == name)
        .ok_or_else(|| SkillError::InvalidSkillName(name.to_string()))?;

    if matches!(cand.status, BuiltinStatus::Conflict) {
        return Err(SkillError::Conflict(format!(
            "'{name}' is not a builtin pool skill (source = customized)"
        )));
    }

    let target = pool_dir.join(name);
    extract_builtin_to(name, &target)?;
    install_default_builtin_signature_provider();
    crate::reconcile::reconcile_pool_manifest_with(pool_dir, &builtin_signatures())?;

    let refreshed = list_builtin_import_candidates_in(pool_dir)?
        .into_iter()
        .find(|c| c.name == name)
        .ok_or_else(|| SkillError::InvalidSkillName(name.to_string()))?;
    Ok(refreshed)
}

pub fn ensure_skill_pool_initialized() -> Result<bool, SkillError> {
    ensure_skill_pool_initialized_in(&default_pool_dir())
}

pub fn ensure_skill_pool_initialized_in(pool_dir: &Path) -> Result<bool, SkillError> {
    let mut created = !pool_dir.exists();
    std::fs::create_dir_all(pool_dir)?;
    let manifest_path = pool_dir.join("skill.json");
    if !manifest_path.exists() {
        created = true;
    }

    install_default_builtin_signature_provider();

    if created {
        import_builtin_skills_in(pool_dir, None, false)?;
    }
    Ok(created)
}

fn read_pool_skills_map(pool_dir: &Path) -> BTreeMap<String, serde_json::Value> {
    let path = pool_dir.join("skill.json");
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(_) => return BTreeMap::new(),
    };
    let value: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(_) => return BTreeMap::new(),
    };
    let Some(obj) = value.as_object() else {
        return BTreeMap::new();
    };
    let Some(skills) = obj.get("skills").and_then(|v| v.as_object()) else {
        return BTreeMap::new();
    };
    skills.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
}

struct ParsedPackagedSkill {
    description: String,
    version: String,
}

fn parse_packaged_skill_md(name: &str) -> ParsedPackagedSkill {
    let dir = match BUILTIN_SKILLS.get_dir(name) {
        Some(d) => d,
        None => {
            return ParsedPackagedSkill {
                description: String::new(),
                version: String::new(),
            }
        }
    };
    let skill_md = dir.entries().iter().find_map(|e| match e {
        DirEntry::File(f) => f
            .path()
            .file_name()
            .and_then(|n| n.to_str())
            .filter(|n| *n == "SKILL.md")
            .map(|_| f),
        _ => None,
    });
    let Some(file) = skill_md else {
        return ParsedPackagedSkill {
            description: String::new(),
            version: String::new(),
        };
    };
    let content = std::str::from_utf8(file.contents()).unwrap_or("");
    let parsed = crate::frontmatter::parse_skill_md_safe(content, name);
    ParsedPackagedSkill {
        description: parsed.description,
        version: parsed.version,
    }
}

struct DefaultProvider;

impl BuiltinSignatureProvider for DefaultProvider {
    fn signatures(&self) -> BTreeMap<String, String> {
        builtin_signatures()
    }

    fn invalidate_cache(&self) {}
}

pub fn install_default_builtin_signature_provider() {
    install_builtin_signature_provider(Box::new(DefaultProvider));
}
