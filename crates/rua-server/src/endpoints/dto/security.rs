use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AllowNoAuthHostsConfig {
    #[serde(default)]
    pub hosts: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolGuardConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub custom_rules: Vec<CustomRule>,
    #[serde(default)]
    pub disabled_rules: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_mode: Option<String>,
    #[serde(default)]
    pub file_guard: Option<serde_json::Value>,
    #[serde(default)]
    pub rule_guard: Option<serde_json::Value>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalModeConfig {
    pub approval_mode: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomRule {
    pub id: String,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub severity: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub remediation: String,
    #[serde(default)]
    pub command_set: Vec<String>,
    #[serde(default)]
    pub arg_matchers: Vec<CustomArgMatcherDto>,
    #[serde(default)]
    pub path_matchers: Vec<CustomPathMatcherDto>,
    #[serde(default)]
    pub lexical: Vec<String>,
    #[serde(default)]
    pub structural: Vec<String>,
    #[serde(default)]
    pub action: String,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomArgMatcherDto {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_contains: Option<String>,
    #[serde(default)]
    pub value_in: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomPathMatcherDto {
    #[serde(default)]
    pub contains: Vec<String>,
    #[serde(default)]
    pub prefix: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub glob: Option<String>,
    #[serde(default)]
    pub on_redirect: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuiltinRule {
    pub id: String,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub severity: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub remediation: String,
    #[serde(default)]
    pub rule_type: String,
    #[serde(default)]
    pub command_set: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub arg_matchers: Vec<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub path_matchers: Vec<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lexical: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub structural: Vec<String>,
    #[serde(default)]
    pub action: String,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileGuardConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub paths: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub builtin_paths: Vec<String>,
    #[serde(default)]
    pub disabled_patterns: Vec<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillScannerConfig {
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub timeout: Option<u64>,
    #[serde(default)]
    pub whitelist: Vec<SkillWhitelistEntry>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillWhitelistEntry {
    pub skill_name: String,
    #[serde(default)]
    pub content_hash: String,
    #[serde(default)]
    pub added_at: String,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockedSkillFinding {
    #[serde(default)]
    pub severity: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub file_path: String,
    #[serde(default)]
    pub line_number: Option<u64>,
    #[serde(default)]
    pub rule_id: String,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockedHistoryEntry {
    #[serde(default)]
    pub skill_name: String,
    #[serde(default)]
    pub blocked_at: String,
    #[serde(default)]
    pub max_severity: String,
    #[serde(default)]
    pub findings: Vec<BlockedSkillFinding>,
    #[serde(default)]
    pub content_hash: String,
    #[serde(default)]
    pub action: String,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}
