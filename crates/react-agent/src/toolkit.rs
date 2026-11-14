use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use agent_types::{ToolCall, ToolDefinition, ToolResult};
use async_trait::async_trait;
use rua_core::tool::ToolContext;

use crate::error::AgentError;

#[async_trait]
pub trait ToolExecutor: Send + Sync {
    async fn execute(&self, call: &ToolCall, ctx: &ToolContext) -> Result<ToolResult, AgentError>;
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisclosureClass {
    Direct,

    DeferredCandidate,

    Hidden,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolSourceKind {
    Builtin,

    Skill,

    Mcp,

    Plugin,

    External,

    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolDisclosureMeta {
    pub class: DisclosureClass,
    pub source_kind: ToolSourceKind,
    pub source_name: Option<String>,
    pub category: Option<String>,
    pub schema_tokens: Option<usize>,
    pub original_name: Option<String>,
    pub product_capability: Option<String>,
}

impl Default for ToolDisclosureMeta {
    fn default() -> Self {
        Self {
            class: DisclosureClass::Direct,
            source_kind: ToolSourceKind::Unknown,
            source_name: None,
            category: None,
            schema_tokens: None,
            original_name: None,
            product_capability: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolDisclosureEntry {
    pub definition: ToolDefinition,
    pub disclosure: ToolDisclosureMeta,
}

struct ToolEntry {
    definition: ToolDefinition,
    executor: Arc<dyn ToolExecutor>,
    disclosure: ToolDisclosureMeta,
}

impl Clone for ToolEntry {
    fn clone(&self) -> Self {
        Self {
            definition: self.definition.clone(),
            executor: self.executor.clone(),
            disclosure: self.disclosure.clone(),
        }
    }
}

pub struct AgentToolkit {
    // Stable schema order preserves prompt prefixes across capability snapshots.
    entries: BTreeMap<String, ToolEntry>,
}

#[derive(Clone)]
pub struct AgentToolkitHandle {
    current: Arc<RwLock<Arc<AgentToolkit>>>,
}

impl AgentToolkitHandle {
    pub fn new(toolkit: Arc<AgentToolkit>) -> Self {
        Self {
            current: Arc::new(RwLock::new(toolkit)),
        }
    }

    pub fn current(&self) -> Arc<AgentToolkit> {
        self.current
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub fn replace(&self, toolkit: Arc<AgentToolkit>) {
        *self
            .current
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = toolkit;
    }
}

impl AgentToolkit {
    pub fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
        }
    }

    pub fn register(&mut self, definition: ToolDefinition, executor: Arc<dyn ToolExecutor>) {
        self.register_with_disclosure(definition, executor, ToolDisclosureMeta::default());
    }

    pub fn register_with_disclosure(
        &mut self,
        definition: ToolDefinition,
        executor: Arc<dyn ToolExecutor>,
        disclosure: ToolDisclosureMeta,
    ) {
        let name = definition.name.clone();
        self.entries.insert(
            name,
            ToolEntry {
                definition,
                executor,
                disclosure,
            },
        );
    }

    pub fn get_definition(&self, name: &str) -> Option<&ToolDefinition> {
        self.entries.get(name).map(|entry| &entry.definition)
    }

    pub fn get_executor(&self, name: &str) -> Option<Arc<dyn ToolExecutor>> {
        self.entries.get(name).map(|entry| entry.executor.clone())
    }

    pub fn disclosure_for(&self, name: &str) -> Option<&ToolDisclosureMeta> {
        self.entries.get(name).map(|entry| &entry.disclosure)
    }

    pub fn set_source_identity(
        &mut self,
        name: &str,
        source: String,
        original_name: String,
        product_capability: Option<String>,
    ) {
        if let Some(entry) = self.entries.get_mut(name) {
            entry.disclosure.source_name = Some(source);
            entry.disclosure.original_name = Some(original_name);
            entry.disclosure.product_capability = product_capability;
        }
    }

    pub fn disclosure_inventory(&self) -> Vec<ToolDisclosureEntry> {
        self.entries
            .values()
            .map(|entry| ToolDisclosureEntry {
                definition: entry.definition.clone(),
                disclosure: entry.disclosure.clone(),
            })
            .collect()
    }

    pub fn has_tool(&self, name: &str) -> bool {
        self.entries.contains_key(name)
    }

    pub fn canonical_name<'a>(&'a self, name: &'a str) -> Option<&'a str> {
        if self.entries.contains_key(name) {
            Some(name)
        } else {
            None
        }
    }

    pub fn export_definitions(&self) -> Vec<ToolDefinition> {
        self.entries
            .values()
            .map(|entry| entry.definition.clone())
            .collect()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn tool_names(&self) -> Vec<&str> {
        self.entries.keys().map(|s| s.as_str()).collect()
    }

    pub fn register_contract_tool(
        &mut self,
        definition: ToolDefinition,
        tool: Arc<dyn rua_contract::Tool>,
    ) {
        let executor: Arc<dyn ToolExecutor> =
            Arc::new(ContractToolExecutor::new(tool).with_schema(definition.parameters.clone()));
        self.register(definition, executor);
    }

    pub fn create_filtered(&self, disabled: &[String]) -> Self {
        use std::collections::HashSet;
        let disabled: HashSet<&str> = disabled.iter().map(String::as_str).collect();

        let mut filtered = Self::new();
        for (name, entry) in &self.entries {
            if disabled.contains(name.as_str()) {
                continue;
            }
            filtered.entries.insert(name.clone(), entry.clone());
        }
        filtered
    }

    pub fn extend_from(&mut self, other: &Self) {
        self.entries.extend(other.entries.clone());
    }

    pub fn populate_from_registry(&mut self, registry: &rua_tool::ToolRegistry) {
        for (_name, definition, tool) in registry.iter_tools() {
            let executor: Arc<dyn ToolExecutor> = Arc::new(
                ContractToolExecutor::new(tool).with_schema(definition.parameters.clone()),
            );
            self.register(definition, executor);
        }
    }
}

impl Default for AgentToolkit {
    fn default() -> Self {
        Self::new()
    }
}

pub struct ContractToolExecutor {
    tool: Arc<dyn rua_contract::Tool>,
    schema: Option<serde_json::Value>,
}

impl ContractToolExecutor {
    pub fn new(tool: Arc<dyn rua_contract::Tool>) -> Self {
        Self { tool, schema: None }
    }

    pub fn with_schema(mut self, schema: serde_json::Value) -> Self {
        self.schema = Some(schema);
        self
    }
}

#[async_trait]
impl ToolExecutor for ContractToolExecutor {
    async fn execute(&self, call: &ToolCall, ctx: &ToolContext) -> Result<ToolResult, AgentError> {
        if let Some(ref schema) = self.schema {
            if let Err(e) = rua_tool::validate_args(&call.name, &call.arguments, schema) {
                return Ok(ToolResult::error(
                    call.id.clone(),
                    format!("schema validation failed for tool '{}': {e}", call.name),
                ));
            }
        }

        let mut context = ctx.clone();
        context
            .metadata
            .insert("tool_call_id".to_string(), serde_json::json!(call.id));
        let core_result = self
            .tool
            .call(call.arguments.clone(), context)
            .await
            .map_err(|e| AgentError::Tool {
                tool: call.name.clone(),
                message: e.to_string(),
            })?;

        Ok(ToolResult {
            call_id: call.id.clone(),
            name: Some(call.name.clone()),
            content: core_result.content,
            is_error: core_result.is_error,
            duration_ms: core_result.duration_ms,
            attachments: core_result.attachments,
            truncation: core_result.truncation,
            metadata: core_result.metadata,
        })
    }
}

pub struct MockToolExecutor {
    response: ToolResult,

    delay_ms: u64,
}

impl MockToolExecutor {
    pub fn new(response_text: impl Into<String>) -> Self {
        Self {
            response: ToolResult::text("mock", response_text),
            delay_ms: 0,
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            response: ToolResult::error("mock", message),
            delay_ms: 0,
        }
    }

    pub fn with_delay(mut self, delay_ms: u64) -> Self {
        self.delay_ms = delay_ms;
        self
    }
}

#[async_trait]
impl ToolExecutor for MockToolExecutor {
    async fn execute(&self, call: &ToolCall, _ctx: &ToolContext) -> Result<ToolResult, AgentError> {
        if self.delay_ms > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(self.delay_ms)).await;
        }
        let mut result = self.response.clone();
        result.call_id = call.id.clone();
        result.name = Some(call.name.clone());
        Ok(result)
    }
}

pub struct FailingToolExecutor {
    message: String,
}

impl FailingToolExecutor {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

#[async_trait]
impl ToolExecutor for FailingToolExecutor {
    async fn execute(&self, call: &ToolCall, _ctx: &ToolContext) -> Result<ToolResult, AgentError> {
        Err(AgentError::Tool {
            tool: call.name.clone(),
            message: self.message.clone(),
        })
    }
}

pub struct FatalToolExecutor {
    message: String,
}

impl FatalToolExecutor {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

#[async_trait]
impl ToolExecutor for FatalToolExecutor {
    async fn execute(&self, call: &ToolCall, _ctx: &ToolContext) -> Result<ToolResult, AgentError> {
        Err(AgentError::ToolFatal {
            tool: call.name.clone(),
            message: self.message.clone(),
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ToolPolicy {
    Allow,

    Deny { reason: String },

    RequireApproval { prompt: String },
}

#[async_trait]
pub trait ToolPolicyProvider: Send + Sync {
    async fn check(&self, tool_name: &str, args: &serde_json::Value) -> ToolPolicy;
}

#[derive(Debug, Clone, PartialEq)]
pub enum ApprovalOutcome {
    Approved,

    Rejected { reason: String },

    Timeout,
}

#[async_trait]
pub trait ApprovalChannel: Send + Sync {
    async fn request_approval(
        &self,
        call_id: &str,
        tool_name: &str,
        args: &serde_json::Value,
        timeout: Duration,
    ) -> ApprovalOutcome;
}

pub struct GuardedToolExecutor {
    inner: Arc<dyn ToolExecutor>,
    policy: Arc<dyn ToolPolicyProvider>,
    approval: Option<Arc<dyn ApprovalChannel>>,
    approval_timeout: Duration,
    event_tx: Option<tokio::sync::mpsc::UnboundedSender<agent_types::AgentEvent>>,
}

impl GuardedToolExecutor {
    pub fn new(inner: Arc<dyn ToolExecutor>, policy: Arc<dyn ToolPolicyProvider>) -> Self {
        Self {
            inner,
            policy,
            approval: None,
            approval_timeout: Duration::from_secs(300),
            event_tx: None,
        }
    }

    pub fn with_approval(mut self, channel: Arc<dyn ApprovalChannel>, timeout: Duration) -> Self {
        self.approval = Some(channel);
        self.approval_timeout = timeout;
        self
    }

    pub fn with_event_sender(
        mut self,
        tx: tokio::sync::mpsc::UnboundedSender<agent_types::AgentEvent>,
    ) -> Self {
        self.event_tx = Some(tx);
        self
    }
}

#[async_trait]
impl ToolExecutor for GuardedToolExecutor {
    async fn execute(&self, call: &ToolCall, ctx: &ToolContext) -> Result<ToolResult, AgentError> {
        let policy = self.policy.check(&call.name, &call.arguments).await;

        match policy {
            ToolPolicy::Allow => self.inner.execute(call, ctx).await,
            ToolPolicy::Deny { reason } => Ok(ToolResult::error(
                call.id.clone(),
                format!("Tool '{}' denied: {reason}", call.name),
            )),
            ToolPolicy::RequireApproval { prompt } => {
                let channel = match &self.approval {
                    Some(c) => c,
                    None => {
                        return Ok(ToolResult::error(
                            call.id.clone(),
                            format!(
                                "Tool '{}' requires approval but no approval channel configured: {prompt}",
                                call.name
                            ),
                        ));
                    }
                };

                if let Some(tx) = &self.event_tx {
                    let _ = tx.send(agent_types::AgentEvent::ToolApprovalPending {
                        call_id: call.id.clone(),
                        tool_name: call.name.clone(),
                        request_id: None,
                        metadata: serde_json::json!({ "prompt": prompt }),
                    });
                }

                let outcome = channel
                    .request_approval(&call.id, &call.name, &call.arguments, self.approval_timeout)
                    .await;

                match outcome {
                    ApprovalOutcome::Approved => self.inner.execute(call, ctx).await,
                    ApprovalOutcome::Rejected { reason } => Ok(ToolResult::error(
                        call.id.clone(),
                        format!("Tool '{}' rejected: {reason}", call.name),
                    )),
                    ApprovalOutcome::Timeout => Ok(ToolResult::error(
                        call.id.clone(),
                        format!(
                            "Tool '{}' approval timed out after {}s",
                            call.name,
                            self.approval_timeout.as_secs()
                        ),
                    )),
                }
            }
        }
    }
}

pub struct AllowAllPolicy;

#[async_trait]
impl ToolPolicyProvider for AllowAllPolicy {
    async fn check(&self, _tool_name: &str, _args: &serde_json::Value) -> ToolPolicy {
        ToolPolicy::Allow
    }
}

pub struct ListBasedPolicy {
    denied: Vec<String>,
    approval_required: Vec<String>,
}

impl ListBasedPolicy {
    pub fn new(denied: Vec<String>, approval_required: Vec<String>) -> Self {
        Self {
            denied,
            approval_required,
        }
    }
}

#[async_trait]
impl ToolPolicyProvider for ListBasedPolicy {
    async fn check(&self, tool_name: &str, _args: &serde_json::Value) -> ToolPolicy {
        if self.denied.iter().any(|t| t == tool_name) {
            return ToolPolicy::Deny {
                reason: format!("'{tool_name}' is in deny list"),
            };
        }
        if self.approval_required.iter().any(|t| t == tool_name) {
            return ToolPolicy::RequireApproval {
                prompt: format!("Tool '{tool_name}' requires approval"),
            };
        }
        ToolPolicy::Allow
    }
}

#[derive(Debug, Clone)]
struct McpServerEntry {
    server_name: String,
    enabled: bool,
    tools: Vec<ToolDefinition>,
}

pub struct McpToolRegistry {
    servers: HashMap<String, McpServerEntry>,
}

impl McpToolRegistry {
    pub fn new() -> Self {
        Self {
            servers: HashMap::new(),
        }
    }

    pub fn set_server_enabled(&mut self, server_name: &str, enabled: bool) -> bool {
        if let Some(entry) = self.servers.get_mut(server_name) {
            entry.enabled = enabled;
            true
        } else {
            false
        }
    }

    pub fn is_server_enabled(&self, server_name: &str) -> Option<bool> {
        self.servers.get(server_name).map(|e| e.enabled)
    }

    pub fn server_names(&self) -> Vec<&str> {
        self.servers.keys().map(|s| s.as_str()).collect()
    }

    pub fn enabled_tool_definitions(&self) -> Vec<ToolDefinition> {
        self.servers
            .values()
            .filter(|e| e.enabled)
            .flat_map(|entry| {
                entry.tools.iter().map(move |tool| {
                    let mut namespaced = tool.clone();
                    namespaced.name = format!("{}_{}", entry.server_name, tool.name);
                    namespaced
                })
            })
            .collect()
    }

    pub fn filtered_tool_definitions(&self, allowed_servers: &[&str]) -> Vec<ToolDefinition> {
        self.servers
            .values()
            .filter(|e| e.enabled && allowed_servers.contains(&e.server_name.as_str()))
            .flat_map(|entry| {
                entry.tools.iter().map(move |tool| {
                    let mut namespaced = tool.clone();
                    namespaced.name = format!("{}_{}", entry.server_name, tool.name);
                    namespaced
                })
            })
            .collect()
    }

    pub fn resolve_namespace<'a>(&'a self, namespaced_name: &'a str) -> Option<(&'a str, &'a str)> {
        for entry in self.servers.values() {
            let prefix = format!("{}_", entry.server_name);
            if let Some(tool_name) = namespaced_name.strip_prefix(&prefix) {
                if entry.tools.iter().any(|t| t.name == tool_name) {
                    return Some((&entry.server_name, tool_name));
                }
            }
        }
        None
    }

    pub fn server_count(&self) -> usize {
        self.servers.len()
    }

    pub fn enabled_tool_count(&self) -> usize {
        self.servers
            .values()
            .filter(|e| e.enabled)
            .map(|e| e.tools.len())
            .sum()
    }

    pub fn populate_toolkit<F>(&self, toolkit: &mut AgentToolkit, executor_factory: F)
    where
        F: Fn(&str, &str) -> Arc<dyn ToolExecutor>,
    {
        for entry in self.servers.values() {
            if !entry.enabled {
                continue;
            }
            for tool in &entry.tools {
                let namespaced_name = format!("{}_{}", entry.server_name, tool.name);
                let mut def = tool.clone();
                def.name = namespaced_name.clone();
                let executor = executor_factory(&entry.server_name, &tool.name);
                toolkit.register(def, executor);
            }
        }
    }
}

impl Default for McpToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}
