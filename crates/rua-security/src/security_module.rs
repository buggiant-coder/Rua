use std::path::PathBuf;
use std::sync::Arc;

use dashmap::DashMap;
use tokio::sync::{Mutex, RwLock};

use rua_config::dirs::Paths;
use rua_config::types::security::SecurityConfig;
use rua_core::{ChatAddress, ChatStore};

use crate::approval_authority::{
    AgentScopeRuleStore, ApprovalAuthority, ApprovalModePreset, ApprovalRequestInput,
    ApprovalReuseRule, ApprovalReuseScope, ApprovalTrigger, ApprovalTriggerSource,
    ChatScopeRuleStore, PermissionToolCall, StoredApprovalReuseRule, ToolCallSnapshot,
};
use crate::config_store::{SecurityConfigStore, StoreError};
use crate::engine::ToolGuardEngine;
use crate::models::{GuardDecision, ToolCallContext, ToolGuardFinding};

pub type AgentId = String;

/// The target's policy for a single file. Confirmation must be verified by the caller.
#[must_use = "a required one-time confirmation must be verified before reading"]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileReadAdmission {
    Allowed,
    RequiresUserConfirmation,
}

/// Distinguishes an explicit policy rejection from a missing or unreadable policy.
#[derive(Debug, thiserror::Error)]
pub enum FileReadPolicyError {
    #[error("{0}")]
    Denied(String),
    #[error("{0}")]
    Unavailable(String),
}

#[derive(Debug, Clone)]
pub enum SecurityVerdict {
    Allow,
    Deny { reason: String },
    RequiresApproval { request_id: String, prompt: String },
    AuditOnly { findings: Vec<ToolGuardFinding> },
}

impl SecurityVerdict {
    pub fn is_allow(&self) -> bool {
        matches!(self, Self::Allow)
    }
    pub fn is_deny(&self) -> bool {
        matches!(self, Self::Deny { .. })
    }
    pub fn requires_approval(&self) -> bool {
        matches!(self, Self::RequiresApproval { .. })
    }
}

pub struct SecurityModule {
    engines: DashMap<AgentId, Arc<RwLock<ToolGuardEngine>>>,
    approval_authority: Arc<ApprovalAuthority>,
    config_store: Arc<SecurityConfigStore>,
    paths: Option<Arc<Paths>>,
    decision_lock: Mutex<()>,
    verdict_override: Option<SecurityVerdict>,
}

impl SecurityModule {
    pub fn new(config_store: Arc<SecurityConfigStore>, paths: Option<Arc<Paths>>) -> Self {
        let agent_rule_store = Arc::new(ConfigBackedAgentScopeRuleStore {
            config_store: Arc::clone(&config_store),
        });
        let chat_rule_store = Arc::new(FileBackedChatScopeRuleStore {
            data_dir: paths
                .as_ref()
                .map(|paths| paths.rua_root.clone())
                .unwrap_or_else(|| config_store.data_dir().to_path_buf()),
        });
        Self {
            engines: DashMap::new(),
            approval_authority: Arc::new(ApprovalAuthority::with_rule_stores(
                agent_rule_store,
                chat_rule_store,
            )),
            config_store,
            paths,
            decision_lock: Mutex::new(()),
            verdict_override: None,
        }
    }

    pub fn secret_dir(&self) -> Option<&std::path::Path> {
        self.paths.as_ref().map(|p| p.secret_dir.as_path())
    }

    pub fn for_test(verdict: SecurityVerdict) -> Arc<Self> {
        let config_store = Arc::new(SecurityConfigStore::new(
            std::env::temp_dir().join("rua-security-test"),
        ));
        Arc::new(Self {
            engines: DashMap::new(),
            approval_authority: Arc::new(ApprovalAuthority::new()),
            config_store,
            paths: None,
            decision_lock: Mutex::new(()),
            verdict_override: Some(verdict),
        })
    }

    pub fn approval_authority(&self) -> &Arc<ApprovalAuthority> {
        &self.approval_authority
    }

