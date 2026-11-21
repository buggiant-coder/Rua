use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, Weak};

use chrono::{DateTime, Utc};
use rua_core::ChatAddress;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, Mutex as AsyncMutex, OwnedMutexGuard, RwLock};
use uuid::Uuid;

mod local_chat_read;

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
    TargetDevice,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exact_read_args: Option<bool>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exact_read_args: Option<bool>,
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
    fn same_permission(&self, other: &Self) -> bool {
        self.id == other.id
            && self.agent_id == other.agent_id
            && self.owner == other.owner
            && self.scope == other.scope
            && self.matcher == other.matcher
            && self.source_approval_id == other.source_approval_id
            && self.source == other.source
            && self.exact_read_args == other.exact_read_args
            && self.enabled == other.enabled
            && self.effect == other.effect
    }

    pub(crate) fn pattern(&self) -> PermissionRulePattern<'_> {
        self.matcher.pattern(
            self.exact_read_args,
            self.effect,
            self.source,
            &self.source_approval_id,
        )
    }

    pub fn to_stored(&self) -> StoredApprovalReuseRule {
        StoredApprovalReuseRule {
            id: self.id.clone(),
            label: self.label.clone(),
            matcher: self.matcher.clone(),
            exact_read_args: self.exact_read_args,
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
    pub(crate) fn pattern(&self) -> PermissionRulePattern<'_> {
        self.matcher.pattern(
            self.exact_read_args,
            self.effect,
            self.source,
            &self.source_approval_id,
        )
    }

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
            exact_read_args: self.exact_read_args,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthenticatedApprovalDecision {
    RunOnce,
    AllowChat,
    AllowRuntimeSession,
    AllowAgent,
    Reject,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthenticatedUserDecision {
    pub user_id: String,
    pub decision: AuthenticatedApprovalDecision,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
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
    /// Set by the accepting authority from its authenticated user session.
    /// Look up the request in that authority when verifying the source of a decision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authenticated_user_decision: Option<AuthenticatedUserDecision>,
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
struct CachedRule {
    rule: ApprovalReuseRule,
    // Only a successful local authenticated persistent read decision can set this bit.
    locally_decided_read: bool,
}

#[derive(Debug)]
pub struct ApprovalAuthority {
    chat_registrations: Mutex<HashMap<ChatAddress, Weak<AsyncMutex<()>>>>,
    requests: RwLock<HashMap<String, ApprovalRequest>>,
    // Publishing, loading or revoking a Chat rule holds this lock across its store operation.
    // A cold lookup cannot overwrite a concurrent local decision or restore a revoked row.
    reuse_rules: RwLock<HashMap<String, CachedRule>>,
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
}

impl ApprovalAuthority {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(BROADCAST_CAPACITY);
        Self {
            chat_registrations: Mutex::default(),
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
            chat_registrations: Mutex::default(),
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
            chat_registrations: Mutex::default(),
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

    /// An Agent switch holds this Chat's registration guard from its pending
    /// approval check through publication of the new selection.
    pub async fn hold_chat_registrations(&self, address: &ChatAddress) -> OwnedMutexGuard<()> {
        let gate = {
            let mut gates = self
                .chat_registrations
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            if let Some(gate) = gates.get(address).and_then(Weak::upgrade) {
                gate
            } else {
                if gates.len() >= 256 {
                    gates.retain(|_, gate| gate.strong_count() > 0);
                }
                let gate = Arc::new(AsyncMutex::new(()));
                gates.insert(address.clone(), Arc::downgrade(&gate));
                gate
            }
        };
        gate.lock_owned().await
    }

    pub async fn create_request(&self, input: ApprovalRequestInput) -> ApprovalRequest {
        self.create_request_for_device(input, None).await
    }

    pub async fn create_request_for_device(
        &self,
        input: ApprovalRequestInput,
        local_device_id: Option<&str>,
    ) -> ApprovalRequest {
        self.register_request(input, local_device_id).await.0
    }

    /// The target needs a one-time decision only when this invocation has no earlier card.
    pub async fn create_target_read_confirmation(
        &self,
        mut input: ApprovalRequestInput,
    ) -> Option<ApprovalRequest> {
        input.trigger.source = ApprovalTriggerSource::TargetDevice;
        let (request, created) = self.register_request(input, None).await;
        created.then_some(request)
    }

    async fn register_request(
        &self,
        input: ApprovalRequestInput,
        local_device_id: Option<&str>,
    ) -> (ApprovalRequest, bool) {
        let _registration = match input.owner.as_chat_address() {
            Some(address) => Some(self.hold_chat_registrations(address).await),
            None => None,
        };
        let call =
            PermissionToolCall::new(&input.tool_name, &input.tool_call.args, local_device_id);
        let target_confirmation = input.trigger.source == ApprovalTriggerSource::TargetDevice;
        let reuse_options = if target_confirmation {
            Vec::new()
        } else {
            reuse_options_for(
                &input.owner,
                &input.tool_name,
                call.bound_read.as_deref().unwrap_or(&input.tool_call.args),
            )
        };
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
            authenticated_user_decision: None,
            reuse_options,
        };

        let mut requests = self.requests.write().await;
        if target_confirmation {
            if let Some(existing) = requests.values().find(|prior| {
                prior.owner == request.owner
                    && prior.turn_id == request.turn_id
                    && prior.tool_call_id == request.tool_call_id
            }) {
                return (existing.clone(), false);
            }
        }
        requests.insert(request.id.clone(), request.clone());
        drop(requests);
        let _ = self.tx.send(ApprovalEvent {
            event: "approval.created".into(),
            request: request.clone(),
        });
        (request, true)
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
            authenticated_user_decision: None,
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
        self.supersede_pending(|request| &request.owner == owner)
            .await
    }

    pub async fn supersede_request(&self, id: &str) {
        self.supersede_pending(|request| request.id == id).await;
    }

    async fn supersede_pending(&self, selected: impl Fn(&ApprovalRequest) -> bool) -> usize {
        let mut requests = self.requests.write().await;
        let now = Utc::now();
        let ids = requests
            .values()
            .filter(|request| selected(request) && request.status == ApprovalStatus::Pending)
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
        self.decide_inner(None, id, decision, None).await
    }

    pub async fn decide_for_agent(
        &self,
        agent_id: &str,
        id: &str,
        decision: ApprovalDecision,
    ) -> Result<ApprovalRequest, ApprovalAuthorityError> {
        self.decide_inner(Some(agent_id), id, decision, None).await
    }

    /// The caller obtains the user identity from the authority's own authentication context.
    pub async fn decide_for_authenticated_user(
        &self,
        agent_id: &str,
        id: &str,
        decision: ApprovalDecision,
        authenticated_user_id: &str,
    ) -> Result<ApprovalRequest, ApprovalAuthorityError> {
        if agent_id.trim().is_empty() {
            return Err(ApprovalAuthorityError::NotFound(id.into()));
        }
        self.decide_inner(Some(agent_id), id, decision, Some(authenticated_user_id))
            .await
    }

    async fn decide_inner(
        &self,
        expected_agent_id: Option<&str>,
        id: &str,
        decision: ApprovalDecision,
        authenticated_user_id: Option<&str>,
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
        if authenticated_user_id.is_some_and(|user_id| user_id.trim().is_empty()) {
            return Err(ApprovalAuthorityError::InvalidState {
                id: id.into(),
                current: request.status,
                attempted: "record a decision without an authenticated user identity",
            });
        }

        let mut rule_to_insert: Option<ApprovalReuseRule> = None;
        if request.trigger.source == ApprovalTriggerSource::TargetDevice
            && matches!(decision, ApprovalDecision::RunOnce { .. })
            && authenticated_user_id.is_none()
        {
            return Err(ApprovalAuthorityError::InvalidState {
                id: id.into(),
                current: request.status,
                attempted: "confirm a target Device check without an authenticated user",
            });
        }
        let (status, decided_by, kind) = match decision {
            ApprovalDecision::RunOnce { decided_by } => (
                ApprovalStatus::Approved,
                decided_by,
                AuthenticatedApprovalDecision::RunOnce,
            ),
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
                    exact_read_args: matcher.read_args_with_asterisk().then_some(true),
                    matcher,
                    source_approval_id: request.id.clone(),
                    created_at: Utc::now(),
                    updated_at: None,
                    enabled: true,
                    effect: PermissionRuleEffect::Allow,
                    source: PermissionRuleSource::Approval,
                    hit_count: 0,
                });
                (
                    ApprovalStatus::Approved,
                    decided_by,
                    AuthenticatedApprovalDecision::AllowChat,
                )
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
                    exact_read_args: matcher.read_args_with_asterisk().then_some(true),
                    matcher,
                    source_approval_id: request.id.clone(),
                    created_at: Utc::now(),
                    updated_at: None,
                    enabled: true,
                    effect: PermissionRuleEffect::Allow,
                    source: PermissionRuleSource::Approval,
                    hit_count: 0,
                });
                (
                    ApprovalStatus::Approved,
                    decided_by,
                    AuthenticatedApprovalDecision::AllowRuntimeSession,
                )
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
                    exact_read_args: matcher.read_args_with_asterisk().then_some(true),
                    matcher,
                    source_approval_id: request.id.clone(),
                    created_at: Utc::now(),
                    updated_at: None,
                    enabled: true,
                    effect: PermissionRuleEffect::Allow,
                    source: PermissionRuleSource::Approval,
                    hit_count: 0,
                });
                (
                    ApprovalStatus::Approved,
                    decided_by,
                    AuthenticatedApprovalDecision::AllowAgent,
                )
            }
            ApprovalDecision::Reject { decided_by } => (
                ApprovalStatus::Rejected,
                decided_by,
                AuthenticatedApprovalDecision::Reject,
            ),
        };

        request.status = status;
        request.decided_at = Some(Utc::now());
        request.decided_by = Some(decided_by);
        request.authenticated_user_decision =
            authenticated_user_id.map(|user_id| AuthenticatedUserDecision {
                user_id: user_id.to_owned(),
                decision: kind,
                extra: BTreeMap::new(),
            });
        let updated = request.clone();

        if let Some(rule) = rule_to_insert {
            drop(requests);
            let mut rules = self.reuse_rules.write().await;
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
            let locally_decided_read = authenticated_user_id.is_some()
                && matches!(
                    kind,
                    AuthenticatedApprovalDecision::AllowChat
                        | AuthenticatedApprovalDecision::AllowAgent
                )
                && updated.tool_name == "read";
            cache_rule(&mut rules, rule, locally_decided_read);
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
                && request.trigger.source != ApprovalTriggerSource::TargetDevice
                && request.status == ApprovalStatus::Approved
        })
    }

    /// Claim the user's one-time decision for this exact Chat file read.
    /// The target still has to verify the local file and apply its permissions.
    pub async fn consume_authenticated_read_once(
        &self,
        ctx: &crate::ToolCallContext,
        target_device_id: &str,
        file_path: &str,
    ) -> Option<String> {
        self.consume_authenticated_read_once_if(ctx, target_device_id, file_path, |_| true)
            .await
    }

    /// Check the live dispatch under the approval write lock immediately before consumption.
    pub async fn consume_authenticated_read_once_if(
        &self,
        ctx: &crate::ToolCallContext,
        target_device_id: &str,
        file_path: &str,
        claim_dispatch: impl FnOnce(&str) -> bool,
    ) -> Option<String> {
        let address = ctx.approval_owner.as_chat_address()?;
        let turn_id = ctx.metadata.get("turn_id")?.as_str()?;
        let tool_call_id = ctx.metadata.get("tool_call_id")?.as_str()?;
        let args = ctx.tool_input.as_object()?;
        if ctx.tool_name != "read"
            || [
                ctx.agent_id.as_str(),
                address.chat_id().as_str(),
                turn_id,
                tool_call_id,
                target_device_id,
            ]
            .iter()
            .any(|id| id.trim().is_empty())
            || args.get("device_id")?.as_str()? != target_device_id
            || args.get("file_path")?.as_str()? != file_path
            || !rua_core::execution_directory::is_device_absolute_path(file_path)
            || file_path.starts_with("//")
            || file_path.starts_with(r"\\")
            || args.keys().any(|key| {
                !matches!(
                    key.as_str(),
                    "device_id" | "file_path" | "start_line" | "limit" | "byte_offset"
                )
            })
        {
            return None;
        }

        let mut requests = self.requests.write().await;
        let mut matches = requests.values_mut().filter(|request| {
            request.agent_id == ctx.agent_id
                && request.owner == ctx.approval_owner
                && request.turn_id == turn_id
                && request.tool_call_id == tool_call_id
        });
        let request = matches.next()?;
        if matches.next().is_some()
            || request.status != ApprovalStatus::Approved
            || request.tool_name != ctx.tool_name
            || request.tool_call.args != ctx.tool_input
            || request.authenticated_user_decision.as_ref()?.decision
                != AuthenticatedApprovalDecision::RunOnce
            || !claim_dispatch(&request.id)
        {
            return None;
        }

        request.status = ApprovalStatus::Consumed;
        let _ = self.tx.send(ApprovalEvent {
            event: "approval.updated".into(),
            request: request.clone(),
        });
        Some(request.id.clone())
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
            &PermissionToolCall::new(tool_name, args, None),
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
        self.find_rule_in_snapshot(
            agent_id,
            owner,
            effect,
            &PermissionToolCall::new(tool_name, args, None),
            snapshot_rules,
        )
        .await
    }

    pub(crate) async fn find_rule_in_snapshot(
        &self,
        agent_id: &str,
        owner: &ApprovalOwner,
        effect: PermissionRuleEffect,
        call: &PermissionToolCall<'_>,
        snapshot_rules: &[StoredApprovalReuseRule],
    ) -> Result<Option<ApprovalReuseRule>, String> {
        let mut rules = self.reuse_rules.write().await;
        let stored_chat_rules = match (owner.as_chat_address(), self.chat_rule_store.as_ref()) {
            (Some(address), Some(store)) => store.list_chat_rules(address)?,
            _ => Vec::new(),
        };
        if let Some(rule) = rules
            .values_mut()
            .map(|entry| &mut entry.rule)
            .find(|rule| {
                matches!(
                    rule.scope,
                    ApprovalReuseScope::Chat | ApprovalReuseScope::RuntimeSession
                ) && rule.owner.as_ref() == Some(owner)
                    && rule.effect == effect
                    && rule.enabled
                    && rule.pattern().matches(call)
            })
        {
            rule.hit_count += 1;
            return Ok(Some(rule.clone()));
        }
        if let Some(address) = owner.as_chat_address() {
            if let Some(store) = &self.chat_rule_store {
                if let Some(mut rule) = stored_chat_rules.into_iter().find(|rule| {
                    rule.enabled && rule.effect == effect && rule.pattern().matches(call)
                }) {
                    rule.hit_count += 1;
                    let _ = store.upsert_chat_rule(address, &rule);
                    cache_rule(&mut rules, rule.clone(), false);
                    return Ok(Some(rule));
                }
            }
        }
        Ok(snapshot_rules
            .iter()
            .find(|rule| rule.enabled && rule.effect == effect && rule.pattern().matches(call))
            .cloned()
            .map(|rule| rule.into_runtime(agent_id.to_string(), None, ApprovalReuseScope::Agent)))
    }

    pub async fn find_deny_rule(
        &self,
        agent_id: &str,
        tool_name: &str,
        args: &serde_json::Value,
    ) -> Option<ApprovalReuseRule> {
        self.find_permission_rule(
            agent_id,
            None,
            PermissionRuleEffect::Deny,
            &PermissionToolCall::new(tool_name, args, None),
        )
        .await
    }

    pub(crate) async fn find_permission_rule(
        &self,
        agent_id: &str,
        owner: Option<&ApprovalOwner>,
        effect: PermissionRuleEffect,
        call: &PermissionToolCall<'_>,
    ) -> Option<ApprovalReuseRule> {
        let mut rules = self.reuse_rules.write().await;
        let inspect_stored_read =
            self.agent_rule_store.is_some() && is_file_read(call.tool_name, call.args);
        if let Some(rule) = rules
            .values_mut()
            .map(|entry| &mut entry.rule)
            .find(|rule| {
                owner.is_some_and(|owner| rule.scope == owner.reuse_scope())
                    && rule.owner.as_ref() == owner
                    && rule.effect == effect
                    && rule.enabled
                    && rule.pattern().matches(call)
            })
        {
            rule.hit_count += 1;
            return Some(rule.clone());
        }

        if let Some(rule) = rules
            .values_mut()
            .map(|entry| &mut entry.rule)
            .find(|rule| {
                rule.agent_id == agent_id
                    && rule.scope == ApprovalReuseScope::Agent
                    && !inspect_stored_read
                    && rule.effect == effect
                    && rule.enabled
                    && rule.pattern().matches(call)
            })
        {
            rule.hit_count += 1;
            if let Some(store) = &self.agent_rule_store {
                let _ = store.upsert_agent_rule(agent_id, rule);
            }
            return Some(rule.clone());
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
                        && rule.pattern().matches(call)
                })
            {
                rule.hit_count += 1;
                let _ = store.upsert_chat_rule(address, &rule);
                cache_rule(&mut rules, rule.clone(), false);
                return Some(rule);
            }
        }

        let store = self.agent_rule_store.as_ref()?;
        let mut rule = self
            .current_agent_rules(agent_id, &mut rules)
            .ok()?
            .into_iter()
            .find(|rule| {
                rule.agent_id == agent_id
                    && rule.scope == ApprovalReuseScope::Agent
                    && rule.effect == effect
                    && rule.enabled
                    && rule.pattern().matches(call)
            })?;
        rule.hit_count += 1;
        // Counting an exact file read must not write an old permission back into synchronized policy.
        let local = rules
            .get(&rule.id)
            .is_some_and(|entry| entry.locally_decided_read && entry.rule.same_permission(&rule));
        if !inspect_stored_read {
            let _ = store.upsert_agent_rule(agent_id, &rule);
        }
        cache_rule(&mut rules, rule.clone(), local);
        Some(rule)
    }

    /// Reconcile locally signed Agent reads with current settings before using either copy.
    fn current_agent_rules(
        &self,
        agent_id: &str,
        cache: &mut HashMap<String, CachedRule>,
    ) -> Result<Vec<ApprovalReuseRule>, String> {
        let scoped = |rule: &ApprovalReuseRule| {
            rule.scope == ApprovalReuseScope::Agent && rule.agent_id == agent_id
        };
        let Some(store) = &self.agent_rule_store else {
            return Ok(cache
                .values()
                .map(|entry| &entry.rule)
                .filter(|rule| scoped(rule))
                .cloned()
                .collect());
        };
        let mut current = store.list_agent_rules(agent_id)?;
        current.retain(|rule| scoped(rule) && rule.owner.is_none());
        for entry in cache
            .values_mut()
            .filter(|entry| entry.locally_decided_read && scoped(&entry.rule))
        {
            entry.locally_decided_read =
                current.iter().any(|rule| entry.rule.same_permission(rule));
        }
        for rule in &mut current {
            if let Some(entry) = cache
                .get(&rule.id)
                .filter(|entry| entry.rule.same_permission(rule))
            {
                rule.hit_count = rule.hit_count.max(entry.rule.hit_count);
            }
        }
        Ok(current)
    }

    pub async fn list_agent_rules(&self, agent_id: &str) -> Vec<ApprovalReuseRule> {
        let mut cache = self.reuse_rules.write().await;
        let mut rules = self
            .current_agent_rules(agent_id, &mut cache)
            .unwrap_or_default();
        rules.sort_by_key(|rule| rule.created_at);
        rules
    }

    pub async fn list_chat_rules(&self, address: &ChatAddress) -> Vec<ApprovalReuseRule> {
        let mut rules = self
            .reuse_rules
            .read()
            .await
            .values()
            .map(|entry| &entry.rule)
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

    pub(crate) async fn chat_rules_for_inspection(
        &self,
        address: &ChatAddress,
    ) -> Result<Vec<ApprovalReuseRule>, String> {
        match &self.chat_rule_store {
            Some(store) => store.list_chat_rules(address),
            None => Ok(self.list_chat_rules(address).await),
        }
    }

    pub async fn delete_chat_rules(&self, address: &ChatAddress) -> Result<(), String> {
        let mut rules = self.reuse_rules.write().await;
        if let Some(store) = &self.chat_rule_store {
            store.delete_chat_rules(address)?;
        }
        rules.retain(|_, entry| {
            let rule = &entry.rule;
            rule.scope != ApprovalReuseScope::Chat
                || rule.owner.as_ref() != Some(&ApprovalOwner::chat(address.clone()))
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
            exact_read_args: input.matcher.read_args_with_asterisk().then_some(false),
            matcher: input.matcher,
            source_approval_id: String::new(),
            created_at: now,
            updated_at: Some(now),
            enabled: input.enabled,
            effect: input.effect,
            source: PermissionRuleSource::Manual,
            hit_count: 0,
        };
        let mut cache = self.reuse_rules.write().await;
        if let Some(store) = &self.agent_rule_store {
            store.upsert_agent_rule(agent_id, &rule)?;
        }
        cache_rule(&mut cache, rule.clone(), false);
        Ok(rule)
    }

    pub async fn update_agent_rule(
        &self,
        agent_id: &str,
        rule_id: &str,
        input: PermissionRuleInput,
    ) -> Result<Option<ApprovalReuseRule>, String> {
        let mut cache = self.reuse_rules.write().await;
        let mut rules = self.current_agent_rules(agent_id, &mut cache)?;
        let Some(existing) = rules.iter_mut().find(|rule| rule.id == rule_id) else {
            return Ok(None);
        };
        if existing.matcher != input.matcher {
            existing.exact_read_args = (input.matcher.read_args_with_asterisk()
                || (existing.source == PermissionRuleSource::Approval
                    && input.matcher.file_read_args().is_some()))
            .then_some(false);
        } else if existing.exact_read_args.is_none() && input.matcher.read_args_with_asterisk() {
            existing.exact_read_args = Some(existing.pattern().exact_read_args);
        }
        existing.effect = input.effect;
        existing.enabled = input.enabled;
        existing.label = input.label;
        existing.matcher = input.matcher;
        existing.updated_at = Some(Utc::now());
        let updated = existing.clone();
        if let Some(store) = &self.agent_rule_store {
            store.upsert_agent_rule(agent_id, &updated)?;
        }
        let local = cache.get(rule_id).is_some_and(|entry| {
            entry.locally_decided_read && entry.rule.same_permission(&updated)
        });
        cache_rule(&mut cache, updated.clone(), local);
        Ok(Some(updated))
    }

    pub async fn delete_agent_rule(&self, agent_id: &str, rule_id: &str) -> Result<bool, String> {
        let mut cache = self.reuse_rules.write().await;
        let removed_store = if let Some(store) = &self.agent_rule_store {
            store.delete_agent_rule(agent_id, rule_id)?
        } else {
            false
        };
        let owned = cache.get(rule_id).is_some_and(|entry| {
            entry.rule.agent_id == agent_id && entry.rule.scope == ApprovalReuseScope::Agent
        });
        if owned {
            cache.remove(rule_id);
        }
        Ok(owned || removed_store)
    }
}

fn cache_rule(
    rules: &mut HashMap<String, CachedRule>,
    rule: ApprovalReuseRule,
    locally_decided_read: bool,
) {
    rules.retain(|_, entry| !same_permission_rule_key(&entry.rule, &rule));
    rules.insert(
        rule.id.clone(),
        CachedRule {
            rule,
            locally_decided_read,
        },
    );
}

impl Default for ApprovalAuthority {
    fn default() -> Self {
        Self::new()
    }
}

fn same_permission_rule_key(left: &ApprovalReuseRule, right: &ApprovalReuseRule) -> bool {
    let same_owner = match left.scope {
        ApprovalReuseScope::Agent => left.agent_id == right.agent_id,
        ApprovalReuseScope::Chat | ApprovalReuseScope::RuntimeSession => left.owner == right.owner,
    };
    same_owner && left.scope == right.scope && left.pattern().same_matcher(&right.pattern())
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

pub(crate) struct PermissionToolCall<'a> {
    tool_name: &'a str,
    args: &'a serde_json::Value,
    bound_read: Option<std::borrow::Cow<'a, serde_json::Value>>,
    unqualified_local_read: Option<serde_json::Value>,
}

fn is_file_read(tool_name: &str, args: &serde_json::Value) -> bool {
    tool_name == "read"
        && args.get("device_id").is_none_or(|device| {
            device
                .as_str()
                .is_some_and(|id| !id.is_empty() && id.trim() == id)
        })
        && args["file_path"].as_str().is_some_and(|path| {
            rua_core::execution_directory::is_device_absolute_path(path)
                && rua_core::chat_media::reference_id(path).is_none()
        })
}

impl<'a> PermissionToolCall<'a> {
    pub(crate) fn new(
        tool_name: &'a str,
        args: &'a serde_json::Value,
        local_device_id: Option<&str>,
    ) -> Self {
        use std::borrow::Cow;
        let mut call = Self {
            tool_name,
            args,
            bound_read: None,
            unqualified_local_read: None,
        };
        let Some(local) = local_device_id.filter(|id| !id.is_empty() && id.trim() == *id) else {
            return call;
        };
        if !is_file_read(tool_name, args) {
            return call;
        }
        match args["device_id"].as_str() {
            None => {
                let mut bound = args.clone();
                bound["device_id"] = local.into();
                call.bound_read = Some(Cow::Owned(bound));
            }
            Some(id) => {
                call.bound_read = Some(Cow::Borrowed(args));
                if id == local {
                    call.unqualified_local_read = Self::without_device_id(args);
                }
            }
        }
        call
    }

    fn without_device_id(args: &serde_json::Value) -> Option<serde_json::Value> {
        let mut fields = args.as_object()?.clone();
        fields.remove("device_id")?;
        Some(fields.into())
    }

    pub(crate) fn target_read_for_denial(
        args: &'a serde_json::Value,
        local_device_id: Option<&str>,
    ) -> Self {
        let mut call = Self::new("read", args, local_device_id);
        // Older target-side callers already checked the destination but supplied it only in args.
        if local_device_id.is_none() {
            call.unqualified_local_read = Self::without_device_id(args);
        }
        call
    }

    fn alternate_local_args(&self) -> Option<&serde_json::Value> {
        self.unqualified_local_read
            .as_ref()
            .or(match &self.bound_read {
                Some(std::borrow::Cow::Owned(qualified)) => Some(qualified),
                _ => None,
            })
    }
}

pub(crate) struct PermissionRulePattern<'a> {
    matcher: &'a ApprovalReuseMatcher,
    exact_read_args: bool,
    approved_file_args: Option<serde_json::Value>,
    effect: PermissionRuleEffect,
}

