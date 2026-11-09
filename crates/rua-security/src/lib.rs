pub mod approval_authority;
pub mod ast_rule;
pub mod audit;
pub mod audit_sink;
pub mod bash_analyzer;
pub mod builtin_rules;
pub mod config_store;
pub mod engine;
pub mod file_guard;
pub mod file_guardian;
pub mod models;
pub mod project_resource_policy;
pub mod redaction;
pub mod security_module;

pub use ast_rule::{
    evaluate_ast_rules, evaluate_ast_rules_full, ArgMatcher, AstAction, AstRule, LexicalCheck,
    PathMatcher, StructuralFlag,
};
pub use bash_analyzer::{BashAnalysis, BashAnalyzer};

pub use approval_authority::{
    ApprovalAuthority, ApprovalAuthorityError, ApprovalDecision, ApprovalEvent, ApprovalModePreset,
    ApprovalOwner, ApprovalRequest, ApprovalRequestInput, ApprovalReuseMatcher,
    ApprovalReuseOption, ApprovalReuseRule, ApprovalReuseScope, ApprovalSnapshot,
    ApprovalStatus as AuthorityStatus, ApprovalTrigger, ApprovalTriggerSource,
    PermissionRuleEffect, PermissionRuleInput, PermissionRuleSource, ToolCallSnapshot,
};
pub use audit::{AuditEventKind, SecurityAuditEvent};
pub use audit_sink::{read_audit_entries, FileAuditSink};
pub use builtin_rules::builtin_structured_rules;
pub use config_store::{SecurityConfigStore, StoreError};
pub use engine::ToolGuardEngine;
pub use file_guard::{FileGuard, FileGuardVerdict};
pub use file_guardian::FileGuardian;
pub use models::{GuardDecision, Severity, ToolCallContext, ToolGuardFinding, ToolGuardResult};
pub use project_resource_policy::{
    EffectiveProjectResourcePolicy, ProjectResourcePolicySource, ProjectResourceProfile,
    PROJECT_RESOURCE_POLICY_KEY,
};
pub use redaction::{redact_path, redact_secrets, truncate_value};
pub use security_module::{SecurityModule, SecurityVerdict};

use rua_config::types::security::{FileGuardConfig, SecurityConfig};

pub fn evaluate_command(command: &str) -> ToolGuardResult {
    let engine = ToolGuardEngine::new(&SecurityConfig::default(), None);
    let ctx = ToolCallContext::new("", "", "bash", serde_json::json!({"command": command}));
    engine.evaluate(&ctx)
}

pub fn evaluate_tool_call(ctx: &ToolCallContext) -> ToolGuardResult {
    let engine = ToolGuardEngine::new(&SecurityConfig::default(), None);
    engine.evaluate(ctx)
}

pub fn evaluate_path(path: &str, working_dir: &str) -> FileGuardVerdict {
    let guard = FileGuard::new(&FileGuardConfig::default());
    guard.check_path(path, working_dir)
}