    /// Inspect a file already allowed by an authenticated user on this Device. The caller
    /// verifies any invocation confirmation against the exact active Chat call. It can satisfy
    /// the approval mode, while every path-specific guard still checks the opened file's names.
    pub async fn inspect_preapproved_chat_file_read(
        &self,
        ctx: &ToolCallContext,
        captured: &SecurityConfig,
        local_names: &[&str],
        confirmed_invocation: bool,
    ) -> Result<FileReadAdmission, FileReadPolicyError> {
        use FileReadPolicyError::{Denied, Unavailable};

        let admission = self
            .inspect_file_read_policy(ctx, captured, local_names, confirmed_invocation)
            .await?;
        let device = ctx
            .local_device_id
            .as_deref()
            .ok_or_else(|| Denied("No local Device for the approved read".into()))?;
        let approved = self
            .approval_authority
            .locally_approved_chat_read(ctx, device)
            .await
            .map_err(Unavailable)?;
        if approved
            .as_deref()
            .is_none_or(|path| Some(path) != local_names.first().copied())
        {
            return Err(Denied(
                "No current local approval for this file read".into(),
            ));
        }
        Ok(admission)
    }

    /// Evaluate the captured and current policies against the supplied local names of the file.
    /// This records no approval and consumes no decision, including during Ask First.
    pub async fn inspect_chat_file_read(
        &self,
        ctx: &ToolCallContext,
        captured: &SecurityConfig,
        local_names: &[&str],
    ) -> Result<FileReadAdmission, FileReadPolicyError> {
        self.inspect_file_read_policy(ctx, captured, local_names, false)
            .await
    }