impl PermissionRulePattern<'_> {
    pub(crate) fn same_matcher(&self, other: &Self) -> bool {
        self.matcher == other.matcher
            && self.exact_read_args == other.exact_read_args
            && self.approved_file_args.is_some() == other.approved_file_args.is_some()
    }

    pub(crate) fn matches(&self, call: &PermissionToolCall<'_>) -> bool {
        if let (Some(approved), Some(bound)) =
            (&self.approved_file_args, call.bound_read.as_deref())
        {
            // A legacy file approval without a Device cannot name this destination.
            return approved == bound;
        }
        self.matches_args(call.tool_name, call.args)
            || self.effect == PermissionRuleEffect::Deny
                && call
                    .alternate_local_args()
                    .is_some_and(|args| self.matches_args(call.tool_name, args))
    }

    fn matches_args(&self, tool_name: &str, args: &serde_json::Value) -> bool {
        let ApprovalReuseMatcher::ToolCall {
            tool_name: tool_pattern,
            input_params,
        } = self.matcher;
        glob_match(tool_pattern, tool_name)
            && input_params.as_ref().is_none_or(|pattern| {
                input_params_for_tool_call(tool_name, args).is_some_and(|params| {
                    if self.exact_read_args {
                        pattern == &params
                    } else {
                        glob_match(pattern, &params)
                    }
                })
            })
    }
}

