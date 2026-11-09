use rua_config::types::security::FileGuardConfig;

use crate::file_guardian::{has_traversal, normalize_path, FileGuardian};
use crate::models::{Severity, ToolCallContext, ToolGuardFinding};
use crate::redaction::redact_secrets;

#[derive(Debug, Clone)]
pub enum FileGuardVerdict {
    Allow,

    Deny { findings: Vec<ToolGuardFinding> },

    RequireApproval { findings: Vec<ToolGuardFinding> },
}

impl FileGuardVerdict {
    pub fn is_allow(&self) -> bool {
        matches!(self, Self::Allow)
    }

    pub fn is_deny(&self) -> bool {
        matches!(self, Self::Deny { .. })
    }

    pub fn is_require_approval(&self) -> bool {
        matches!(self, Self::RequireApproval { .. })
    }

    pub fn findings(&self) -> &[ToolGuardFinding] {
        match self {
            Self::Allow => &[],
            Self::Deny { findings } | Self::RequireApproval { findings } => findings,
        }
    }
}

pub struct FileGuard {
    guardian: FileGuardian,
}

impl FileGuard {
    pub fn new(config: &FileGuardConfig) -> Self {
        Self {
            guardian: FileGuardian::new(config),
        }
    }

    pub fn check_path(&self, path: &str, working_dir: &str) -> FileGuardVerdict {
        if has_traversal(path) {
            let message = format!(
                "Path traversal detected in path: {}",
                crate::redaction::redact_path(path)
            );
            let finding = ToolGuardFinding {
                rule_id: "FILE-TRAVERSAL".into(),
                category: "path_traversal".into(),
                severity: Severity::High,
                title: "Path traversal detected".into(),
                description: message.clone(),
                tool_name: "read".into(),
                guardian: "file_guardian".into(),
                message,
                remediation: "Use absolute paths or paths relative to the working directory \
                              without '..' components"
                    .into(),
                matched_pattern: Some("..".into()),
                matched_value: Some(crate::redaction::redact_path(path)),
            };
            return FileGuardVerdict::RequireApproval {
                findings: vec![finding],
            };
        }

        if path.contains('\0') {
            let finding = ToolGuardFinding {
                rule_id: "FILE-NULLBYTE".into(),
                category: "path_traversal".into(),
                severity: Severity::Critical,
                title: "Null byte detected".into(),
                description: "Null byte detected in file path".into(),
                tool_name: "read".into(),
                guardian: "file_guardian".into(),
                message: "Null byte detected in file path".into(),
                remediation: "Remove null bytes from the file path".into(),
                matched_pattern: Some("\\0".into()),
                matched_value: Some(crate::redaction::redact_path(path)),
            };
            return FileGuardVerdict::RequireApproval {
                findings: vec![finding],
            };
        }

        let ctx = ToolCallContext {
            security_snapshot: None,
            agent_id: String::new(),
            approval_owner: crate::ApprovalOwner::runtime_session("", ""),
            session_id: String::new(),
            tool_name: "read".into(),
            tool_input: serde_json::json!({"path": path}),
            working_dir: working_dir.into(),
            channel: "console".into(),
            trusted: false,
            user_id: String::new(),
            metadata: Default::default(),
        };

        let findings = self.guardian.evaluate(&ctx);

        if findings.is_empty() {
            return FileGuardVerdict::Allow;
        }

        FileGuardVerdict::RequireApproval { findings }
    }

    pub fn is_within_workspace(&self, path: &str, working_dir: &str) -> bool {
        if !path.starts_with('/') {
            return true;
        }
        let normalized = normalize_path(path);
        let workspace = normalize_path(working_dir);
        normalized.starts_with(&workspace)
    }

    pub fn has_traversal(&self, path: &str) -> bool {
        has_traversal(path)
    }

    pub fn normalize(&self, path: &str) -> String {
        normalize_path(path)
    }

    pub fn redact_content(&self, content: &str) -> String {
        redact_secrets(content)
    }
}