    async fn inspect_file_read_policy(
        &self,
        ctx: &ToolCallContext,
        captured: &SecurityConfig,
        local_names: &[&str],
        confirmed_invocation: bool,
    ) -> Result<FileReadAdmission, FileReadPolicyError> {
        use FileReadPolicyError::{Denied, Unavailable};

        let address = ctx
            .approval_owner
            .as_chat_address()
            .ok_or_else(|| Denied("File read requires a Chat".into()))?;
        let requested_name = ctx
            .tool_input
            .get("file_path")
            .and_then(serde_json::Value::as_str);
        if ctx.tool_name != "read"
            || requested_name.is_none()
            || requested_name != local_names.first().copied()
        {
            return Err(Denied("File read must match the requested path".into()));
        }
        let current = self
            .config_store
            .load(&ctx.agent_id)
            .map_err(|e| Unavailable(e.to_string()))?;
        let chat_rules = self
            .approval_authority
            .chat_rules_for_inspection(address)
            .await
            .map_err(Unavailable)?;
        let mut admission = FileReadAdmission::Allowed;
        for config in std::iter::once(captured).chain((captured != &current).then_some(&current)) {
            let mut rules = snapshot_permission_rules(config).map_err(Unavailable)?;
            rules.extend(chat_rules.iter().map(ApprovalReuseRule::to_stored));
            let allows = |args: &serde_json::Value| {
                let call = PermissionToolCall::new("read", args, ctx.local_device_id.as_deref());
                rules.iter().any(|rule| {
                    rule.enabled
                        && rule.effect == crate::PermissionRuleEffect::Allow
                        && rule.pattern().matches(&call)
                })
            };
            let requested_allowed = allows(&ctx.tool_input);
            let engine = ToolGuardEngine::new(config, self.paths.as_deref());
            let mode = canonical_approval_mode_preset(&config.tool_guard.approval_mode);
            if !confirmed_invocation
                && !requested_allowed
                && mode.requires_approval_for_tool(&ctx.tool_name)
            {
                admission = FileReadAdmission::RequiresUserConfirmation;
            }
            for path in local_names {
                let mut operation = ctx.clone();
                operation.tool_input["file_path"] = serde_json::json!(path);
                check_file_read_denials(&operation, &rules).map_err(Denied)?;
                let guard = engine.evaluate(&operation);
                if guard.decision == GuardDecision::Deny {
                    return Err(Denied(format!(
                        "File read denied by security rules: {}",
                        guard
                            .findings
                            .iter()
                            .map(|f| f.rule_id.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )));
                }
                // A guard on a resolved or backing name requires an allow that covers that name too.
                if guard.decision == GuardDecision::RequiresApproval
                    && (!requested_allowed || !allows(&operation.tool_input))
                {
                    admission = FileReadAdmission::RequiresUserConfirmation;
                }
            }
        }
        Ok(admission)
    }

    pub async fn hard_deny_reason(&self, ctx: &ToolCallContext) -> Option<String> {
        if let Err(reason) = validate_snapshot_context(ctx) {
            return Some(reason);
        }
        let deny_rule = match self
            .permission_rule(ctx, crate::PermissionRuleEffect::Deny)
            .await
        {
            Ok(rule) => rule,
            Err(reason) => return Some(reason),
        };
        if let Some(rule) = deny_rule {
            return Some(format!(
                "Tool '{}' denied by permission rule '{}'",
                ctx.tool_name, rule.label
            ));
        }
        let result = self.evaluate_context(ctx).await;
        if result.decision != GuardDecision::Deny {
            return None;
        }
        let reason = result
            .findings
            .iter()
            .map(|finding| format!("[{}] {}", finding.rule_id, finding.message))
            .collect::<Vec<_>>()
            .join("; ");
        self.approval_authority
            .create_denied_request(approval_input_from_ctx(
                ctx,
                trigger_from_finding(primary_finding(&result.findings)),
            ))
            .await;
        Some(format!(
            "Tool '{}' denied by security rules: {reason}",
            ctx.tool_name
        ))
    }

    pub async fn enforce(
        &self,
        ctx: &ToolCallContext,
        approval_required: &[String],
    ) -> SecurityVerdict {
        self.enforce_operations(ctx, approval_required, &[]).await
    }

    /// Authorize the real operations before a disclosed composite invocation.
    /// One exact approval resumes the parent; a leaf deny always wins.
    pub async fn enforce_composite(
        &self,
        ctx: &ToolCallContext,
        operations: &[ToolCallContext],
    ) -> SecurityVerdict {
        self.enforce_operations(ctx, &[], operations).await
    }

    async fn enforce_operations(
        &self,
        ctx: &ToolCallContext,
        approval_required: &[String],
        operations: &[ToolCallContext],
    ) -> SecurityVerdict {
        if let Some(ref v) = self.verdict_override {
            return v.clone();
        }
        let _guard = self.decision_lock.lock().await;
        let mut operation_approval = None;
        for operation in operations {
            if let Some(reason) = self.hard_deny_reason(operation).await {
                return SecurityVerdict::Deny { reason };
            }
            match self
                .permission_rule(operation, crate::PermissionRuleEffect::Allow)
                .await
            {
                Ok(Some(_)) => continue,
                Ok(None) => {}
                Err(reason) => return SecurityVerdict::Deny { reason },
            }
            let result = self.evaluate_context(operation).await;
            if result.decision == GuardDecision::RequiresApproval {
                operation_approval = Some(trigger_from_finding(primary_finding(&result.findings)));
            }
            let config = operation
                .security_snapshot
                .as_deref()
                .cloned()
                .unwrap_or_else(|| {
                    self.config_store
                        .load(&operation.agent_id)
                        .unwrap_or_default()
                });
            let mode = canonical_approval_mode_preset(&config.tool_guard.approval_mode);
            if mode == ApprovalModePreset::AskFirst
                || mode == ApprovalModePreset::Balanced
                    && mode.requires_approval_for_tool(&operation.tool_name)
            {
                operation_approval = Some(ApprovalTrigger::approval_mode(format!(
                    "Composite operation '{}' requires approval",
                    operation.tool_name
                )));
            }
        }

        if let Err(reason) = validate_snapshot_context(ctx) {
            return SecurityVerdict::Deny { reason };
        }
        let deny_rule = match self
            .permission_rule(ctx, crate::PermissionRuleEffect::Deny)
            .await
        {
            Ok(rule) => rule,
            Err(reason) => return SecurityVerdict::Deny { reason },
        };
        if let Some(rule) = deny_rule {
            return SecurityVerdict::Deny {
                reason: format!(
                    "Tool '{}' denied by permission rule '{}'",
                    ctx.tool_name, rule.label
                ),
            };
        }

        let result = self.evaluate_context(ctx).await;
        if !result.findings.is_empty() {
            let reasons: Vec<String> = result
                .findings
                .iter()
                .map(|f| format!("[{}] {}", f.rule_id, f.message))
                .collect();
            let reason = reasons.join("; ");
            if result.decision == GuardDecision::Deny {
                self.approval_authority
                    .create_denied_request(approval_input_from_ctx(
                        ctx,
                        trigger_from_finding(primary_finding(&result.findings)),
                    ))
                    .await;
                return SecurityVerdict::Deny {
                    reason: format!(
                        "Tool '{}' denied by security rules: {reason}",
                        ctx.tool_name
                    ),
                };
            }
        }

        if self
            .approval_authority
            .invocation_is_approved(
                &ctx.agent_id,
                &ctx.approval_owner,
                ctx.metadata
                    .get("turn_id")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or(""),
                ctx.metadata
                    .get("tool_call_id")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or(""),
                &ctx.tool_name,
                &ctx.tool_input,
            )
            .await
        {
            return SecurityVerdict::Allow;
        }
        if let Some(trigger) = operation_approval {
            let prompt = format!(
                "Tool '{}' contains an operation requiring approval: {}",
                ctx.tool_name, trigger.message
            );
            let request = self.create_approval_request(ctx, trigger).await;
            return SecurityVerdict::RequiresApproval {
                request_id: request.id,
                prompt,
            };
        }
        let allow_rule = match self
            .permission_rule(ctx, crate::PermissionRuleEffect::Allow)
            .await
        {
            Ok(rule) => rule,
            Err(reason) => return SecurityVerdict::Deny { reason },
        };
        if allow_rule.is_some() {
            return SecurityVerdict::Allow;
        }

        let config = ctx
            .security_snapshot
            .as_deref()
            .cloned()
            .unwrap_or_else(|| self.config_store.load(&ctx.agent_id).unwrap_or_default());
        let mode = canonical_approval_mode_preset(&config.tool_guard.approval_mode);

        if !result.findings.is_empty() {
            let reasons: Vec<String> = result
                .findings
                .iter()
                .map(|f| format!("[{}] {}", f.rule_id, f.message))
                .collect();
            let reason = reasons.join("; ");
            match result.decision {
                GuardDecision::Deny => {
                    return SecurityVerdict::Deny {
                        reason: format!(
                            "Tool '{}' denied by security rules: {reason}",
                            ctx.tool_name
                        ),
                    };
                }
                GuardDecision::AuditOnly => {
                    return SecurityVerdict::AuditOnly {
                        findings: result.findings,
                    };
                }
                GuardDecision::RequiresApproval => {
                    let request = self
                        .create_approval_request(
                            ctx,
                            trigger_from_finding(primary_finding(&result.findings)),
                        )
                        .await;
                    return SecurityVerdict::RequiresApproval {
                        request_id: request.id,
                        prompt: format!(
                            "Tool '{}' flagged by security rules: {reason}",
                            ctx.tool_name
                        ),
                    };
                }
                GuardDecision::Allow => {}
            }
        }

        match mode {
            ApprovalModePreset::AskFirst => {
                let message = format!("Tool '{}' requires approval (Ask First)", ctx.tool_name);
                let request = self
                    .create_approval_request(ctx, ApprovalTrigger::approval_mode(message.clone()))
                    .await;
                return SecurityVerdict::RequiresApproval {
                    request_id: request.id,
                    prompt: message,
                };
            }
            ApprovalModePreset::Balanced if mode.requires_approval_for_tool(&ctx.tool_name) => {
                let message = format!("Tool '{}' requires approval (Balanced)", ctx.tool_name);
                let request = self
                    .create_approval_request(ctx, ApprovalTrigger::approval_mode(message.clone()))
                    .await;
                return SecurityVerdict::RequiresApproval {
                    request_id: request.id,
                    prompt: message,
                };
            }
            ApprovalModePreset::HandsOff | ApprovalModePreset::JustDoIt => {}
            _ => {}
        }

        if matches!(
            mode,
            ApprovalModePreset::AskFirst | ApprovalModePreset::Balanced
        ) && approval_required.contains(&ctx.tool_name)
        {
            let message = format!("Tool '{}' requires approval", ctx.tool_name);
            let request = self
                .create_approval_request(ctx, ApprovalTrigger::approval_mode(message.clone()))
                .await;
            return SecurityVerdict::RequiresApproval {
                request_id: request.id,
                prompt: message,
            };
        }

        SecurityVerdict::Allow
    }

    pub fn save_and_invalidate(
        &self,
        agent_id: &str,
        config: &SecurityConfig,
    ) -> Result<(), StoreError> {
        self.config_store.save(agent_id, config)?;
        self.engines.remove(agent_id);
        Ok(())
    }

    pub fn invalidate(&self, agent_id: &str) {
        self.engines.remove(agent_id);
    }

    pub fn invalidate_all(&self) {
        self.engines.clear();
    }

    pub fn config_store(&self) -> &Arc<SecurityConfigStore> {
        &self.config_store
    }

    async fn evaluate_context(&self, ctx: &ToolCallContext) -> crate::ToolGuardResult {
        if let Some(config) = ctx.security_snapshot.as_ref() {
            ToolGuardEngine::new(config, self.paths.as_deref()).evaluate(ctx)
        } else {
            self.get_or_create_engine(&ctx.agent_id)
                .read()
                .await
                .evaluate(ctx)
        }
    }

    async fn permission_rule(
        &self,
        ctx: &ToolCallContext,
        effect: crate::PermissionRuleEffect,
    ) -> Result<Option<ApprovalReuseRule>, String> {
        let call = PermissionToolCall::new(
            &ctx.tool_name,
            &ctx.tool_input,
            ctx.local_device_id.as_deref(),
        );
        if let Some(config) = ctx.security_snapshot.as_ref() {
            let rules = snapshot_permission_rules(config)?;
            return self
                .approval_authority
                .find_rule_in_snapshot(&ctx.agent_id, &ctx.approval_owner, effect, &call, &rules)
                .await;
        }
        let owner = (effect == crate::PermissionRuleEffect::Allow).then_some(&ctx.approval_owner);
        Ok(self
            .approval_authority
            .find_permission_rule(&ctx.agent_id, owner, effect, &call)
            .await)
    }

    async fn create_approval_request(
        &self,
        ctx: &ToolCallContext,
        trigger: ApprovalTrigger,
    ) -> crate::ApprovalRequest {
        self.approval_authority
            .create_request_for_device(
                approval_input_from_ctx(ctx, trigger),
                ctx.local_device_id.as_deref(),
            )
            .await
    }

    fn get_or_create_engine(&self, agent_id: &str) -> Arc<RwLock<ToolGuardEngine>> {
        if let Some(entry) = self.engines.get(agent_id) {
            return entry.value().clone();
        }

        let config = self.config_store.load(agent_id).unwrap_or_default();
        let engine = ToolGuardEngine::new(&config, self.paths.as_deref());
        let arc = Arc::new(RwLock::new(engine));
        self.engines.insert(agent_id.to_string(), arc.clone());
        arc
    }
}

fn validate_snapshot_context(ctx: &ToolCallContext) -> Result<(), String> {
    if ctx.metadata.contains_key("chat_configuration_epoch") && ctx.security_snapshot.is_none() {
        return Err("Captured Chat execution is missing its security snapshot".to_string());
    }
    Ok(())
}

fn approval_input_from_ctx(
    ctx: &ToolCallContext,
    trigger: ApprovalTrigger,
) -> ApprovalRequestInput {
    let tool_call_id = ctx
        .metadata
        .get("tool_call_id")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_string();
    let turn_id = ctx
        .metadata
        .get("turn_id")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_string();

    ApprovalRequestInput {
        agent_id: ctx.agent_id.clone(),
        owner: ctx.approval_owner.clone(),
        turn_id,
        tool_call_id,
        tool_name: ctx.tool_name.clone(),
        trigger,
        tool_call: ToolCallSnapshot {
            display: format!("{} {}", ctx.tool_name, ctx.tool_input),
            args: ctx.tool_input.clone(),
        },
    }
}

fn trigger_from_finding(finding: &ToolGuardFinding) -> ApprovalTrigger {
    let source = if finding.guardian == "file_guardian" || finding.rule_id.starts_with("FILE-") {
        ApprovalTriggerSource::FileGuard
    } else {
        ApprovalTriggerSource::ToolGuard
    };

    ApprovalTrigger {
        source,
        rule_id: Some(finding.rule_id.clone()),
        guardian: (!finding.guardian.is_empty()).then(|| finding.guardian.clone()),
        severity: Some(finding.severity.to_string()),
        message: finding.message.clone(),
    }
}

fn primary_finding(findings: &[ToolGuardFinding]) -> &ToolGuardFinding {
    findings
        .iter()
        .max_by_key(|finding| finding.severity)
        .expect("approval trigger requires at least one finding")
}

const PERMISSION_RULES_KEY: &str = "permission_rules";

fn snapshot_permission_rules(
    config: &SecurityConfig,
) -> Result<Vec<StoredApprovalReuseRule>, String> {
    config
        .extra
        .get(PERMISSION_RULES_KEY)
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map(|rules| rules.unwrap_or_default())
        .map_err(|error| format!("Invalid security snapshot permission rules: {error}"))
}

fn check_file_read_denials(
    ctx: &ToolCallContext,
    rules: &[StoredApprovalReuseRule],
) -> Result<(), String> {
    use crate::PermissionRuleEffect;

    let call =
        PermissionToolCall::target_read_for_denial(&ctx.tool_input, ctx.local_device_id.as_deref());
    if let Some(rule) = rules.iter().find(|rule| {
        rule.enabled && rule.effect == PermissionRuleEffect::Deny && rule.pattern().matches(&call)
    }) {
        return Err(format!(
            "File read denied by permission rule '{}'",
            rule.label
        ));
    }
    Ok(())
}

#[derive(Debug)]
struct ConfigBackedAgentScopeRuleStore {
    config_store: Arc<SecurityConfigStore>,
}

impl AgentScopeRuleStore for ConfigBackedAgentScopeRuleStore {
    fn list_agent_rules(&self, agent_id: &str) -> Result<Vec<ApprovalReuseRule>, String> {
        let config = self
            .config_store
            .load(agent_id)
            .map_err(|error| error.to_string())?;
        Ok(read_stored_permission_rules_from_security_config(&config)
            .into_iter()
            .map(|rule| rule.into_runtime(agent_id.to_string(), None, ApprovalReuseScope::Agent))
            .collect())
    }