impl ApprovalReuseMatcher {
    fn file_read_args(&self) -> Option<serde_json::Value> {
        let Self::ToolCall {
            tool_name,
            input_params,
        } = self;
        if tool_name != "read" {
            return None;
        }
        let args = serde_json::from_str(input_params.as_deref()?).ok()?;
        is_file_read(tool_name, &args).then_some(args)
    }

    fn read_args_with_asterisk(&self) -> bool {
        matches!(self, Self::ToolCall { tool_name, input_params: Some(params) }
            if tool_name == "read" && params.contains('*'))
    }

    fn pattern(
        &self,
        exact_read_args: Option<bool>,
        effect: PermissionRuleEffect,
        source: PermissionRuleSource,
        source_approval_id: &str,
    ) -> PermissionRulePattern<'_> {
        // Older rules recorded who created them but did not distinguish a literal '*' from
        // a wildcard. Narrow only approval-origin allows. Existing denies retain their reach.
        let approved_read = source == PermissionRuleSource::Approval
            && !source_approval_id.is_empty()
            && effect == PermissionRuleEffect::Allow;
        PermissionRulePattern {
            matcher: self,
            exact_read_args: self.read_args_with_asterisk()
                && exact_read_args.unwrap_or(approved_read),
            approved_file_args: if approved_read && exact_read_args != Some(false) {
                self.file_read_args()
            } else {
                None
            },
            effect,
        }
    }

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
