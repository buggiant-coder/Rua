use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::ApprovalOwner;
pub const EFFECTIVE_SECURITY_CONFIG_METADATA_KEY: &str = "effective_security_config";

#[derive(Debug, Clone)]
pub struct ToolCallContext {
    pub security_snapshot: Option<std::sync::Arc<rua_config::types::security::SecurityConfig>>,
    pub agent_id: String,

    pub approval_owner: ApprovalOwner,

    pub session_id: String,

    pub tool_name: String,

    pub tool_input: serde_json::Value,

    pub working_dir: String,

    pub channel: String,

    pub trusted: bool,

    pub user_id: String,

    pub metadata: HashMap<String, serde_json::Value>,
}

impl ToolCallContext {
    pub fn new(
        agent_id: impl Into<String>,
        session_id: impl Into<String>,
        tool_name: impl Into<String>,
        tool_input: serde_json::Value,
    ) -> Self {
        let agent_id = agent_id.into();
        let session_id = session_id.into();
        Self {
            security_snapshot: None,
            approval_owner: ApprovalOwner::runtime_session(&agent_id, &session_id),
            agent_id,
            session_id,
            tool_name: tool_name.into(),
            tool_input,
            working_dir: String::new(),
            channel: "console".into(),
            trusted: false,
            user_id: String::new(),
            metadata: HashMap::new(),
        }
    }

    pub fn with_chat_address(mut self, chat_address: rua_core::ChatAddress) -> Self {
        self.approval_owner = ApprovalOwner::chat(chat_address);
        self
    }

    pub fn with_security_snapshot(
        mut self,
        config: std::sync::Arc<rua_config::types::security::SecurityConfig>,
    ) -> Self {
        self.security_snapshot = Some(config);
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Severity {
    Info,
    Low,
    Medium,
    High,
    Critical,
}

impl Severity {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_uppercase().as_str() {
            "INFO" => Some(Self::Info),
            "LOW" => Some(Self::Low),
            "MEDIUM" | "MED" => Some(Self::Medium),
            "HIGH" => Some(Self::High),
            "CRITICAL" | "CRIT" => Some(Self::Critical),
            _ => None,
        }
    }
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Info => write!(f, "INFO"),
            Self::Low => write!(f, "LOW"),
            Self::Medium => write!(f, "MEDIUM"),
            Self::High => write!(f, "HIGH"),
            Self::Critical => write!(f, "CRITICAL"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolGuardFinding {
    pub rule_id: String,

    pub category: String,

    pub severity: Severity,

    #[serde(default)]
    pub title: String,

    #[serde(default)]
    pub description: String,

    #[serde(default)]
    pub tool_name: String,

    #[serde(default)]
    pub guardian: String,

    pub message: String,

    #[serde(default)]
    pub remediation: String,

    #[serde(default)]
    pub matched_pattern: Option<String>,

    #[serde(default)]
    pub matched_value: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GuardDecision {
    Allow,

    Deny,

    #[serde(alias = "ask_user", alias = "require_approval")]
    RequiresApproval,

    AuditOnly,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolGuardResult {
    pub decision: GuardDecision,

    pub findings: Vec<ToolGuardFinding>,

    pub guarded: bool,

    pub evaluated_at: DateTime<Utc>,
}

impl ToolGuardResult {
    pub fn allow() -> Self {
        Self {
            decision: GuardDecision::Allow,
            findings: Vec::new(),
            guarded: false,
            evaluated_at: Utc::now(),
        }
    }

    pub fn deny(findings: Vec<ToolGuardFinding>) -> Self {
        Self {
            decision: GuardDecision::Deny,
            findings,
            guarded: true,
            evaluated_at: Utc::now(),
        }
    }

    pub fn has_high_severity(&self) -> bool {
        self.findings.iter().any(|f| f.severity >= Severity::High)
    }
}