    fn upsert_agent_rule(&self, agent_id: &str, rule: &ApprovalReuseRule) -> Result<(), String> {
        if rule.scope != ApprovalReuseScope::Agent {
            return Err("only Agent Scope permission rules are durable".into());
        }
        let mut config = self
            .config_store
            .load(agent_id)
            .map_err(|error| error.to_string())?;
        let mut rules = read_stored_permission_rules_from_security_config(&config);
        rules.retain(|existing| {
            existing.id != rule.id && !existing.pattern().same_matcher(&rule.pattern())
        });
        rules.push(rule.to_stored());
        write_stored_permission_rules_to_security_config(&mut config, &rules)?;
        self.config_store
            .save(agent_id, &config)
            .map_err(|error| error.to_string())
    }

    fn delete_agent_rule(&self, agent_id: &str, rule_id: &str) -> Result<bool, String> {
        let mut config = self
            .config_store
            .load(agent_id)
            .map_err(|error| error.to_string())?;
        let mut rules = read_stored_permission_rules_from_security_config(&config);
        let before = rules.len();
        rules.retain(|rule| rule.id != rule_id);
        if rules.len() == before {
            return Ok(false);
        }
        write_stored_permission_rules_to_security_config(&mut config, &rules)?;
        self.config_store
            .save(agent_id, &config)
            .map_err(|error| error.to_string())?;
        Ok(true)
    }
}

#[derive(Debug)]
struct FileBackedChatScopeRuleStore {
    data_dir: PathBuf,
}

impl FileBackedChatScopeRuleStore {
    fn permission_rules_path(&self, address: &ChatAddress) -> PathBuf {
        ChatStore::resolve(&self.data_dir, address.clone())
            .root()
            .join("sessions")
            .join(safe_path_component(address.chat_id().as_str()))
            .join("permission_rules.json")
    }
}

impl ChatScopeRuleStore for FileBackedChatScopeRuleStore {
    fn list_chat_rules(&self, address: &ChatAddress) -> Result<Vec<ApprovalReuseRule>, String> {
        let path = self.permission_rules_path(address);
        Ok(read_stored_permission_rules_from_path(&path)?
            .into_iter()
            .map(|rule| {
                rule.into_runtime(
                    String::new(),
                    Some(crate::ApprovalOwner::chat(address.clone())),
                    ApprovalReuseScope::Chat,
                )
            })
            .collect())
    }

