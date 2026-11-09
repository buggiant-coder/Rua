use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use rua_core::{ChatAddress, ProjectId};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, RwLock};
use uuid::Uuid;

const BROADCAST_CAPACITY: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ApprovalModePreset {
    AskFirst,
    Balanced,
    HandsOff,
    JustDoIt,
}

impl ApprovalModePreset {
    pub fn requires_approval_for_tool(self, tool_name: &str) -> bool {
        let normalized = tool_name.trim().to_ascii_lowercase();
        match self {
            Self::AskFirst => true,
            Self::Balanced => matches!(
                normalized.as_str(),
                "bash"
                    | "shell_exec"
                    | "file_edit"
                    | "file_write"
                    | "file_append"
                    | "mcp"
                    | "plugin"
                    | "web"
            ),
            Self::HandsOff | Self::JustDoIt => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalStatus {
    Pending,
    Approved,
    Rejected,
    Denied,
    Expired,
    Consumed,
    Superseded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalTriggerSource {
    ApprovalMode,
    ToolGuard,
    FileGuard,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalTrigger {
    pub source: ApprovalTriggerSource,
    #[serde(default)]
    pub rule_id: Option<String>,
    #[serde(default)]
    pub guardian: Option<String>,
    #[serde(default)]
    pub severity: Option<String>,
    pub message: String,
}

impl ApprovalTrigger {
    pub fn approval_mode(message: impl Into<String>) -> Self {
        Self {
            source: ApprovalTriggerSource::ApprovalMode,
            rule_id: None,
            guardian: None,
            severity: None,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCallSnapshot {
    pub display: String,
    pub args: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalReuseScope {
    Chat,
    RuntimeSession,
    Agent,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ApprovalOwner {
    Chat {
        address: ChatAddress,
    },
    RuntimeSession {
        agent_id: String,
        session_id: String,
    },
}

impl ApprovalOwner {
    pub fn chat(address: ChatAddress) -> Self {
        Self::Chat { address }
    }

    pub fn runtime_session(agent_id: impl Into<String>, session_id: impl Into<String>) -> Self {
        Self::RuntimeSession {
            agent_id: agent_id.into(),
            session_id: session_id.into(),
        }
    }

    pub fn as_chat_address(&self) -> Option<&ChatAddress> {
        match self {
            Self::Chat { address } => Some(address),
            Self::RuntimeSession { .. } => None,
        }
    }

    pub fn is_runtime_session(&self, agent_id: &str, session_id: &str) -> bool {
        matches!(
            self,
            Self::RuntimeSession {
                agent_id: owner_agent_id,
                session_id: owner_session_id,
            } if owner_agent_id == agent_id && owner_session_id == session_id
        )
    }

    fn reuse_scope(&self) -> ApprovalReuseScope {
        match self {
            Self::Chat { .. } => ApprovalReuseScope::Chat,
            Self::RuntimeSession { .. } => ApprovalReuseScope::RuntimeSession,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ApprovalReuseMatcher {
    ToolCall {
        tool_name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        input_params: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalReuseOption {
    pub scope: ApprovalReuseScope,
    pub label: String,
    pub matcher: ApprovalReuseMatcher,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApprovalReuseRule {
    pub id: String,
    pub agent_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<ApprovalOwner>,
    pub scope: ApprovalReuseScope,
    pub label: String,
    pub matcher: ApprovalReuseMatcher,
    pub source_approval_id: String,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub updated_at: Option<DateTime<Utc>>,
    #[serde(default = "default_rule_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub effect: PermissionRuleEffect,
    #[serde(default)]
    pub source: PermissionRuleSource,
    pub hit_count: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredApprovalReuseRule {
    pub id: String,
    pub label: String,
    pub matcher: ApprovalReuseMatcher,
    pub source_approval_id: String,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub updated_at: Option<DateTime<Utc>>,
    #[serde(default = "default_rule_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub effect: PermissionRuleEffect,
    #[serde(default)]
    pub source: PermissionRuleSource,
    pub hit_count: u64,
}

impl ApprovalReuseRule {
    pub fn to_stored(&self) -> StoredApprovalReuseRule {
        StoredApprovalReuseRule {
            id: self.id.clone(),
            label: self.label.clone(),
            matcher: self.matcher.clone(),
            source_approval_id: self.source_approval_id.clone(),
            created_at: self.created_at,
            updated_at: self.updated_at,
            enabled: self.enabled,
            effect: self.effect,
            source: self.source,
            hit_count: self.hit_count,
        }
    }
}

impl StoredApprovalReuseRule {
    pub fn into_runtime(
        self,
        agent_id: impl Into<String>,
        owner: Option<ApprovalOwner>,
        scope: ApprovalReuseScope,
    ) -> ApprovalReuseRule {
        ApprovalReuseRule {
            id: self.id,
            agent_id: agent_id.into(),
            owner,
            scope,
            label: self.label,
            matcher: self.matcher,
            source_approval_id: self.source_approval_id,
            created_at: self.created_at,
            updated_at: self.updated_at,
            enabled: self.enabled,
            effect: self.effect,
            source: self.source,
            hit_count: self.hit_count,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PermissionRuleEffect {
    #[default]
    Allow,
    Deny,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PermissionRuleSource {
    #[default]
    Approval,
    Manual,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionRuleInput {
    pub effect: PermissionRuleEffect,
    #[serde(default = "default_rule_enabled")]
    pub enabled: bool,
    pub label: String,
    pub matcher: ApprovalReuseMatcher,
}

fn default_rule_enabled() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApprovalRequestInput {
    pub agent_id: String,
    pub owner: ApprovalOwner,
    pub turn_id: String,
    pub tool_call_id: String,
    pub tool_name: String,
    pub trigger: ApprovalTrigger,
    pub tool_call: ToolCallSnapshot,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApprovalRequest {
    pub id: String,
    pub agent_id: String,
    pub owner: ApprovalOwner,
    pub turn_id: String,
    pub tool_call_id: String,
    pub tool_name: String,
    pub status: ApprovalStatus,
    pub trigger: ApprovalTrigger,
    pub tool_call: ToolCallSnapshot,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub decided_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub decided_by: Option<String>,
    #[serde(default)]
    pub reuse_options: Vec<ApprovalReuseOption>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApprovalSnapshot {
    pub pending: Vec<ApprovalRequest>,
    pub resolved: Vec<ApprovalRequest>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApprovalEvent {
    pub event: String,
    pub request: ApprovalRequest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalDecision {
    RunOnce {
        decided_by: String,
    },
    AllowChat {
        decided_by: String,
        matcher: ApprovalReuseMatcher,
    },
    AllowRuntimeSession {
        decided_by: String,
        matcher: ApprovalReuseMatcher,
    },
    AllowAgent {
        decided_by: String,
        matcher: ApprovalReuseMatcher,
    },
    Reject {
        decided_by: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalAuthorityError {
    NotFound(String),
    InvalidState {
        id: String,
        current: ApprovalStatus,
        attempted: &'static str,
    },
    Store(String),
}

impl std::fmt::Display for ApprovalAuthorityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(id) => write!(f, "approval request not found: {id}"),
            Self::InvalidState {
                id,
                current,
                attempted,
            } => write!(
                f,
                "approval request {id} is {current:?}, cannot {attempted}"
            ),
            Self::Store(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for ApprovalAuthorityError {}

#[derive(Debug)]
pub struct ApprovalAuthority {
    requests: RwLock<HashMap<String, ApprovalRequest>>,
    reuse_rules: RwLock<HashMap<String, ApprovalReuseRule>>,
    agent_rule_store: Option<Arc<dyn AgentScopeRuleStore>>,
    chat_rule_store: Option<Arc<dyn ChatScopeRuleStore>>,
    tx: broadcast::Sender<ApprovalEvent>,
}

pub trait AgentScopeRuleStore: Send + Sync + std::fmt::Debug {
    fn list_agent_rules(&self, agent_id: &str) -> Result<Vec<ApprovalReuseRule>, String>;
    fn upsert_agent_rule(&self, agent_id: &str, rule: &ApprovalReuseRule) -> Result<(), String>;
    fn delete_agent_rule(&self, agent_id: &str, rule_id: &str) -> Result<bool, String>;
}

pub trait ChatScopeRuleStore: Send + Sync + std::fmt::Debug {
    fn list_chat_rules(&self, address: &ChatAddress) -> Result<Vec<ApprovalReuseRule>, String>;
    fn upsert_chat_rule(
        &self,
        address: &ChatAddress,
        rule: &ApprovalReuseRule,
    ) -> Result<(), String>;
    fn delete_chat_rules(&self, address: &ChatAddress) -> Result<(), String>;
    fn delete_project_rules(&self, project_id: &ProjectId) -> Result<(), String>;
}

impl ApprovalAuthority {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(BROADCAST_CAPACITY);
        Self {
            requests: RwLock::new(HashMap::new()),
            reuse_rules: RwLock::new(HashMap::new()),
            agent_rule_store: None,
            chat_rule_store: None,
            tx,
        }
    }

    pub fn with_agent_rule_store(agent_rule_store: Arc<dyn AgentScopeRuleStore>) -> Self {
        let (tx, _) = broadcast::channel(BROADCAST_CAPACITY);
        Self {
            requests: RwLock::new(HashMap::new()),
            reuse_rules: RwLock::new(HashMap::new()),
            agent_rule_store: Some(agent_rule_store),
            chat_rule_store: None,
            tx,
        }
    }

    pub fn with_rule_stores(
        agent_rule_store: Arc<dyn AgentScopeRuleStore>,
        chat_rule_store: Arc<dyn ChatScopeRuleStore>,
    ) -> Self {
        let (tx, _) = broadcast::channel(BROADCAST_CAPACITY);
        Self {
            requests: RwLock::new(HashMap::new()),
            reuse_rules: RwLock::new(HashMap::new()),
            agent_rule_store: Some(agent_rule_store),
            chat_rule_store: Some(chat_rule_store),
            tx,
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ApprovalEvent> {
        self.tx.subscribe()
    }

    pub async fn request(&self, id: &str) -> Option<ApprovalRequest> {
        self.requests.read().await.get(id).cloned()
    }

    pub async fn create_request(&self, input: ApprovalRequestInput) -> ApprovalRequest {
        let reuse_options =
            reuse_options_for(&input.owner, &input.tool_name, &input.tool_call.args);
        let request = ApprovalRequest {
            id: Uuid::new_v4().to_string(),
            agent_id: input.agent_id,
            owner: input.owner,
            turn_id: input.turn_id,
            tool_call_id: input.tool_call_id,
            tool_name: input.tool_name,
            status: ApprovalStatus::Pending,
            trigger: input.trigger,
            tool_call: input.tool_call,
            created_at: Utc::now(),
            decided_at: None,
            decided_by: None,
            reuse_options,
        };

        self.requests
            .write()
            .await
            .insert(request.id.clone(), request.clone());
        let _ = self.tx.send(ApprovalEvent {
            event: "approval.created".into(),
            request: request.clone(),
        });
        request
    }

    pub async fn create_denied_request(&self, input: ApprovalRequestInput) -> ApprovalRequest {
        let now = Utc::now();
        let request = ApprovalRequest {
            id: Uuid::new_v4().to_string(),
            agent_id: input.agent_id,
            owner: input.owner,
            turn_id: input.turn_id,
            tool_call_id: input.tool_call_id,
            tool_name: input.tool_name,
            status: ApprovalStatus::Denied,
            trigger: input.trigger,
            tool_call: input.tool_call,
            created_at: now,
            decided_at: Some(now),
            decided_by: None,
            reuse_options: Vec::new(),
        };

        self.requests
            .write()
            .await
            .insert(request.id.clone(), request.clone());
        let _ = self.tx.send(ApprovalEvent {
            event: "approval.created".into(),
            request: request.clone(),
        });
        request
    }

    pub async fn snapshot_at(&self, address: &ChatAddress) -> ApprovalSnapshot {
        self.snapshot_for_owner(&ApprovalOwner::chat(address.clone()))
            .await
    }

    pub async fn snapshot_for_runtime_session(
        &self,
        agent_id: &str,
        session_id: &str,
    ) -> ApprovalSnapshot {
        self.snapshot_for_owner(&ApprovalOwner::runtime_session(agent_id, session_id))
            .await
    }

    pub async fn snapshot_for_owner(&self, owner: &ApprovalOwner) -> ApprovalSnapshot {
        let requests = self.requests.read().await;
        let mut pending = Vec::new();
        let mut resolved = Vec::new();

        for request in requests.values() {
            if request.owner != *owner {
                continue;
            }

            if request.status == ApprovalStatus::Pending {
                pending.push(request.clone());
            } else {
                resolved.push(request.clone());
            }
        }

        pending.sort_by_key(|request| request.created_at);
        resolved.sort_by_key(|request| request.created_at);
        ApprovalSnapshot { pending, resolved }
    }

    pub async fn list_pending_for_agent(&self, agent_id: &str) -> Vec<ApprovalRequest> {
        let mut pending = self
            .requests
            .read()
            .await
            .values()
            .filter(|request| {
                request.agent_id == agent_id && request.status == ApprovalStatus::Pending
            })
            .cloned()
            .collect::<Vec<_>>();
        pending.sort_by_key(|request| request.created_at);
        pending
    }

    pub async fn list_pending_for_address(&self, address: &ChatAddress) -> Vec<ApprovalRequest> {
        self.list_pending_for_owner(&ApprovalOwner::chat(address.clone()))
            .await
    }

    pub async fn list_pending_for_runtime_session(
        &self,
        agent_id: &str,
        session_id: &str,
    ) -> Vec<ApprovalRequest> {
        self.list_pending_for_owner(&ApprovalOwner::runtime_session(agent_id, session_id))
            .await
    }

    async fn list_pending_for_owner(&self, owner: &ApprovalOwner) -> Vec<ApprovalRequest> {
        let mut pending = self
            .requests
            .read()
            .await
            .values()
            .filter(|request| request.owner == *owner && request.status == ApprovalStatus::Pending)
            .cloned()
            .collect::<Vec<_>>();
        pending.sort_by_key(|request| request.created_at);
        pending
    }

    pub async fn clear_by_address(&self, address: &ChatAddress) -> usize {
        self.clear_by_owner(&ApprovalOwner::chat(address.clone()))
            .await
    }

    pub async fn clear_by_runtime_session(&self, agent_id: &str, session_id: &str) -> usize {
        self.clear_by_owner(&ApprovalOwner::runtime_session(agent_id, session_id))
            .await
    }

    async fn clear_by_owner(&self, owner: &ApprovalOwner) -> usize {
        let mut requests = self.requests.write().await;
        let now = Utc::now();
        let ids = requests
            .values()
            .filter(|request| request.owner == *owner && request.status == ApprovalStatus::Pending)
            .map(|request| request.id.clone())
            .collect::<Vec<_>>();
        let mut cleared = Vec::new();
        for id in ids {
            if let Some(request) = requests.get_mut(&id) {
                request.status = ApprovalStatus::Superseded;
                request.decided_at = Some(now);
                cleared.push(request.clone());
            }
        }
        drop(requests);

        for request in &cleared {
            let _ = self.tx.send(ApprovalEvent {
                event: "approval.updated".into(),
                request: request.clone(),
            });
        }
        cleared.len()
    }

    pub async fn decide_oldest_pending_for_address(
        &self,
        address: &ChatAddress,
        decision: ApprovalDecision,
    ) -> Result<Option<ApprovalRequest>, ApprovalAuthorityError> {
        self.decide_oldest_pending_for_owner(&ApprovalOwner::chat(address.clone()), decision)
            .await
    }

    async fn decide_oldest_pending_for_owner(
        &self,
        owner: &ApprovalOwner,
        decision: ApprovalDecision,
    ) -> Result<Option<ApprovalRequest>, ApprovalAuthorityError> {
        let id = {
            let requests = self.requests.read().await;
            requests
                .values()
                .filter(|request| {
                    request.owner == *owner && request.status == ApprovalStatus::Pending
                })
                .min_by_key(|request| request.created_at)
                .map(|request| request.id.clone())
        };
        let Some(id) = id else {
            return Ok(None);
        };
        self.decide(&id, decision).await.map(Some)
    }

    pub async fn decide(
        &self,
        id: &str,
        decision: ApprovalDecision,
    ) -> Result<ApprovalRequest, ApprovalAuthorityError> {
        self.decide_inner(None, id, decision).await
    }

    pub async fn decide_for_agent(
        &self,
        agent_id: &str,
        id: &str,
        decision: ApprovalDecision,
    ) -> Result<ApprovalRequest, ApprovalAuthorityError> {
        self.decide_inner(Some(agent_id), id, decision).await
    }

    async fn decide_inner(
        &self,
        expected_agent_id: Option<&str>,
        id: &str,
        decision: ApprovalDecision,
    ) -> Result<ApprovalRequest, ApprovalAuthorityError> {
        let mut requests = self.requests.write().await;
        let request = requests
            .get_mut(id)
            .ok_or_else(|| ApprovalAuthorityError::NotFound(id.into()))?;
        if expected_agent_id.is_some_and(|agent_id| request.agent_id != agent_id) {
            return Err(ApprovalAuthorityError::NotFound(id.into()));
        }

        if request.status != ApprovalStatus::Pending {
            return Err(ApprovalAuthorityError::InvalidState {
                id: id.into(),
                current: request.status,
                attempted: "decide",
            });
        }

        let mut rule_to_insert: Option<ApprovalReuseRule> = None;
        let (status, decided_by) = match decision {
            ApprovalDecision::RunOnce { decided_by } => (ApprovalStatus::Approved, decided_by),
            ApprovalDecision::AllowChat {
                decided_by,
                matcher,
            } => {
                if request.owner.as_chat_address().is_none() {
                    return Err(ApprovalAuthorityError::InvalidState {
                        id: id.into(),
                        current: request.status,
                        attempted: "allow_chat",
                    });
                }
                let Some(option) = request
                    .reuse_options
                    .iter()
                    .find(|option| {
                        option.scope == ApprovalReuseScope::Chat && option.matcher == matcher
                    })
                    .cloned()
                else {
                    return Err(ApprovalAuthorityError::InvalidState {
                        id: id.into(),
                        current: request.status,
                        attempted: "allow_chat",
                    });
                };
                rule_to_insert = Some(ApprovalReuseRule {
                    id: Uuid::new_v4().to_string(),
                    agent_id: request.agent_id.clone(),
                    owner: Some(request.owner.clone()),
                    scope: ApprovalReuseScope::Chat,
                    label: option.label,
                    matcher,
                    source_approval_id: request.id.clone(),
                    created_at: Utc::now(),
                    updated_at: None,
                    enabled: true,
                    effect: PermissionRuleEffect::Allow,
                    source: PermissionRuleSource::Approval,
                    hit_count: 0,
                });
                (ApprovalStatus::Approved, decided_by)
            }
            ApprovalDecision::AllowRuntimeSession {
                decided_by,
                matcher,
            } => {
                if !matches!(request.owner, ApprovalOwner::RuntimeSession { .. }) {
                    return Err(ApprovalAuthorityError::InvalidState {
                        id: id.into(),
                        current: request.status,
                        attempted: "allow_runtime_session",
                    });
                }
                let Some(option) = request
                    .reuse_options
                    .iter()
                    .find(|option| {
                        option.scope == ApprovalReuseScope::RuntimeSession
                            && option.matcher == matcher
                    })
                    .cloned()
                else {
                    return Err(ApprovalAuthorityError::InvalidState {
                        id: id.into(),
                        current: request.status,
                        attempted: "allow_runtime_session",
                    });
                };
                rule_to_insert = Some(ApprovalReuseRule {
                    id: Uuid::new_v4().to_string(),
                    agent_id: request.agent_id.clone(),
                    owner: Some(request.owner.clone()),
                    scope: ApprovalReuseScope::RuntimeSession,
                    label: option.label,
                    matcher,
                    source_approval_id: request.id.clone(),
                    created_at: Utc::now(),
                    updated_at: None,
                    enabled: true,
                    effect: PermissionRuleEffect::Allow,
                    source: PermissionRuleSource::Approval,
                    hit_count: 0,
                });
                (ApprovalStatus::Approved, decided_by)
            }
            ApprovalDecision::AllowAgent {
                decided_by,
                matcher,
            } => {
                let Some(option) = request
                    .reuse_options
                    .iter()
                    .find(|option| {
                        option.scope == ApprovalReuseScope::Agent && option.matcher == matcher
                    })
                    .cloned()
                else {
                    return Err(ApprovalAuthorityError::InvalidState {
                        id: id.into(),
                        current: request.status,
                        attempted: "allow_agent",
                    });
                };
                rule_to_insert = Some(ApprovalReuseRule {
                    id: Uuid::new_v4().to_string(),
                    agent_id: request.agent_id.clone(),
                    owner: None,
                    scope: ApprovalReuseScope::Agent,
                    label: option.label,
                    matcher,
                    source_approval_id: request.id.clone(),
                    created_at: Utc::now(),
                    updated_at: None,
                    enabled: true,
                    effect: PermissionRuleEffect::Allow,
                    source: PermissionRuleSource::Approval,
                    hit_count: 0,
                });
                (ApprovalStatus::Approved, decided_by)
            }
            ApprovalDecision::Reject { decided_by } => (ApprovalStatus::Rejected, decided_by),
        };

        request.status = status;
        request.decided_at = Some(Utc::now());
        request.decided_by = Some(decided_by);
        let updated = request.clone();
        drop(requests);

        if let Some(rule) = rule_to_insert {
            match rule.scope {
                ApprovalReuseScope::Agent => {
                    if let Some(store) = &self.agent_rule_store {
                        store
                            .upsert_agent_rule(&rule.agent_id, &rule)
                            .map_err(ApprovalAuthorityError::Store)?;
                    }
                }
                ApprovalReuseScope::Chat => {
                    if let (Some(store), Some(address)) = (
                        &self.chat_rule_store,
                        rule.owner.as_ref().and_then(ApprovalOwner::as_chat_address),
                    ) {
                        store
                            .upsert_chat_rule(address, &rule)
                            .map_err(ApprovalAuthorityError::Store)?;
                    }
                }
                ApprovalReuseScope::RuntimeSession => {}
            }
            self.upsert_runtime_rule(rule).await;
        }

        let _ = self.tx.send(ApprovalEvent {
            event: "approval.updated".into(),
            request: updated.clone(),
        });
        Ok(updated)
    }

    pub async fn wait_for_decision(
        &self,
        id: &str,
    ) -> Result<ApprovalRequest, ApprovalAuthorityError> {
        let mut rx = self.subscribe();
        loop {
            {
                let requests = self.requests.read().await;
                let request = requests
                    .get(id)
                    .ok_or_else(|| ApprovalAuthorityError::NotFound(id.into()))?
                    .clone();
                if request.status != ApprovalStatus::Pending {
                    return Ok(request);
                }
            }

            match rx.recv().await {
                Ok(event) if event.request.id == id => {
                    if event.request.status != ApprovalStatus::Pending {
                        return Ok(event.request);
                    }
                }
                Ok(_) => {}
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => {
                    return Err(ApprovalAuthorityError::NotFound(id.into()));
                }
            }
        }
    }

    pub async fn find_reuse_rule_at(
        &self,
        agent_id: &str,
        address: &ChatAddress,
        tool_name: &str,
        args: &serde_json::Value,
    ) -> Option<ApprovalReuseRule> {
        self.find_reuse_rule_for_owner(
            agent_id,
            &ApprovalOwner::chat(address.clone()),
            tool_name,
            args,
        )
        .await
    }

    pub async fn find_reuse_rule_for_runtime_session(
        &self,
        agent_id: &str,
        session_id: &str,
        tool_name: &str,
        args: &serde_json::Value,
    ) -> Option<ApprovalReuseRule> {
        self.find_reuse_rule_for_owner(
            agent_id,
            &ApprovalOwner::runtime_session(agent_id, session_id),
            tool_name,
            args,
        )
        .await
    }

    pub async fn invocation_is_approved(
        &self,
        agent_id: &str,
        owner: &ApprovalOwner,
        turn_id: &str,
        tool_call_id: &str,
        tool_name: &str,
        arguments: &serde_json::Value,
    ) -> bool {
        if turn_id.is_empty() || tool_call_id.is_empty() {
            return false;
        }
        self.requests.read().await.values().any(|request| {
            request.agent_id == agent_id
                && &request.owner == owner
                && request.turn_id == turn_id
                && request.tool_call_id == tool_call_id
                && request.tool_name == tool_name
                && &request.tool_call.args == arguments
                && request.status == ApprovalStatus::Approved
        })
    }

    pub async fn find_reuse_rule_for_owner(
        &self,
        agent_id: &str,
        owner: &ApprovalOwner,
        tool_name: &str,
        args: &serde_json::Value,
    ) -> Option<ApprovalReuseRule> {
        self.find_permission_rule(
            agent_id,
            Some(owner),
            PermissionRuleEffect::Allow,
            tool_name,
            args,
        )
        .await
    }

    pub async fn find_permission_rule_with_snapshot(
        &self,
        agent_id: &str,
        owner: &ApprovalOwner,
        effect: PermissionRuleEffect,
        tool_name: &str,
        args: &serde_json::Value,
        snapshot_rules: &[StoredApprovalReuseRule],
    ) -> Result<Option<ApprovalReuseRule>, String> {
        let stored_chat_rules = match (owner.as_chat_address(), self.chat_rule_store.as_ref()) {
            (Some(address), Some(store)) => store.list_chat_rules(address)?,
            _ => Vec::new(),
        };
        {
            let mut rules = self.reuse_rules.write().await;
            if let Some(rule) = rules.values_mut().find(|rule| {
                matches!(
                    rule.scope,
                    ApprovalReuseScope::Chat | ApprovalReuseScope::RuntimeSession
                ) && rule.owner.as_ref() == Some(owner)
                    && rule.effect == effect
                    && rule.enabled
                    && matcher_matches_tool_call(&rule.matcher, tool_name, args)
            }) {
                rule.hit_count += 1;
                return Ok(Some(rule.clone()));
            }
        }
        if let Some(address) = owner.as_chat_address() {
            if let Some(store) = &self.chat_rule_store {
                if let Some(mut rule) = stored_chat_rules.into_iter().find(|rule| {
                    rule.enabled
                        && rule.effect == effect
                        && matcher_matches_tool_call(&rule.matcher, tool_name, args)
                }) {
                    rule.hit_count += 1;
                    let _ = store.upsert_chat_rule(address, &rule);
                    self.upsert_runtime_rule(rule.clone()).await;
                    return Ok(Some(rule));
                }
            }
        }
        Ok(snapshot_rules
            .iter()
            .find(|rule| {
                rule.enabled
                    && rule.effect == effect
                    && matcher_matches_tool_call(&rule.matcher, tool_name, args)
            })
            .cloned()
            .map(|rule| rule.into_runtime(agent_id.to_string(), None, ApprovalReuseScope::Agent)))
    }

    pub async fn find_deny_rule(
        &self,
        agent_id: &str,
        tool_name: &str,
        args: &serde_json::Value,
    ) -> Option<ApprovalReuseRule> {
        self.find_permission_rule(agent_id, None, PermissionRuleEffect::Deny, tool_name, args)
            .await
    }

    async fn find_permission_rule(
        &self,
        agent_id: &str,
        owner: Option<&ApprovalOwner>,
        effect: PermissionRuleEffect,
        tool_name: &str,
        args: &serde_json::Value,
    ) -> Option<ApprovalReuseRule> {
        {
            let mut rules = self.reuse_rules.write().await;
            if let Some(rule) = rules.values_mut().find(|rule| {
                owner.is_some_and(|owner| rule.scope == owner.reuse_scope())
                    && rule.owner.as_ref() == owner
                    && rule.effect == effect
                    && rule.enabled
                    && matcher_matches_tool_call(&rule.matcher, tool_name, args)
            }) {
                rule.hit_count += 1;
                return Some(rule.clone());
            }

            if let Some(rule) = rules.values_mut().find(|rule| {
                rule.agent_id == agent_id
                    && rule.scope == ApprovalReuseScope::Agent
                    && rule.effect == effect
                    && rule.enabled
                    && matcher_matches_tool_call(&rule.matcher, tool_name, args)
            }) {
                rule.hit_count += 1;
                if let Some(store) = &self.agent_rule_store {
                    let _ = store.upsert_agent_rule(agent_id, rule);
                }
                return Some(rule.clone());
            }
        }

        if let (Some(store), Some(ApprovalOwner::Chat { address })) = (&self.chat_rule_store, owner)
        {
            if let Some(mut rule) = store
                .list_chat_rules(address)
                .ok()?
                .into_iter()
                .find(|rule| {
                    rule.scope == ApprovalReuseScope::Chat
                        && rule.owner.as_ref() == owner
                        && rule.effect == effect
                        && rule.enabled
                        && matcher_matches_tool_call(&rule.matcher, tool_name, args)
                })
            {
                rule.hit_count += 1;
                let _ = store.upsert_chat_rule(address, &rule);
                self.upsert_runtime_rule(rule.clone()).await;
                return Some(rule);
            }
        }

        let store = self.agent_rule_store.as_ref()?;
        let mut rule = store
            .list_agent_rules(agent_id)
            .ok()?
            .into_iter()
            .find(|rule| {
                rule.agent_id == agent_id
                    && rule.scope == ApprovalReuseScope::Agent
                    && rule.effect == effect
                    && rule.enabled
                    && matcher_matches_tool_call(&rule.matcher, tool_name, args)
            })?;
        rule.hit_count += 1;
        let _ = store.upsert_agent_rule(agent_id, &rule);
        self.upsert_runtime_rule(rule.clone()).await;
        Some(rule)
    }

    pub async fn list_agent_rules(&self, agent_id: &str) -> Vec<ApprovalReuseRule> {
        let mut rules = self
            .reuse_rules
            .read()
            .await
            .values()
            .filter(|rule| rule.agent_id == agent_id && rule.scope == ApprovalReuseScope::Agent)
            .cloned()
            .collect::<Vec<_>>();

        if let Some(store) = &self.agent_rule_store {
            if let Ok(stored) = store.list_agent_rules(agent_id) {
                for rule in stored {
                    if !rules.iter().any(|existing| existing.id == rule.id) {
                        rules.push(rule);
                    }
                }
            }
        }

        rules.sort_by_key(|rule| rule.created_at);
        rules
    }

    pub async fn list_chat_rules(&self, address: &ChatAddress) -> Vec<ApprovalReuseRule> {
        let mut rules = self
            .reuse_rules
            .read()
            .await
            .values()
            .filter(|rule| {
                rule.scope == ApprovalReuseScope::Chat
                    && rule.owner.as_ref() == Some(&ApprovalOwner::chat(address.clone()))
            })
            .cloned()
            .collect::<Vec<_>>();

        if let Some(store) = &self.chat_rule_store {
            if let Ok(stored) = store.list_chat_rules(address) {
                for rule in stored {
                    if !rules.iter().any(|existing| existing.id == rule.id) {
                        rules.push(rule);
                    }
                }
            }
        }

        rules.sort_by_key(|rule| rule.created_at);
        rules
    }

    pub async fn delete_chat_rules(&self, address: &ChatAddress) -> Result<(), String> {
        if let Some(store) = &self.chat_rule_store {
            store.delete_chat_rules(address)?;
        }
        self.reuse_rules.write().await.retain(|_, rule| {
            rule.scope != ApprovalReuseScope::Chat
                || rule.owner.as_ref() != Some(&ApprovalOwner::chat(address.clone()))
        });
        Ok(())
    }

    pub async fn delete_project_rules(&self, project_id: &ProjectId) -> Result<(), String> {
        if let Some(store) = &self.chat_rule_store {
            store.delete_project_rules(project_id)?;
        }
        self.reuse_rules.write().await.retain(|_, rule| {
            rule.scope != ApprovalReuseScope::Chat
                || rule
                    .owner
                    .as_ref()
                    .and_then(ApprovalOwner::as_chat_address)
                    .and_then(ChatAddress::project_id)
                    != Some(project_id)
        });
        Ok(())
    }

    pub async fn create_agent_rule(
        &self,
        agent_id: &str,
        input: PermissionRuleInput,
    ) -> Result<ApprovalReuseRule, String> {
        let now = Utc::now();
        let rule = ApprovalReuseRule {
            id: Uuid::new_v4().to_string(),
            agent_id: agent_id.to_string(),
            owner: None,
            scope: ApprovalReuseScope::Agent,
            label: input.label,
            matcher: input.matcher,
            source_approval_id: String::new(),
            created_at: now,
            updated_at: Some(now),
            enabled: input.enabled,
            effect: input.effect,
            source: PermissionRuleSource::Manual,
            hit_count: 0,
        };
        if let Some(store) = &self.agent_rule_store {
            store.upsert_agent_rule(agent_id, &rule)?;
        }
        self.upsert_runtime_rule(rule.clone()).await;
        Ok(rule)
    }

    pub async fn update_agent_rule(
        &self,
        agent_id: &str,
        rule_id: &str,
        input: PermissionRuleInput,
    ) -> Result<Option<ApprovalReuseRule>, String> {
        let mut rules = self.list_agent_rules(agent_id).await;
        let Some(existing) = rules.iter_mut().find(|rule| rule.id == rule_id) else {
            return Ok(None);
        };
        existing.effect = input.effect;
        existing.enabled = input.enabled;
        existing.label = input.label;
        existing.matcher = input.matcher;
        existing.updated_at = Some(Utc::now());
        let updated = existing.clone();
        if let Some(store) = &self.agent_rule_store {
            store.upsert_agent_rule(agent_id, &updated)?;
        }
        self.upsert_runtime_rule(updated.clone()).await;
        Ok(Some(updated))
    }

    pub async fn delete_agent_rule(&self, agent_id: &str, rule_id: &str) -> Result<bool, String> {
        let removed_memory = self.reuse_rules.write().await.remove(rule_id).is_some();
        let removed_store = if let Some(store) = &self.agent_rule_store {
            store.delete_agent_rule(agent_id, rule_id)?
        } else {
            false
        };
        Ok(removed_memory || removed_store)
    }

    async fn upsert_runtime_rule(&self, rule: ApprovalReuseRule) {
        let mut rules = self.reuse_rules.write().await;
        upsert_runtime_rule(&mut rules, rule);
    }
}

impl Default for ApprovalAuthority {
    fn default() -> Self {
        Self::new()
    }
}

fn upsert_runtime_rule(rules: &mut HashMap<String, ApprovalReuseRule>, rule: ApprovalReuseRule) {
    rules.retain(|_, existing| !same_permission_rule_key(existing, &rule));
    rules.insert(rule.id.clone(), rule);
}

fn same_permission_rule_key(left: &ApprovalReuseRule, right: &ApprovalReuseRule) -> bool {
    let same_owner = match left.scope {
        ApprovalReuseScope::Agent => left.agent_id == right.agent_id,
        ApprovalReuseScope::Chat | ApprovalReuseScope::RuntimeSession => left.owner == right.owner,
    };
    same_owner && left.scope == right.scope && left.matcher == right.matcher
}

fn reuse_options_for(
    owner: &ApprovalOwner,
    tool_name: &str,
    args: &serde_json::Value,
) -> Vec<ApprovalReuseOption> {
    let input_params = input_params_for_tool_call(tool_name, args);
    let mut matchers = vec![ApprovalReuseMatcher::ToolCall {
        tool_name: tool_name.to_string(),
        input_params: input_params.clone(),
    }];

    if let Some(command) = input_params
        .as_deref()
        .filter(|_| is_bash_tool_name(tool_name))
    {
        if let Some(prefix) = bash_command_reuse_prefix(command) {
            let pattern = format!("{prefix} *");
            if Some(pattern.as_str()) != input_params.as_deref() {
                matchers.push(ApprovalReuseMatcher::ToolCall {
                    tool_name: tool_name.to_string(),
                    input_params: Some(pattern),
                });
            }
        }
    };

    matchers
        .into_iter()
        .flat_map(|matcher| {
            let display = matcher.display_pattern();
            let local = match owner {
                ApprovalOwner::Chat { .. } => ApprovalReuseOption {
                    scope: ApprovalReuseScope::Chat,
                    label: format!("Allow {display} in this chat"),
                    matcher: matcher.clone(),
                },
                ApprovalOwner::RuntimeSession { .. } => ApprovalReuseOption {
                    scope: ApprovalReuseScope::RuntimeSession,
                    label: format!("Allow {display} in this runtime session"),
                    matcher: matcher.clone(),
                },
            };
            [
                local,
                ApprovalReuseOption {
                    scope: ApprovalReuseScope::Agent,
                    label: format!("Always allow {display} for this agent"),
                    matcher,
                },
            ]
        })
        .collect()
}

fn matcher_matches_tool_call(
    matcher: &ApprovalReuseMatcher,
    tool_name: &str,
    args: &serde_json::Value,
) -> bool {
    match matcher {
        ApprovalReuseMatcher::ToolCall {
            tool_name: tool_pattern,
            input_params,
        } => {
            glob_match(tool_pattern, tool_name)
                && input_params.as_ref().is_none_or(|pattern| {
                    input_params_for_tool_call(tool_name, args)
                        .is_some_and(|params| glob_match(pattern, &params))
                })
        }
    }
}

impl ApprovalReuseMatcher {
    fn display_pattern(&self) -> String {
        match self {
            ApprovalReuseMatcher::ToolCall {
                tool_name,
                input_params: Some(input_params),
            } => format!("{tool_name}({input_params})"),
            ApprovalReuseMatcher::ToolCall {
                tool_name,
                input_params: None,
            } => tool_name.clone(),
        }
    }
}

fn glob_match(pattern: &str, value: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    let Some((prefix, suffix)) = pattern.split_once('*') else {
        return pattern == value;
    };
    if suffix.is_empty() && prefix.ends_with(' ') {
        let base = prefix.trim_end();
        return value == base || value.starts_with(prefix);
    }
    value.starts_with(prefix) && value.ends_with(suffix)
}

fn is_bash_tool_name(tool_name: &str) -> bool {
    tool_name == "bash"
}

fn input_params_for_tool_call(tool_name: &str, args: &serde_json::Value) -> Option<String> {
    if is_bash_tool_name(tool_name) {
        return normalized_bash_command(args);
    }
    if args.is_null() {
        return None;
    }
    Some(args.to_string())
}

fn normalized_bash_command(args: &serde_json::Value) -> Option<String> {
    let command = args.get("command")?.as_str()?;
    let tokens = shlex::split(command)?;
    (!tokens.is_empty()).then(|| tokens.join(" "))
}

fn bash_command_reuse_prefix(command: &str) -> Option<String> {
    let tokens = shlex::split(command)?;
    let program = tokens.first()?.as_str();
    let second = tokens.get(1)?.as_str();
    if second == "|" || second.contains('>') || second.contains('<') || looks_like_path(second) {
        return None;
    }
    Some(format!("{program} {second}"))
}

fn looks_like_path(token: &str) -> bool {
    token.starts_with('/')
        || token.starts_with("./")
        || token.starts_with("../")
        || token.starts_with("~/")
}
