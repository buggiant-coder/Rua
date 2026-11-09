use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::types::Severity;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileLimits {
    pub max_file_count: usize,
    pub max_file_size_bytes: u64,
}

impl Default for FileLimits {
    fn default() -> Self {
        Self {
            max_file_count: 500,
            max_file_size_bytes: 10 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RuleScoping {
    pub skip_in_docs: HashSet<String>,

    pub code_only: HashSet<String>,

    pub doc_path_indicators: HashSet<String>,

    pub dedupe_duplicate_findings: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CredentialsPolicy {
    pub known_test_values: HashSet<String>,
    pub placeholder_markers: HashSet<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FileClassPolicy {
    pub inert_extensions: HashSet<String>,
    pub archive_extensions: HashSet<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanPolicy {
    pub rule_scoping: RuleScoping,
    pub credentials: CredentialsPolicy,
    pub file_classification: FileClassPolicy,
    pub file_limits: FileLimits,
    pub severity_overrides: HashMap<String, Severity>,
    pub disabled_rules: HashSet<String>,
}

impl Default for ScanPolicy {
    fn default() -> Self {
        let dedupe = true;
        let doc_indicators: HashSet<String> = ["docs", "doc", "examples", "tests", "test"]
            .into_iter()
            .map(|s| s.to_string())
            .collect();
        Self {
            rule_scoping: RuleScoping {
                skip_in_docs: HashSet::new(),
                code_only: HashSet::new(),
                doc_path_indicators: doc_indicators,
                dedupe_duplicate_findings: dedupe,
            },
            credentials: CredentialsPolicy {
                known_test_values: [
                    "AKIAIOSFODNN7EXAMPLE",
                    "AKIAI44QH8DHBEXAMPLE",
                    "EXAMPLEKEYID",
                ]
                .into_iter()
                .map(|s| s.to_string())
                .collect(),
                placeholder_markers: [
                    "example",
                    "placeholder",
                    "your-",
                    "your_",
                    "fake",
                    "dummy",
                    "test_",
                ]
                .into_iter()
                .map(|s| s.to_string())
                .collect(),
            },
            file_classification: FileClassPolicy {
                inert_extensions: [
                    ".png", ".jpg", ".jpeg", ".gif", ".bmp", ".ico", ".svg", ".webp", ".woff",
                    ".woff2", ".eot", ".ttf", ".otf",
                ]
                .into_iter()
                .map(|s| s.to_string())
                .collect(),
                archive_extensions: [".zip", ".tar", ".gz", ".bz2", ".xz", ".7z", ".rar"]
                    .into_iter()
                    .map(|s| s.to_string())
                    .collect(),
            },
            file_limits: FileLimits::default(),
            severity_overrides: HashMap::new(),
            disabled_rules: HashSet::new(),
        }
    }
}

impl ScanPolicy {
    pub fn is_rule_disabled(&self, rule_id: &str) -> bool {
        self.disabled_rules.contains(rule_id)
    }

    pub fn get_severity_override(&self, rule_id: &str) -> Option<Severity> {
        self.severity_overrides.get(rule_id).copied()
    }

    pub fn is_doc_path(&self, rel_path: &str) -> bool {
        if self.rule_scoping.doc_path_indicators.is_empty() {
            return false;
        }
        for part in Path::new(rel_path).components() {
            if let Some(s) = part.as_os_str().to_str() {
                if self
                    .rule_scoping
                    .doc_path_indicators
                    .contains(&s.to_lowercase())
                {
                    return true;
                }
            }
        }
        false
    }

    pub fn should_skip_extension(&self, ext_with_dot: &str) -> bool {
        let lower = ext_with_dot.to_lowercase();
        self.file_classification.inert_extensions.contains(&lower)
            || self.file_classification.archive_extensions.contains(&lower)
    }

    pub fn from_file(path: &Path) -> Result<ScanPolicy, ScanPolicyError> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| ScanPolicyError::Io(path.to_path_buf(), e))?;
        Self::from_yaml_str(&content)
            .map_err(|e| ScanPolicyError::Merge(path.to_path_buf(), e.to_string()))
    }

    fn from_yaml_str(content: &str) -> Result<ScanPolicy, String> {
        let user_yaml: serde_yaml::Value =
            serde_yaml::from_str(content).map_err(|e| e.to_string())?;
        if user_yaml.is_null() {
            return Ok(ScanPolicy::default());
        }
        let base = serde_json::to_value(ScanPolicy::default()).map_err(|e| e.to_string())?;
        let user = serde_json::to_value(user_yaml).map_err(|e| e.to_string())?;
        let merged = merge_json_values(base, user);
        serde_json::from_value(merged).map_err(|e| e.to_string())
    }

    pub fn from_preset(name: &str) -> Result<ScanPolicy, ScanPolicyError> {
        let yaml = match name.to_lowercase().as_str() {
            "paranoid" => include_str!("../presets/paranoid.yaml"),
            "balanced" => include_str!("../presets/balanced.yaml"),
            "permissive" => include_str!("../presets/permissive.yaml"),
            _ => {
                return Err(ScanPolicyError::UnknownPreset(name.to_string()));
            }
        };
        serde_yaml::from_str(yaml).map_err(|e| ScanPolicyError::PresetParse(name.to_string(), e))
    }

    pub fn preset_names() -> &'static [&'static str] {
        &["paranoid", "balanced", "permissive"]
    }
}

#[derive(Debug)]
pub enum ScanPolicyError {
    Io(std::path::PathBuf, std::io::Error),
    Parse(std::path::PathBuf, serde_yaml::Error),
    Merge(std::path::PathBuf, String),
    UnknownPreset(String),
    PresetParse(String, serde_yaml::Error),
}

impl std::fmt::Display for ScanPolicyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(path, e) => write!(f, "failed to read policy file {}: {}", path.display(), e),
            Self::Parse(path, e) => {
                write!(f, "failed to parse policy file {}: {}", path.display(), e)
            }
            Self::Merge(path, e) => {
                write!(f, "failed to merge policy file {}: {}", path.display(), e)
            }
            Self::UnknownPreset(name) => write!(
                f,
                "unknown preset '{}'; available: paranoid, balanced, permissive",
                name
            ),
            Self::PresetParse(name, e) => {
                write!(f, "failed to parse preset '{}': {}", name, e)
            }
        }
    }
}

impl std::error::Error for ScanPolicyError {}

fn merge_json_values(base: serde_json::Value, overlay: serde_json::Value) -> serde_json::Value {
    use serde_json::Value;
    match (base, overlay) {
        (Value::Object(mut base_map), Value::Object(overlay_map)) => {
            for (key, overlay_val) in overlay_map {
                let merged = match base_map.remove(&key) {
                    Some(base_val) => merge_json_values(base_val, overlay_val),
                    None => overlay_val,
                };
                base_map.insert(key, merged);
            }
            Value::Object(base_map)
        }
        (_, overlay) => overlay,
    }
}
