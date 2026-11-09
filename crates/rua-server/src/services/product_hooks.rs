use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use rua_core::ChatAddress;

use crate::services::scope_composition::{
    CompositionInput, CompositionWarning, HookBinding, ResolvedHookBinding, SelectionSource,
};

pub const CHAT_HOOK_CONFIGURATION_META_KEY: &str = "hook_configuration";
pub const NATIVE_TERMINAL_DEFINITION_ID: &str = "rua:native/turn-terminal-audit";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HookSource {
    Native,
    User,
    Project,
    Plugin,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HookStateView {
    pub status: String,
    #[serde(default)]
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HookHandlerView {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub program: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default = "default_true")]
    pub available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_directory: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_resources: Vec<String>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HookRevisionView {
    pub revision: String,
    pub event: String,
    pub capabilities: Vec<String>,
    pub handler: HookHandlerView,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HookDefinitionView {
    pub id: String,
    pub owner_namespace: String,
    pub logical_id: String,
    pub display_name: String,
    pub source: HookSource,
    pub latest_revision: String,
    pub revision: String,
    pub event: String,
    pub capabilities: Vec<String>,
    pub handler_kind: String,
    pub registered: bool,
    pub availability: HookStateView,
    pub validation: HookStateView,
    pub active: bool,
    pub disabled: bool,
    pub revision_changed: bool,
    #[serde(default)]
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HookDefinitionDetailView {
    #[serde(flatten)]
    pub definition: HookDefinitionView,
    pub revisions: Vec<HookRevisionView>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateUserHookDefinition {
    pub logical_id: String,
    pub display_name: String,
    pub event: String,
    #[serde(default)]
    pub capabilities: Vec<String>,
    pub handler: HookHandlerView,
    #[serde(default)]
    pub owner_namespace: Option<String>,
    #[serde(default)]
    pub source: Option<HookSource>,
}

#[derive(Debug, Clone)]
pub struct CreateProjectHookDefinition {
    pub logical_id: String,
    pub display_name: String,
    pub event: String,
    pub capabilities: Vec<String>,
    pub handler: HookHandlerView,
}

#[derive(Debug, Clone)]
pub struct CreatePluginHookDefinition {
    pub logical_id: String,
    pub display_name: String,
    pub event: String,
    pub capabilities: Vec<String>,
    pub handler: HookHandlerView,
    pub available: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReviseUserHookDefinition {
    pub display_name: Option<String>,
    pub event: Option<String>,
    pub capabilities: Option<Vec<String>>,
    pub program: Option<String>,
    pub args: Option<Vec<String>>,
    pub working_directory: Option<String>,
    pub timeout_seconds: Option<u64>,
    pub available: Option<bool>,
}

#[derive(Debug, Clone)]
struct HookDefinitionRecord {
    owner_namespace: String,
    logical_id: String,
    display_name: String,
    source: HookSource,
    revisions: Vec<HookRevisionView>,
    available: bool,
    revision_changed: bool,
    diagnostics: Vec<String>,
}

impl HookDefinitionRecord {
    fn id(&self) -> String {
        format!("{}/{}", self.owner_namespace, self.logical_id)
    }

    fn latest(&self) -> &HookRevisionView {
        self.revisions.last().expect("Definition has a revision")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChatHookConfiguration {
    pub epoch: String,
    pub running_config: rua_config::ChatRunningConfig,
    pub security_config: rua_config::types::security::SecurityConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_assignment: Option<rua_chat::TurnAgentAssignment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_file_content: Option<String>,
    #[serde(default)]
    pub tool_config_snapshot: rua_tool::tool_config_secret_io::ToolConfigSnapshot,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability_configuration: Option<rua_agent::runtime::ChatCapabilityConfiguration>,
    #[serde(default)]
    pub selected_agent_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<serde_json::Value>,
    #[serde(default)]
    pub collaboration_mode: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness_configuration: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authority_device_id: Option<String>,
    pub definition_revisions: Vec<String>,
    #[serde(default)]
    pub effective_hooks: Vec<ResolvedHookBinding>,
    #[serde(default)]
    pub warnings: Vec<CompositionWarning>,
    #[serde(default)]
    pub execution_requirements: Vec<HookExecutionRequirement>,
    #[serde(default, flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl ChatHookConfiguration {
    pub(crate) fn from_persisted(value: serde_json::Value) -> Result<Self, String> {
        let configuration: Self = serde_json::from_value(value)
            .map_err(|error| format!("invalid Chat configuration: {error}"))?;
        configuration.validate()?;
        Ok(configuration)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.epoch.trim().is_empty() {
            return Err("Chat configuration epoch is empty".to_string());
        }
        self.running_config.validate()?;
        if let Some(rules) = self.security_config.extra.get("permission_rules") {
            serde_json::from_value::<
                Vec<rua_security::approval_authority::StoredApprovalReuseRule>,
            >(rules.clone())
            .map_err(|error| format!("Invalid security snapshot permission rules: {error}"))?;
        }
        if let Some(assignment) = self.agent_assignment.as_ref() {
            if assignment.agent_id != self.selected_agent_id {
                return Err(
                    "Chat configuration Agent assignment does not match selection".to_string(),
                );
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChatConfigurationPreview {
    pub current: ChatHookConfiguration,
    pub candidate: ChatHookConfiguration,
    pub applied: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collaboration: Option<rua_session::collaboration::CollaborationConfigurationView>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HookExecutionRequirement {
    pub definition_revision: String,
    pub authority_device_id: String,
    pub handler_kind: String,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub program: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_directory: Option<String>,
    pub timeout_seconds: Option<u64>,
    #[serde(default)]
    pub required_resources: Vec<String>,
    pub available: bool,
}

#[derive(Debug, Clone)]
pub struct ChatConfigurationMaterialization {
    pub selected_agent_id: String,
    pub running_config: rua_config::ChatRunningConfig,
    pub security_config: rua_config::types::security::SecurityConfig,
    pub agent_assignment: Option<rua_chat::TurnAgentAssignment>,
    pub prompt_file_content: Option<String>,
    pub tool_config_snapshot: rua_tool::tool_config_secret_io::ToolConfigSnapshot,
    pub capability_configuration: Option<rua_agent::runtime::ChatCapabilityConfiguration>,
    pub model: Option<serde_json::Value>,
    pub thinking: Option<serde_json::Value>,
    pub collaboration_mode: serde_json::Value,
    pub harness_configuration: Option<serde_json::Value>,
    pub authority_device_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HookRunStatus {
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HookRunView {
    pub invocation_id: String,
    pub agent_id: String,
    pub chat_id: String,
    pub turn_id: String,
    pub event: String,
    pub definition_revision: String,
    pub source: String,
    pub resolved_order: usize,
    pub status: HookRunStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision_summary: Option<String>,
    #[serde(default)]
    pub diagnostics: Vec<String>,
    pub started_at: String,
    pub completed_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProductHookContributionView {
    pub chat_address: ChatAddress,
    pub turn_id: String,
    pub tool_call_id: String,
    pub event: String,
    pub definition_revision: String,
    pub source: String,
    pub resolved_order: usize,
    pub kind: String,
    pub content: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<String>,
}

pub struct ProductHookService {
    definitions: RwLock<HashMap<String, HookDefinitionRecord>>,
    agent_activations: RwLock<HashMap<String, HashSet<String>>>,
    project_bindings: RwLock<HashMap<String, Vec<HookBinding>>>,
    chat_bindings: RwLock<HashMap<ChatAddress, Vec<HookBinding>>>,
    chat_configurations: RwLock<HashMap<ChatAddress, ChatHookConfiguration>>,
    configuration_previews: RwLock<HashMap<ChatAddress, ChatConfigurationPreview>>,
    frozen_turns: RwLock<HashMap<(ChatAddress, String), ChatHookConfiguration>>,
    runs: RwLock<Vec<HookRunView>>,
    contributions: RwLock<Vec<ProductHookContributionView>>,
}

impl Default for ProductHookService {
    fn default() -> Self {
        let native = HookDefinitionRecord {
            owner_namespace: "rua:native".to_string(),
            logical_id: "turn-terminal-audit".to_string(),
            display_name: "Turn terminal audit".to_string(),
            source: HookSource::Native,
            revisions: vec![HookRevisionView {
                revision: "v1".to_string(),
                event: "turn.terminal".to_string(),
                capabilities: vec!["identity".to_string()],
                handler: HookHandlerView {
                    kind: "native".to_string(),
                    executable: Some("turn_terminal_audit".to_string()),
                    program: None,
                    args: Vec::new(),
                    available: true,
                    working_directory: None,
                    timeout_seconds: None,
                    required_resources: Vec::new(),
                },
            }],
            available: true,
            revision_changed: false,
            diagnostics: Vec::new(),
        };
        Self {
            definitions: RwLock::new(HashMap::from([(native.id(), native)])),
            agent_activations: RwLock::new(HashMap::new()),
            project_bindings: RwLock::new(HashMap::new()),
            chat_bindings: RwLock::new(HashMap::new()),
            chat_configurations: RwLock::new(HashMap::new()),
            configuration_previews: RwLock::new(HashMap::new()),
            frozen_turns: RwLock::new(HashMap::new()),
            runs: RwLock::new(Vec::new()),
            contributions: RwLock::new(Vec::new()),
        }
    }
}

impl ProductHookService {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn definitions_for_agent(&self, agent_id: &str) -> Vec<HookDefinitionView> {
        let definitions = self.definitions.read().await;
        let activations = self.agent_activations.read().await;
        let active = activations.get(agent_id);
        let mut views = definitions
            .values()
            .filter(|record| definition_visible_to_agent(record, agent_id))
            .map(|record| {
                definition_view(
                    record,
                    active.is_some_and(|ids| activation_selects_definition(ids, &record.id())),
                )
            })
            .collect::<Vec<_>>();
        views.sort_by(|left, right| left.id.cmp(&right.id));
        views
    }

    pub async fn definitions_for_project(&self, project_id: &str) -> Vec<HookDefinitionView> {
        let owner_namespace = format!("project:{project_id}");
        let definitions = self.definitions.read().await;
        let bindings = self.project_bindings.read().await;
        let active = bindings.get(project_id);
        let mut views = definitions
            .values()
            .filter(|record| {
                record.source == HookSource::Plugin
                    || (record.source == HookSource::Project
                        && record.owner_namespace == owner_namespace)
            })
            .map(|record| {
                let id = record.id();
                definition_view(
                    record,
                    active.is_some_and(|bindings| {
                        bindings.iter().any(|binding| {
                            binding
                                .definition_revision
                                .strip_prefix(&id)
                                .is_some_and(|suffix| suffix.starts_with('@'))
                        })
                    }),
                )
            })
            .collect::<Vec<_>>();
        views.sort_by(|left, right| left.id.cmp(&right.id));
        views
    }

    pub async fn definitions_for_plugin(&self, plugin_id: &str) -> Vec<HookDefinitionView> {
        let owner_namespace = format!("plugin:{plugin_id}");
        let definitions = self.definitions.read().await;
        let mut views = definitions
            .values()
            .filter(|record| {
                record.source == HookSource::Plugin && record.owner_namespace == owner_namespace
            })
            .map(|record| definition_view(record, false))
            .collect::<Vec<_>>();
        views.sort_by(|left, right| left.id.cmp(&right.id));
        views
    }

    pub async fn register_plugin_definitions(
        &self,
        plugin_id: &str,
        inputs: Vec<CreatePluginHookDefinition>,
    ) -> Result<Vec<HookDefinitionView>, String> {
        let owner_namespace = format!("plugin:{plugin_id}");
        let mut logical_ids = HashSet::new();
        for input in &inputs {
            validate_logical_id(&input.logical_id)?;
            if !logical_ids.insert(input.logical_id.as_str()) {
                return Err(format!(
                    "duplicate Plugin Hook logical ID '{}'",
                    input.logical_id
                ));
            }
            validate_revision(&input.event, &input.capabilities, &input.handler)?;
            if input.handler.kind != "plugin" {
                return Err("Plugin Hook declarations support only plugin Handlers".to_string());
            }
        }
        let mut definitions = self.definitions.write().await;
        for input in inputs {
            let id = format!("{owner_namespace}/{}", input.logical_id);
            match definitions.get_mut(&id) {
                Some(record) => {
                    if record.source != HookSource::Plugin {
                        return Err(format!("Hook Definition identity is already owned: {id}"));
                    }
                    record.display_name = input.display_name;
                    let latest = record.latest();
                    if latest.event != input.event
                        || latest.capabilities != input.capabilities
                        || latest.handler != input.handler
                    {
                        record.revisions.push(HookRevisionView {
                            revision: format!("v{}", record.revisions.len() + 1),
                            event: input.event,
                            capabilities: input.capabilities,
                            handler: input.handler,
                        });
                    }
                    record.available = input.available;
                    record.diagnostics = availability_diagnostics(record.available);
                }
                None => {
                    let revision = HookRevisionView {
                        revision: "v1".to_string(),
                        event: input.event,
                        capabilities: input.capabilities,
                        handler: input.handler,
                    };
                    let record = HookDefinitionRecord {
                        owner_namespace: owner_namespace.clone(),
                        logical_id: input.logical_id,
                        display_name: input.display_name,
                        source: HookSource::Plugin,
                        available: input.available,
                        diagnostics: availability_diagnostics(input.available),
                        revisions: vec![revision],
                        revision_changed: false,
                    };
                    definitions.insert(record.id(), record);
                }
            }
        }
        drop(definitions);
        Ok(self.definitions_for_plugin(plugin_id).await)
    }

    pub async fn register_project_definition(
        &self,
        project_id: &str,
        input: CreateProjectHookDefinition,
    ) -> Result<HookDefinitionView, String> {
        self.register_project_definitions(project_id, vec![input])
            .await
            .map(|mut definitions| definitions.remove(0))
    }

    pub async fn register_project_definitions(
        &self,
        project_id: &str,
        inputs: Vec<CreateProjectHookDefinition>,
    ) -> Result<Vec<HookDefinitionView>, String> {
        let owner_namespace = format!("project:{project_id}");
        let mut logical_ids = HashSet::new();
        for input in &inputs {
            validate_logical_id(&input.logical_id)?;
            if !logical_ids.insert(input.logical_id.as_str()) {
                return Err(format!(
                    "duplicate Project Hook logical ID '{}'",
                    input.logical_id
                ));
            }
            validate_revision(&input.event, &input.capabilities, &input.handler)?;
            if input.handler.kind != "command" {
                return Err("Project Hook manifests support only command Handlers".to_string());
            }
        }
        let mut definitions = self.definitions.write().await;
        for input in &inputs {
            let id = format!("{owner_namespace}/{}", input.logical_id);
            if definitions
                .get(&id)
                .is_some_and(|record| record.source != HookSource::Project)
            {
                return Err(format!("Hook Definition identity is already owned: {id}"));
            }
        }
        let mut changed_ids = Vec::new();
        for input in inputs {
            let id = format!("{owner_namespace}/{}", input.logical_id);
            match definitions.get_mut(&id) {
                Some(record) => {
                    record.display_name = input.display_name;
                    let latest = record.latest();
                    let material_change = latest.event != input.event
                        || latest.capabilities != input.capabilities
                        || latest.handler != input.handler;
                    if material_change {
                        record.revisions.push(HookRevisionView {
                            revision: format!("v{}", record.revisions.len() + 1),
                            event: input.event,
                            capabilities: input.capabilities,
                            handler: input.handler,
                        });
                        record.revision_changed = true;
                        changed_ids.push(id);
                    }
                    record.available = record.latest().handler.available;
                    record.diagnostics = availability_diagnostics(record.available);
                }
                None => {
                    let revision = HookRevisionView {
                        revision: "v1".to_string(),
                        event: input.event,
                        capabilities: input.capabilities,
                        handler: input.handler,
                    };
                    let record = HookDefinitionRecord {
                        owner_namespace: owner_namespace.clone(),
                        logical_id: input.logical_id,
                        display_name: input.display_name,
                        source: HookSource::Project,
                        available: revision.handler.available,
                        diagnostics: availability_diagnostics(revision.handler.available),
                        revisions: vec![revision],
                        revision_changed: false,
                    };
                    definitions.insert(record.id(), record);
                }
            }
        }
        drop(definitions);
        if !changed_ids.is_empty() {
            self.project_bindings
                .write()
                .await
                .values_mut()
                .for_each(|bindings| {
                    bindings.retain(|binding| {
                        !changed_ids.iter().any(|id| {
                            binding
                                .definition_revision
                                .strip_prefix(id)
                                .is_some_and(|suffix| suffix.starts_with('@'))
                        })
                    });
                });
        }
        Ok(self.definitions_for_project(project_id).await)
    }

    pub async fn create_user_definition(
        &self,
        agent_id: &str,
        input: CreateUserHookDefinition,
    ) -> Result<HookDefinitionView, String> {
        if input.owner_namespace.is_some() || input.source.is_some() {
            return Err("owner_namespace and source are assigned by Rua".to_string());
        }
        validate_logical_id(&input.logical_id)?;
        validate_revision(&input.event, &input.capabilities, &input.handler)?;
        if input.handler.kind == "native" {
            return Err("native Hook Definitions are product-owned".to_string());
        }
        let owner_namespace = format!("agent:{agent_id}");
        let id = format!("{owner_namespace}/{}", input.logical_id);
        let available = input.handler.available;
        let mut definitions = self.definitions.write().await;
        if definitions.contains_key(&id) {
            return Err(format!("Hook Definition already exists: {id}"));
        }
        let record = HookDefinitionRecord {
            owner_namespace,
            logical_id: input.logical_id,
            display_name: input.display_name,
            source: HookSource::User,
            revisions: vec![HookRevisionView {
                revision: "v1".to_string(),
                event: input.event,
                capabilities: input.capabilities,
                handler: input.handler,
            }],
            available,
            revision_changed: false,
            diagnostics: Vec::new(),
        };
        let view = definition_view(&record, false);
        definitions.insert(id, record);
        Ok(view)
    }

    pub async fn definition_detail(
        &self,
        agent_id: &str,
        owner_namespace: &str,
        logical_id: &str,
    ) -> Option<HookDefinitionDetailView> {
        let id = format!("{owner_namespace}/{logical_id}");
        let definitions = self.definitions.read().await;
        let record = definitions.get(&id)?;
        if !definition_visible_to_agent(record, agent_id) {
            return None;
        }
        let active = self
            .agent_activations
            .read()
            .await
            .get(agent_id)
            .is_some_and(|ids| activation_selects_definition(ids, &id));
        Some(HookDefinitionDetailView {
            definition: definition_view(record, active),
            revisions: record.revisions.clone(),
        })
    }

    pub async fn revise_user_definition(
        &self,
        agent_id: &str,
        owner_namespace: &str,
        logical_id: &str,
        input: ReviseUserHookDefinition,
    ) -> Result<HookDefinitionView, String> {
        let id = format!("{owner_namespace}/{logical_id}");
        let mut definitions = self.definitions.write().await;
        let record = definitions
            .get_mut(&id)
            .ok_or_else(|| format!("Hook Definition not found: {id}"))?;
        if record.source != HookSource::User
            || record.owner_namespace != format!("agent:{agent_id}")
        {
            return Err("only user-owned Agent Definitions can be edited".to_string());
        }
        if let Some(display_name) = input.display_name {
            record.display_name = display_name;
        }
        let executable_change = input.event.is_some()
            || input.capabilities.is_some()
            || input.program.is_some()
            || input.args.is_some()
            || input.working_directory.is_some()
            || input.timeout_seconds.is_some()
            || input.available.is_some();
        if executable_change {
            let current = record.latest().clone();
            let mut handler = current.handler;
            if let Some(program) = input.program {
                handler.program = Some(program);
            }
            if let Some(args) = input.args {
                handler.args = args;
            }
            if let Some(working_directory) = input.working_directory {
                handler.working_directory = Some(working_directory);
            }
            if let Some(timeout_seconds) = input.timeout_seconds {
                handler.timeout_seconds = Some(timeout_seconds);
            }
            if let Some(available) = input.available {
                handler.available = available;
            }
            let next = HookRevisionView {
                revision: format!("v{}", record.revisions.len() + 1),
                event: input.event.unwrap_or(current.event),
                capabilities: input.capabilities.unwrap_or(current.capabilities),
                handler,
            };
            validate_revision(&next.event, &next.capabilities, &next.handler)?;
            record.available = next.handler.available;
            record.diagnostics = availability_diagnostics(record.available);
            record.revisions.push(next);
            self.agent_activations
                .write()
                .await
                .values_mut()
                .for_each(|ids| {
                    ids.retain(|selected| {
                        !selected
                            .strip_prefix(&id)
                            .is_some_and(|suffix| suffix.starts_with('@'))
                    });
                });
        }
        let active = self
            .agent_activations
            .read()
            .await
            .get(agent_id)
            .is_some_and(|ids| activation_selects_definition(ids, &id));
        Ok(definition_view(record, active))
    }

    pub async fn set_agent_activation(
        &self,
        agent_id: &str,
        definition_id: &str,
        enabled: bool,
    ) -> Result<(), String> {
        let definitions = self.definitions.read().await;
        let record = definitions
            .get(definition_id)
            .ok_or_else(|| format!("Hook Definition not found: {definition_id}"))?;
        if !definition_visible_to_agent(record, agent_id) {
            return Err(format!("Hook Definition not found: {definition_id}"));
        }
        let definition_revision = format!("{definition_id}@{}", record.latest().revision);
        drop(definitions);
        let mut activations = self.agent_activations.write().await;
        let selected = activations.entry(agent_id.to_string()).or_default();
        selected.retain(|selected| {
            !selected
                .strip_prefix(definition_id)
                .is_some_and(|suffix| suffix.starts_with('@'))
        });
        if enabled {
            selected.insert(definition_revision);
        }
        Ok(())
    }

    pub async fn set_project_activation(
        &self,
        project_id: &str,
        definition_id: &str,
        enabled: bool,
    ) -> Result<(), String> {
        let definitions = self.definitions.read().await;
        let record = definitions
            .get(definition_id)
            .ok_or_else(|| format!("Hook Definition not found: {definition_id}"))?;
        if record.source != HookSource::Plugin
            && !(record.source == HookSource::Project
                && record.owner_namespace == format!("project:{project_id}"))
        {
            return Err(format!("Hook Definition not found: {definition_id}"));
        }
        let revision = record.latest();
        let definition_revision = format!("{definition_id}@{}", revision.revision);
        let event = revision.event.clone();
        drop(definitions);

        let mut bindings = self.project_bindings.write().await;
        let selected = bindings.entry(project_id.to_string()).or_default();
        selected.retain(|binding| {
            !binding
                .definition_revision
                .strip_prefix(definition_id)
                .is_some_and(|suffix| suffix.starts_with('@'))
        });
        if enabled {
            selected.push(HookBinding {
                definition_revision,
                event,
                source: SelectionSource::Project,
                configured_order: selected.len() as u32,
                parameters: Default::default(),
                before: Vec::new(),
                after: Vec::new(),
                requires: Vec::new(),
            });
        }
        Ok(())
    }

    pub async fn mark_plugin_unavailable(&self, plugin_id: &str, diagnostic: &str) {
        let owner_namespace = format!("plugin:{plugin_id}");
        for record in self.definitions.write().await.values_mut() {
            if record.source == HookSource::Plugin && record.owner_namespace == owner_namespace {
                record.available = false;
                record.diagnostics = vec![diagnostic.to_string()];
            }
        }
    }

    pub async fn resolve_chat_configuration(
        &self,
        address: &ChatAddress,
        agent_id: &str,
    ) -> Result<ChatHookConfiguration, String> {
        self.resolve_chat_configuration_inner(
            address,
            ChatConfigurationMaterialization {
                selected_agent_id: agent_id.to_string(),
                running_config: rua_config::ChatRunningConfig::default(),
                security_config: Default::default(),
                agent_assignment: None,
                prompt_file_content: None,
                tool_config_snapshot: Default::default(),
                capability_configuration: None,
                model: None,
                thinking: None,
                collaboration_mode: serde_json::Value::Null,
                harness_configuration: None,
                authority_device_id: String::new(),
            },
            false,
        )
        .await
    }

    pub async fn resolve_complete_chat_configuration(
        &self,
        address: &ChatAddress,
        materialization: ChatConfigurationMaterialization,
    ) -> Result<ChatHookConfiguration, String> {
        if materialization.authority_device_id.trim().is_empty() {
            return Err("Chat configuration requires an explicit Authority Device".to_string());
        }
        materialization.running_config.validate()?;
        self.resolve_chat_configuration_inner(address, materialization, true)
            .await
    }

    async fn resolve_chat_configuration_inner(
        &self,
        address: &ChatAddress,
        materialization: ChatConfigurationMaterialization,
        require_explicit_execution_context: bool,
    ) -> Result<ChatHookConfiguration, String> {
        let definitions = self.definitions.read().await;
        let activations = self.agent_activations.read().await;
        let agent = activations
            .get(&materialization.selected_agent_id)
            .into_iter()
            .flatten()
            .filter_map(|definition_revision| {
                let (definition_id, revision_id) = definition_revision.rsplit_once('@')?;
                let record = definitions.get(definition_id)?;
                let revision = record
                    .revisions
                    .iter()
                    .find(|revision| revision.revision == revision_id)?;
                Some(HookBinding {
                    definition_revision: definition_revision.clone(),
                    event: revision.event.clone(),
                    source: SelectionSource::Agent,
                    configured_order: 0,
                    parameters: Default::default(),
                    before: Vec::new(),
                    after: Vec::new(),
                    requires: Vec::new(),
                })
            })
            .collect::<Vec<_>>();
        drop(activations);
        drop(definitions);
        let project = match address.project_id() {
            Some(project_id) => self
                .project_bindings
                .read()
                .await
                .get(project_id.as_str())
                .cloned()
                .unwrap_or_default(),
            None => Vec::new(),
        };
        let chat = self
            .chat_bindings
            .read()
            .await
            .get(address)
            .cloned()
            .unwrap_or_default();
        let preview = crate::services::scope_composition::compose(CompositionInput {
            agent,
            project,
            chat,
        })?;
        let definitions = self.definitions.read().await;
        let mut execution_requirements = Vec::new();
        for binding in &preview.hooks {
            let Some((definition_id, revision_id)) = binding.definition_revision.rsplit_once('@')
            else {
                continue;
            };
            let Some(record) = definitions.get(definition_id) else {
                continue;
            };
            let Some(revision) = record
                .revisions
                .iter()
                .find(|revision| revision.revision == revision_id)
            else {
                continue;
            };
            if require_explicit_execution_context
                && revision.handler.kind == "command"
                && revision.handler.working_directory.is_none()
            {
                return Err(format!(
                    "Hook '{}' requires an explicit working directory",
                    binding.definition_revision
                ));
            }
            execution_requirements.push(HookExecutionRequirement {
                definition_revision: binding.definition_revision.clone(),
                authority_device_id: materialization.authority_device_id.clone(),
                handler_kind: revision.handler.kind.clone(),
                capabilities: revision.capabilities.clone(),
                executable: revision.handler.executable.clone(),
                program: revision.handler.program.clone(),
                args: revision.handler.args.clone(),
                working_directory: revision.handler.working_directory.clone(),
                timeout_seconds: revision.handler.timeout_seconds,
                required_resources: revision.handler.required_resources.clone(),
                available: record.available,
            });
        }
        let definition_revisions = preview
            .hooks
            .iter()
            .map(|binding| binding.definition_revision.clone())
            .collect();
        let snapshot = ChatHookConfiguration {
            epoch: format!("hook-config-{}", uuid::Uuid::new_v4()),
            running_config: materialization.running_config,
            security_config: materialization.security_config,
            extra: serde_json::Map::new(),
            agent_assignment: materialization.agent_assignment,
            prompt_file_content: materialization.prompt_file_content,
            tool_config_snapshot: materialization.tool_config_snapshot,
            capability_configuration: materialization.capability_configuration,
            selected_agent_id: materialization.selected_agent_id,
            model: materialization.model,
            thinking: materialization.thinking,
            collaboration_mode: materialization.collaboration_mode,
            harness_configuration: materialization.harness_configuration,
            authority_device_id: (!materialization.authority_device_id.is_empty())
                .then_some(materialization.authority_device_id),
            definition_revisions,
            effective_hooks: preview.hooks,
            warnings: preview.warnings,
            execution_requirements,
        };
        snapshot.validate()?;
        Ok(snapshot)
    }

    pub async fn set_project_bindings(&self, project_id: &str, bindings: Vec<HookBinding>) {
        {
            let mut definitions = self.definitions.write().await;
            for binding in &bindings {
                let Some((definition_id, revision)) = binding.definition_revision.rsplit_once('@')
                else {
                    continue;
                };
                if let Some(record) = definitions.get_mut(definition_id) {
                    if record.source == HookSource::Project
                        && record.owner_namespace == format!("project:{project_id}")
                        && record.latest().revision == revision
                    {
                        record.revision_changed = false;
                    }
                }
            }
        }
        self.project_bindings
            .write()
            .await
            .insert(project_id.to_string(), bindings);
    }

    pub async fn set_chat_bindings(&self, address: &ChatAddress, bindings: Vec<HookBinding>) {
        self.chat_bindings
            .write()
            .await
            .insert(address.clone(), bindings);
    }

    pub async fn chat_configuration(&self, address: &ChatAddress) -> Option<ChatHookConfiguration> {
        self.chat_configurations.read().await.get(address).cloned()
    }

    pub async fn store_configuration_preview(
        &self,
        address: &ChatAddress,
        preview: ChatConfigurationPreview,
    ) {
        self.configuration_previews
            .write()
            .await
            .insert(address.clone(), preview);
    }

    pub async fn configuration_preview(
        &self,
        address: &ChatAddress,
        epoch: &str,
    ) -> Option<ChatConfigurationPreview> {
        self.configuration_previews
            .read()
            .await
            .get(address)
            .filter(|preview| preview.candidate.epoch == epoch)
            .cloned()
    }

    pub async fn adopt_chat_configuration(
        &self,
        address: &ChatAddress,
        configuration: ChatHookConfiguration,
    ) {
        let mut configurations = self.chat_configurations.write().await;
        if configurations.get(address) == Some(&configuration) {
            return;
        }
        configurations.insert(address.clone(), configuration);
        drop(configurations);
        self.configuration_previews.write().await.remove(address);
    }

    pub async fn freeze_turn(&self, address: &ChatAddress, turn_id: &str) {
        let snapshot = self.chat_configurations.read().await.get(address).cloned();
        if let Some(snapshot) = snapshot {
            self.frozen_turns
                .write()
                .await
                .insert((address.clone(), turn_id.to_string()), snapshot);
        }
    }

    pub async fn frozen_turn_configuration(
        &self,
        address: &ChatAddress,
        turn_id: &str,
    ) -> Option<ChatHookConfiguration> {
        self.frozen_turns
            .read()
            .await
            .get(&(address.clone(), turn_id.to_string()))
            .cloned()
    }

    pub async fn observe_terminal(
        &self,
        address: &ChatAddress,
        turn_id: &str,
        completed_at: Option<&str>,
    ) {
        let frozen = self
            .frozen_turns
            .write()
            .await
            .remove(&(address.clone(), turn_id.to_string()));
        let Some(frozen) = frozen else { return };
        if !frozen
            .definition_revisions
            .iter()
            .any(|revision| revision == &format!("{NATIVE_TERMINAL_DEFINITION_ID}@v1"))
        {
            return;
        }
        let now = chrono::Utc::now().to_rfc3339();
        self.runs.write().await.push(HookRunView {
            invocation_id: format!("hook-run-{}", uuid::Uuid::new_v4()),
            agent_id: address
                .agent_id()
                .map(|agent_id| agent_id.as_str())
                .unwrap_or_default()
                .to_string(),
            chat_id: address.chat_id().as_str().to_string(),
            turn_id: turn_id.to_string(),
            event: "turn.terminal".to_string(),
            definition_revision: format!("{NATIVE_TERMINAL_DEFINITION_ID}@v1"),
            source: "native".to_string(),
            resolved_order: 0,
            status: HookRunStatus::Completed,
            decision_summary: Some("terminal observed".to_string()),
            diagnostics: Vec::new(),
            started_at: now.clone(),
            completed_at: completed_at.unwrap_or(&now).to_string(),
        });
    }

    pub async fn record_terminal_observer_diagnostics(
        &self,
        terminal: &crate::services::conversation_authority::ConversationTurnTerminalFact,
        diagnostics: &[crate::services::terminal_observer::TerminalObserverDiagnostic],
    ) {
        let now = chrono::Utc::now().to_rfc3339();
        let mut runs = self.runs.write().await;
        for item in diagnostics {
            runs.push(HookRunView {
                invocation_id: format!("hook-run-{}", uuid::Uuid::new_v4()),
                agent_id: terminal
                    .address
                    .agent_id()
                    .map(|agent_id| agent_id.as_str())
                    .unwrap_or_default()
                    .to_string(),
                chat_id: terminal.address.chat_id().as_str().to_string(),
                turn_id: terminal.turn_id.clone(),
                event: "turn.terminal".to_string(),
                definition_revision: item.definition_revision.clone(),
                source: "terminal_observer".to_string(),
                resolved_order: item.resolved_order,
                status: match item.status.as_str() {
                    "completed" => HookRunStatus::Completed,
                    "cancelled" => HookRunStatus::Cancelled,
                    _ => HookRunStatus::Failed,
                },
                decision_summary: Some(item.status.clone()),
                diagnostics: item.diagnostic.iter().cloned().collect(),
                started_at: now.clone(),
                completed_at: terminal.completed_at.clone().unwrap_or_else(|| now.clone()),
            });
        }
    }

    pub async fn runs(&self, agent_id: &str, chat_id: Option<&str>) -> Vec<HookRunView> {
        self.runs
            .read()
            .await
            .iter()
            .filter(|run| run.agent_id == agent_id)
            .filter(|run| chat_id.is_none_or(|chat_id| run.chat_id == chat_id))
            .cloned()
            .collect()
    }

    pub async fn record_contribution(&self, contribution: ProductHookContributionView) {
        self.contributions.write().await.push(contribution);
    }

    pub async fn contributions(
        &self,
        address: &ChatAddress,
        turn_id: &str,
    ) -> Vec<ProductHookContributionView> {
        self.contributions
            .read()
            .await
            .iter()
            .filter(|item| item.chat_address == *address && item.turn_id == turn_id)
            .cloned()
            .collect()
    }
}

fn definition_visible_to_agent(record: &HookDefinitionRecord, agent_id: &str) -> bool {
    matches!(record.source, HookSource::Native | HookSource::Plugin)
        || record.owner_namespace == format!("agent:{agent_id}")
}

fn activation_selects_definition(selected: &HashSet<String>, definition_id: &str) -> bool {
    selected.iter().any(|selection| {
        selection
            .strip_prefix(definition_id)
            .is_some_and(|suffix| suffix.starts_with('@'))
    })
}

fn definition_view(record: &HookDefinitionRecord, active: bool) -> HookDefinitionView {
    let latest = record.latest();
    HookDefinitionView {
        id: record.id(),
        owner_namespace: record.owner_namespace.clone(),
        logical_id: record.logical_id.clone(),
        display_name: record.display_name.clone(),
        source: record.source.clone(),
        latest_revision: latest.revision.clone(),
        revision: latest.revision.clone(),
        event: latest.event.clone(),
        capabilities: latest.capabilities.clone(),
        handler_kind: latest.handler.kind.clone(),
        registered: true,
        availability: HookStateView {
            status: if record.available {
                "available".to_string()
            } else {
                "unavailable".to_string()
            },
            diagnostics: record.diagnostics.clone(),
        },
        validation: HookStateView {
            status: "valid".to_string(),
            diagnostics: Vec::new(),
        },
        active,
        disabled: !active,
        revision_changed: record.revision_changed,
        diagnostics: record.diagnostics.clone(),
    }
}

fn availability_diagnostics(available: bool) -> Vec<String> {
    if available {
        Vec::new()
    } else {
        vec!["Handler implementation is unavailable".to_string()]
    }
}

fn validate_logical_id(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.contains('/')
        || !value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return Err("logical_id must contain only letters, numbers, '-' or '_'".to_string());
    }
    Ok(())
}

fn validate_revision(
    event: &str,
    capabilities: &[String],
    handler: &HookHandlerView,
) -> Result<(), String> {
    let allowed: &[&str] = match event {
        "turn.before_completion" | "turn.terminal" => &["identity", "candidate_response"],
        "tool.before" => &["identity", "tool_arguments"],
        "tool.after" => &["identity", "tool_result", "context"],
        "tool.failed" => &["identity", "redacted_diagnostics", "context"],
        "permission.request" => &["identity", "tool_arguments", "permission_request"],
        _ => return Err(format!("unsupported Hook event: {event}")),
    };
    if let Some(capability) = capabilities
        .iter()
        .find(|capability| !allowed.contains(&capability.as_str()))
    {
        return Err(format!(
            "capability '{capability}' is not allowed for event '{event}'"
        ));
    }
    if !matches!(handler.kind.as_str(), "command" | "plugin") {
        return Err(format!("unsupported user Handler kind: {}", handler.kind));
    }
    match handler.kind.as_str() {
        "command" => {
            if !matches!(handler.program.as_deref(), Some(program) if !program.is_empty()) {
                return Err("command Handler program is required".to_string());
            }
            if !matches!(
                handler.working_directory.as_deref(),
                Some(working_directory) if !working_directory.is_empty()
            ) {
                return Err("command Handler working_directory is required".to_string());
            }
            if handler.timeout_seconds == Some(0) {
                return Err("command Handler timeout_seconds must be greater than zero".to_string());
            }
            if handler.executable.is_some() {
                return Err("command Handler uses program and args, not executable".to_string());
            }
        }
        "plugin" => {
            if !matches!(
                handler.executable.as_deref(),
                Some(executable) if !executable.is_empty()
            ) {
                return Err("plugin Handler executable is required".to_string());
            }
        }
        _ => unreachable!("unsupported kind returned above"),
    }
    Ok(())
}