    fn upsert_chat_rule(
        &self,
        address: &ChatAddress,
        rule: &ApprovalReuseRule,
    ) -> Result<(), String> {
        if rule.scope != ApprovalReuseScope::Chat {
            return Err("only Chat Scope permission rules belong in chat storage".into());
        }
        let path = self.permission_rules_path(address);
        let mut rules = read_stored_permission_rules_from_path(&path)?;
        rules.retain(|existing| {
            existing.id != rule.id && !existing.pattern().same_matcher(&rule.pattern())
        });
        rules.push(rule.to_stored());
        write_stored_permission_rules_to_path(&path, &rules)
    }

    fn delete_chat_rules(&self, address: &ChatAddress) -> Result<(), String> {
        remove_permission_rules_file(&self.permission_rules_path(address))
    }
}

fn read_stored_permission_rules_from_security_config(
    config: &SecurityConfig,
) -> Vec<StoredApprovalReuseRule> {
    config
        .extra
        .get(PERMISSION_RULES_KEY)
        .cloned()
        .and_then(|value| serde_json::from_value::<Vec<StoredApprovalReuseRule>>(value).ok())
        .unwrap_or_default()
}

fn write_stored_permission_rules_to_security_config(
    config: &mut SecurityConfig,
    rules: &[StoredApprovalReuseRule],
) -> Result<(), String> {
    config.extra.insert(
        PERMISSION_RULES_KEY.to_string(),
        serde_json::to_value(rules).map_err(|error| error.to_string())?,
    );
    Ok(())
}

fn read_stored_permission_rules_from_path(
    path: &std::path::Path,
) -> Result<Vec<StoredApprovalReuseRule>, String> {
    match std::fs::read_to_string(path) {
        Ok(content) => serde_json::from_str(&content).map_err(|error| error.to_string()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error.to_string()),
    }
}

fn write_stored_permission_rules_to_path(
    path: &std::path::Path,
    rules: &[StoredApprovalReuseRule],
) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let value = serde_json::to_value(rules).map_err(|error| error.to_string())?;
    rua_config::atomic_write_json(path, &value).map_err(|error| error.to_string())
}

fn remove_permission_rules_file(path: &std::path::Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

fn safe_path_component(value: &str) -> String {
    value
        .chars()
        .map(|ch| match ch {
            '/' | '\\' | ':' | '\0' => '_',
            ch => ch,
        })
        .collect::<String>()
}

fn canonical_approval_mode_preset(raw: &str) -> ApprovalModePreset {
    match raw.trim().to_ascii_lowercase().as_str() {
        "ask_first" | "ask-first" => ApprovalModePreset::AskFirst,
        "balanced" | "hands_off" | "hands-off" | "strict" | "smart" | "custom" | "off" | "" => {
            ApprovalModePreset::HandsOff
        }
        "just_do_it" | "just-do-it" => ApprovalModePreset::JustDoIt,
        _ => ApprovalModePreset::HandsOff,
    }
}
