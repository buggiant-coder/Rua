use crate::serde_helpers::deserialize_null_default;
use serde::{Deserialize, Serialize};

fn default_allow_no_auth_hosts() -> Vec<String> {
    vec!["127.0.0.1".to_string(), "::1".to_string()]
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SecurityConfig {
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub tool_guard: ToolGuardConfig,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub file_guard: FileGuardConfig,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub skill_scanner: SkillScannerConfig,
    #[serde(
        default = "default_allow_no_auth_hosts",
        deserialize_with = "deserialize_null_default"
    )]
    pub allow_no_auth_hosts: Vec<String>,

    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            tool_guard: ToolGuardConfig::default(),
            file_guard: FileGuardConfig::default(),
            skill_scanner: SkillScannerConfig::default(),
            allow_no_auth_hosts: default_allow_no_auth_hosts(),
            extra: serde_json::Map::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolGuardConfig {
    #[serde(
        default = "default_true",
        deserialize_with = "deserialize_null_default"
    )]
    pub enabled: bool,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub custom_rules: Vec<ToolGuardRuleConfig>,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub disabled_rules: Vec<String>,
    #[serde(
        default = "default_approval_mode",
        deserialize_with = "deserialize_null_default"
    )]
    pub approval_mode: String,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

fn default_true() -> bool {
    true
}

fn default_approval_mode() -> String {
    "hands_off".into()
}

impl Default for ToolGuardConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            custom_rules: Vec::new(),
            disabled_rules: Vec::new(),
            approval_mode: default_approval_mode(),
            extra: serde_json::Map::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolGuardRuleConfig {
    pub id: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub tools: Vec<String>,
    #[serde(
        default = "default_category",
        deserialize_with = "deserialize_null_default"
    )]
    pub category: String,
    #[serde(
        default = "default_severity",
        deserialize_with = "deserialize_null_default"
    )]
    pub severity: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub description: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub remediation: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub command_set: Vec<String>,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub arg_matchers: Vec<CustomArgMatcher>,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub path_matchers: Vec<CustomPathMatcher>,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub lexical: Vec<String>,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub structural: Vec<String>,
    #[serde(
        default = "default_action",
        deserialize_with = "deserialize_null_default"
    )]
    pub action: String,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CustomArgMatcher {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_contains: Option<String>,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub value_in: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CustomPathMatcher {
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub contains: Vec<String>,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub prefix: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub glob: Option<String>,
    #[serde(default)]
    pub on_redirect: bool,
}

fn default_action() -> String {
    "NeedsApproval".into()
}

fn default_category() -> String {
    "command_injection".into()
}

fn default_severity() -> String {
    "HIGH".into()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileGuardConfig {
    #[serde(
        default = "default_true",
        deserialize_with = "deserialize_null_default"
    )]
    pub enabled: bool,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub sensitive_files: Vec<String>,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub disabled_patterns: Vec<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl Default for FileGuardConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            sensitive_files: Vec::new(),
            disabled_patterns: Vec::new(),
            extra: serde_json::Map::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SkillScannerConfig {
    #[serde(
        default = "default_mode",
        deserialize_with = "deserialize_null_default"
    )]
    pub mode: String,
    #[serde(
        default = "default_scanner_timeout",
        deserialize_with = "deserialize_null_default"
    )]
    pub timeout: u32,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub whitelist: Vec<SkillScannerWhitelistEntry>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

fn default_mode() -> String {
    "warn".into()
}

fn default_scanner_timeout() -> u32 {
    30
}

impl Default for SkillScannerConfig {
    fn default() -> Self {
        Self {
            mode: default_mode(),
            timeout: 30,
            whitelist: Vec::new(),
            policy: None,
            extra: serde_json::Map::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SkillScannerWhitelistEntry {
    pub skill_name: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub content_hash: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub added_at: String,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}
