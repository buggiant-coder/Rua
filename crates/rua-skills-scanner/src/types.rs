use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "UPPERCASE")]
pub enum Severity {
    Safe,
    Info,
    Low,
    Medium,
    High,
    Critical,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Safe => "SAFE",
            Severity::Info => "INFO",
            Severity::Low => "LOW",
            Severity::Medium => "MEDIUM",
            Severity::High => "HIGH",
            Severity::Critical => "CRITICAL",
        }
    }

    fn rank(self) -> u8 {
        match self {
            Severity::Safe => 0,
            Severity::Info => 1,
            Severity::Low => 2,
            Severity::Medium => 3,
            Severity::High => 4,
            Severity::Critical => 5,
        }
    }

    pub fn max(self, other: Severity) -> Severity {
        if self.rank() >= other.rank() {
            self
        } else {
            other
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ThreatCategory {
    PromptInjection,
    CommandInjection,
    DataExfiltration,
    UnauthorizedToolUse,
    Obfuscation,
    HardcodedSecrets,
    SocialEngineering,
    ResourceAbuse,
    PolicyViolation,
    Malware,
    HarmfulContent,
    SkillDiscoveryAbuse,
    TransitiveTrustAbuse,
    AutonomyAbuse,
    ToolChainingAbuse,
    UnicodeSteganography,
    SupplyChainAttack,
}

impl ThreatCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            ThreatCategory::PromptInjection => "prompt_injection",
            ThreatCategory::CommandInjection => "command_injection",
            ThreatCategory::DataExfiltration => "data_exfiltration",
            ThreatCategory::UnauthorizedToolUse => "unauthorized_tool_use",
            ThreatCategory::Obfuscation => "obfuscation",
            ThreatCategory::HardcodedSecrets => "hardcoded_secrets",
            ThreatCategory::SocialEngineering => "social_engineering",
            ThreatCategory::ResourceAbuse => "resource_abuse",
            ThreatCategory::PolicyViolation => "policy_violation",
            ThreatCategory::Malware => "malware",
            ThreatCategory::HarmfulContent => "harmful_content",
            ThreatCategory::SkillDiscoveryAbuse => "skill_discovery_abuse",
            ThreatCategory::TransitiveTrustAbuse => "transitive_trust_abuse",
            ThreatCategory::AutonomyAbuse => "autonomy_abuse",
            ThreatCategory::ToolChainingAbuse => "tool_chaining_abuse",
            ThreatCategory::UnicodeSteganography => "unicode_steganography",
            ThreatCategory::SupplyChainAttack => "supply_chain_attack",
        }
    }
}

#[derive(Debug, Clone)]
pub struct SkillFile {
    pub path: PathBuf,
    pub relative_path: String,
    pub file_type: String,
    pub size_bytes: u64,
    pub content: Option<String>,
}

impl SkillFile {
    pub fn from_path(path: &Path, base_dir: &Path) -> Self {
        let rel = path
            .strip_prefix(base_dir)
            .map(|p| {
                p.components()
                    .filter_map(|c| c.as_os_str().to_str())
                    .collect::<Vec<_>>()
                    .join("/")
            })
            .unwrap_or_else(|_| {
                path.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default()
                    .to_string()
            });
        let file_type = file_type_for(path);
        let size_bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        Self {
            path: path.to_path_buf(),
            relative_path: rel,
            file_type,
            size_bytes,
            content: None,
        }
    }

    pub fn read_content(&mut self) -> &str {
        if self.content.is_none() {
            let bytes = std::fs::read(&self.path).unwrap_or_default();
            let text = match std::str::from_utf8(&bytes) {
                Ok(s) => s.to_string(),
                Err(_) => String::from_utf8_lossy(&bytes).into_owned(),
            };
            self.content = Some(text);
        }
        self.content.as_deref().unwrap_or("")
    }

