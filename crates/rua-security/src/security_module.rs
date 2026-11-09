use std::path::PathBuf;
use std::sync::Arc;

use dashmap::DashMap;
use tokio::sync::{Mutex, RwLock};

use rua_config::dirs::Paths;
use rua_config::types::security::SecurityConfig;
use rua_core::{ChatAddress, ChatScopeStore, ProjectId};

use crate::approval_authority::{
    AgentScopeRuleStore, ApprovalAuthority, ApprovalModePreset, ApprovalRequestInput,
    ApprovalReuseRule, ApprovalReuseScope, ApprovalTrigger, ApprovalTriggerSource,
    ChatScopeRuleStore, StoredApprovalReuseRule, ToolCallSnapshot,
};
use crate::config_store::{SecurityConfigStore, StoreError};
use crate::engine::ToolGuardEngine;
use crate::models::{GuardDecision, ToolCallContext, ToolGuardFinding};

pub type AgentId = String;

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
            let request = self
                .approval_authority
                .create_request(approval_input_from_ctx(ctx, trigger))
                .await;
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
                        .approval_authority
                        .create_request(approval_input_from_ctx(
                            ctx,
                            trigger_from_finding(primary_finding(&result.findings)),
                        ))
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
                    .approval_authority
                    .create_request(approval_input_from_ctx(
                        ctx,
                        ApprovalTrigger::approval_mode(message.clone()),
                    ))
                    .await;
                return SecurityVerdict::RequiresApproval {
                    request_id: request.id,
                    prompt: message,
                };
            }
            ApprovalModePreset::Balanced if mode.requires_approval_for_tool(&ctx.tool_name) => {
                let message = format!("Tool '{}' requires approval (Balanced)", ctx.tool_name);
                let request = self
                    .approval_authority
                    .create_request(approval_input_from_ctx(
                        ctx,
                        ApprovalTrigger::approval_mode(message.clone()),
                    ))
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
                .approval_authority
                .create_request(approval_input_from_ctx(
                    ctx,
                    ApprovalTrigger::approval_mode(message.clone()),
                ))
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
        if let Some(config) = ctx.security_snapshot.as_ref() {
            let rules = config
                .extra
                .get(PERMISSION_RULES_KEY)
                .cloned()
                .map(serde_json::from_value::<Vec<StoredApprovalReuseRule>>)
                .transpose()
                .map_err(|error| format!("Invalid security snapshot permission rules: {error}"))?
                .unwrap_or_default();
            return self
                .approval_authority
                .find_permission_rule_with_snapshot(
                    &ctx.agent_id,
                    &ctx.approval_owner,
                    effect,
                    &ctx.tool_name,
                    &ctx.tool_input,
                    &rules,
                )
                .await;
        }
        Ok(match effect {
            crate::PermissionRuleEffect::Deny => {
                self.approval_authority
                    .find_deny_rule(&ctx.agent_id, &ctx.tool_name, &ctx.tool_input)
                    .await
            }
            crate::PermissionRuleEffect::Allow => {
                self.approval_authority
                    .find_reuse_rule_for_owner(
                        &ctx.agent_id,
                        &ctx.approval_owner,
                        &ctx.tool_name,
                        &ctx.tool_input,
                    )
                    .await
            }
        })
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
        rules.retain(|existing| existing.id != rule.id && existing.matcher != rule.matcher);
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
        ChatScopeStore::resolve(&self.data_dir, address.clone())
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
                    address
                        .agent_id()
                        .map(|agent_id| agent_id.as_str())
                        .unwrap_or_default()
                        .to_string(),
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
        rules.retain(|existing| existing.id != rule.id && existing.matcher != rule.matcher);
        rules.push(rule.to_stored());
        write_stored_permission_rules_to_path(&path, &rules)
    }

    fn delete_chat_rules(&self, address: &ChatAddress) -> Result<(), String> {
        remove_permission_rules_file(&self.permission_rules_path(address))
    }

    fn delete_project_rules(&self, project_id: &ProjectId) -> Result<(), String> {
        let sessions_dir = self
            .data_dir
            .join("projects")
            .join(project_id.as_str())
            .join("sessions");
        let entries = match std::fs::read_dir(&sessions_dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.to_string()),
        };
        for entry in entries {
            let entry = entry.map_err(|error| error.to_string())?;
            let file_type = entry.file_type().map_err(|error| error.to_string())?;
            if !file_type.is_dir() || file_type.is_symlink() {
                continue;
            }
            remove_permission_rules_file(&entry.path().join("permission_rules.json"))?;
        }
        Ok(())
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
