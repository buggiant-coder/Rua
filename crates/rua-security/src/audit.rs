use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::models::{GuardDecision, Severity, ToolGuardFinding};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditEventKind {
    ToolGuardEvaluation,

    ToolCallDenied,

    ApprovalRequired,

    AuditOnlyEvaluation,

    ApprovalGranted,

    ApprovalRejected,

    ApprovalTimedOut,

    SensitiveFileAccess,

    PathTraversalAttempt,

    DangerousCommand,

    ConfigUpdated,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityAuditEvent {
    pub kind: AuditEventKind,

    pub timestamp: DateTime<Utc>,

    pub agent_id: String,

    pub session_id: String,

    pub tool_name: String,

    pub decision: GuardDecision,

    #[serde(default)]
    pub max_severity: Option<Severity>,

    pub finding_count: usize,

    #[serde(default)]
    pub finding_summaries: Vec<String>,

    #[serde(default)]
    pub request_id: Option<String>,

    #[serde(default)]
    pub context: Option<String>,
}

impl SecurityAuditEvent {
    pub fn from_evaluation(
        agent_id: &str,
        session_id: &str,
        tool_name: &str,
        decision: GuardDecision,
        findings: &[ToolGuardFinding],
    ) -> Self {
        let kind = match decision {
            GuardDecision::Allow => AuditEventKind::ToolGuardEvaluation,
            GuardDecision::Deny => AuditEventKind::ToolCallDenied,
            GuardDecision::RequiresApproval => AuditEventKind::ApprovalRequired,
            GuardDecision::AuditOnly => AuditEventKind::AuditOnlyEvaluation,
        };

        let max_severity = findings.iter().map(|f| f.severity).max();

        let finding_summaries: Vec<String> = findings
            .iter()
            .map(|f| format!("[{}] {}: {}", f.severity, f.rule_id, f.message))
            .collect();

        Self {
            kind,
            timestamp: Utc::now(),
            agent_id: agent_id.to_string(),
            session_id: session_id.to_string(),
            tool_name: tool_name.to_string(),
            decision,
            max_severity,
            finding_count: findings.len(),
            finding_summaries,
            request_id: None,
            context: None,
        }
    }

    pub fn approval_event(
        kind: AuditEventKind,
        agent_id: &str,
        session_id: &str,
        tool_name: &str,
        request_id: &str,
    ) -> Self {
        Self {
            kind,
            timestamp: Utc::now(),
            agent_id: agent_id.to_string(),
            session_id: session_id.to_string(),
            tool_name: tool_name.to_string(),
            decision: match kind {
                AuditEventKind::ApprovalGranted => GuardDecision::Allow,
                AuditEventKind::ApprovalRejected | AuditEventKind::ApprovalTimedOut => {
                    GuardDecision::Deny
                }
                _ => GuardDecision::RequiresApproval,
            },
            max_severity: None,
            finding_count: 0,
            finding_summaries: Vec::new(),
            request_id: Some(request_id.to_string()),
            context: None,
        }
    }

    pub fn emit(&self) {
        tracing::info!(
            kind = ?self.kind,
            agent_id = %self.agent_id,
            session_id = %self.session_id,
            tool_name = %self.tool_name,
            decision = ?self.decision,
            finding_count = self.finding_count,
            request_id = ?self.request_id,
            "security_audit_event"
        );
    }
}