    pub fn is_hidden(&self) -> bool {
        Path::new(&self.relative_path)
            .components()
            .any(|component| {
                component
                    .as_os_str()
                    .to_str()
                    .is_some_and(|part| part != "." && part.starts_with('.'))
            })
    }
}

fn file_type_for(path: &Path) -> String {
    let suffix = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();
    match suffix.as_str() {
        "md" | "markdown" => "markdown",
        "py" => "python",
        "sh" | "bash" | "zsh" => "bash",
        "js" => "javascript",
        "ts" => "typescript",
        "yaml" | "yml" => "yaml",
        "json" => "json",
        "toml" => "toml",
        _ => "other",
    }
    .to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    pub id: String,
    pub rule_id: String,
    pub category: ThreatCategory,
    pub severity: Severity,
    pub title: String,
    pub description: String,
    #[serde(default)]
    pub file_path: Option<String>,
    #[serde(default)]
    pub line_number: Option<u32>,
    #[serde(default)]
    pub snippet: Option<String>,
    #[serde(default)]
    pub remediation: Option<String>,
    #[serde(default)]
    pub analyzer: Option<String>,
    #[serde(default)]
    pub metadata: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalyzerFailure {
    pub analyzer: String,
    pub error: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanResult {
    pub skill_name: String,
    pub skill_directory: String,
    #[serde(default)]
    pub findings: Vec<Finding>,
    #[serde(default)]
    pub scan_duration_seconds: f64,
    #[serde(default)]
    pub analyzers_used: Vec<String>,
    #[serde(default)]
    pub analyzers_failed: Vec<AnalyzerFailure>,

    pub timestamp: String,
    pub is_safe: bool,
    pub max_severity: Severity,
}

impl ScanResult {
    pub fn compute(
        skill_name: String,
        skill_directory: String,
        findings: Vec<Finding>,
        scan_duration_seconds: f64,
        analyzers_used: Vec<String>,
        analyzers_failed: Vec<AnalyzerFailure>,
    ) -> Self {
        let max_severity = findings
            .iter()
            .map(|f| f.severity)
            .fold(Severity::Safe, Severity::max);
        let is_safe = !findings
            .iter()
            .any(|f| matches!(f.severity, Severity::Critical | Severity::High));
        let timestamp = chrono::Utc::now().to_rfc3339();
        Self {
            skill_name,
            skill_directory,
            findings,
            scan_duration_seconds,
            analyzers_used,
            analyzers_failed,
            timestamp,
            is_safe,
            max_severity,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanMode {
    Off,
    Warn,
    Block,
}

impl ScanMode {
    pub fn parse(s: &str) -> Option<ScanMode> {
        match s.trim().to_lowercase().as_str() {
            "off" => Some(ScanMode::Off),
            "warn" => Some(ScanMode::Warn),
            "block" => Some(ScanMode::Block),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, thiserror::Error)]
#[error("scan blocked: {message}")]
pub struct SkillScanError {
    pub message: String,
    pub findings: Vec<Finding>,
}

impl SkillScanError {
    pub fn from_result(result: &ScanResult) -> Self {
        let summary = result
            .findings
            .iter()
            .take(5)
            .map(|f| {
                let loc = match (f.file_path.as_deref(), f.line_number) {
                    (Some(p), Some(l)) => format!("({p}:{l})"),
                    (Some(p), None) => format!("({p})"),
                    _ => String::new(),
                };
                format!("[{}] {} {loc}", f.severity.as_str(), f.title)
            })
            .collect::<Vec<_>>()
            .join("; ");
        let truncated = if result.findings.len() > 5 {
            format!(" (and {} more)", result.findings.len() - 5)
        } else {
            String::new()
        };
        let message = format!(
            "Security scan of skill '{}' found {} issue(s) (max severity: {}): {summary}{truncated}",
            result.skill_name,
            result.findings.len(),
            result.max_severity.as_str(),
        );
        Self {
            message,
            findings: result.findings.clone(),
        }
    }
}
