use agent_types::{ToolCall, ToolDefinition, ToolResult};
use serde::{Deserialize, Serialize};

use crate::toolkit::{AgentToolkit, DisclosureClass, ToolSourceKind};

pub const TOOL_SEARCH_NAME: &str = "tool_search";
pub const TOOL_DESCRIBE_NAME: &str = "tool_describe";
pub const TOOL_CALL_NAME: &str = "tool_call";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolDisclosureMode {
    Auto,
    On,
    Off,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDisclosureConfig {
    #[serde(default = "default_disclosure_mode")]
    pub enabled: ToolDisclosureMode,
    #[serde(default = "default_threshold_pct")]
    pub threshold_pct: f32,
    #[serde(default = "default_search_default_limit")]
    pub search_default_limit: usize,
    #[serde(default = "default_search_max_limit")]
    pub search_max_limit: usize,
}

impl Default for ToolDisclosureConfig {
    fn default() -> Self {
        Self {
            enabled: default_disclosure_mode(),
            threshold_pct: default_threshold_pct(),
            search_default_limit: default_search_default_limit(),
            search_max_limit: default_search_max_limit(),
        }
    }
}

fn default_disclosure_mode() -> ToolDisclosureMode {
    ToolDisclosureMode::Auto
}

fn default_threshold_pct() -> f32 {
    10.0
}

fn default_search_default_limit() -> usize {
    5
}

fn default_search_max_limit() -> usize {
    20
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolDisclosureAssembly {
    pub tools: Vec<ToolDefinition>,
    pub activated: bool,
    pub direct_tool_names: Vec<String>,
    pub deferred_tool_names: Vec<String>,
    pub deferred_schema_tokens: usize,
    pub threshold_tokens: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DeferredToolSearchCandidate {
    pub name: String,
    pub description: String,
    pub source_kind: ToolSourceKind,
    pub source_name: Option<String>,
    pub category: Option<String>,
    pub parameters: Option<serde_json::Value>,
}

#[derive(Debug, Clone)]
struct DeferredToolCatalogEntry {
    definition: ToolDefinition,
    source_kind: ToolSourceKind,
    source_name: Option<String>,
    category: Option<String>,
    index_name: String,
    index_description: String,
    index_parameters: String,
}

#[derive(Debug, Clone)]
pub struct DeferredToolCatalog {
    entries: Vec<DeferredToolCatalogEntry>,
    default_limit: usize,
    max_limit: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum BridgeToolCallResolution {
    NotBridge,
    Unwrapped(ToolCall),
    Rejected(ToolResult),
}

#[derive(Debug, Clone, PartialEq)]
pub enum BridgeReadResolution {
    NotBridge,
    Resolved(Box<ToolResult>),
}

impl DeferredToolCatalog {
    pub fn from_toolkit(toolkit: &AgentToolkit, config: &ToolDisclosureConfig) -> Self {
        let entries = toolkit
            .disclosure_inventory()
            .into_iter()
            .filter(|item| item.disclosure.class == DisclosureClass::DeferredCandidate)
            .map(|item| {
                let top_level_parameters = top_level_parameter_names(&item.definition.parameters);
                DeferredToolCatalogEntry {
                    index_name: item.definition.name.to_lowercase(),
                    index_description: item.definition.description.to_lowercase(),
                    index_parameters: top_level_parameters.join(" ").to_lowercase(),
                    definition: item.definition,
                    source_kind: item.disclosure.source_kind,
                    source_name: item.disclosure.source_name,
                    category: item.disclosure.category,
                }
            })
            .collect();

        let max_limit = config.search_max_limit.max(1);
        let default_limit = config.search_default_limit.clamp(1, max_limit);

        Self {
            entries,
            default_limit,
            max_limit,
        }
    }

    pub fn search(&self, query: &str, limit: Option<usize>) -> Vec<DeferredToolSearchCandidate> {
        let query_terms = tokenize_query(query);
        let limit = limit.unwrap_or(self.default_limit).clamp(1, self.max_limit);
        let mut scored: Vec<(usize, &DeferredToolCatalogEntry)> = self
            .entries
            .iter()
            .map(|entry| (score_entry(entry, &query_terms), entry))
            .filter(|(score, _)| query_terms.is_empty() || *score > 0)
            .collect();

        scored.sort_by(|(left_score, left), (right_score, right)| {
            right_score
                .cmp(left_score)
                .then_with(|| left.definition.name.cmp(&right.definition.name))
        });

        scored
            .into_iter()
            .take(limit)
            .map(|(_, entry)| DeferredToolSearchCandidate {
                name: entry.definition.name.clone(),
                description: entry.definition.description.clone(),
                source_kind: entry.source_kind.clone(),
                source_name: entry.source_name.clone(),
                category: entry.category.clone(),
                parameters: None,
            })
            .collect()
    }

    pub fn describe(&self, name: &str) -> Option<ToolDefinition> {
        self.entries
            .iter()
            .find(|entry| entry.definition.name == name)
            .or_else(|| {
                self.entries
                    .iter()
                    .find(|entry| entry.definition.name == name)
            })
            .map(|entry| entry.definition.clone())
    }
}

pub fn resolve_bridge_tool_call(
    call: &ToolCall,
    toolkit: &AgentToolkit,
) -> BridgeToolCallResolution {
    if call.name != TOOL_CALL_NAME {
        return BridgeToolCallResolution::NotBridge;
    }

    let Some(target_name) = call.arguments.get("name").and_then(|value| value.as_str()) else {
        return BridgeToolCallResolution::Rejected(ToolResult::error(
            call.id.clone(),
            "tool_call requires a deferred tool name. Use tool_search to find tools and tool_describe to inspect parameters.",
        ));
    };

    if is_bridge_tool_name(target_name) {
        return BridgeToolCallResolution::Rejected(scope_error_result(&call.id, target_name));
    }

    let Some(disclosure) = toolkit.disclosure_for(target_name) else {
        return BridgeToolCallResolution::Rejected(scope_error_result(&call.id, target_name));
    };
    if disclosure.class != DisclosureClass::DeferredCandidate {
        return BridgeToolCallResolution::Rejected(scope_error_result(&call.id, target_name));
    }

    let Some(arguments) = call.arguments.get("arguments") else {
        return BridgeToolCallResolution::Rejected(arguments_error_result(&call.id, target_name));
    };
    if !arguments.is_object() {
        return BridgeToolCallResolution::Rejected(arguments_error_result(&call.id, target_name));
    }

    let mut unwrapped = call.clone();
    unwrapped.name = toolkit
        .canonical_name(target_name)
        .unwrap_or(target_name)
        .to_string();
    unwrapped.arguments = arguments.clone();
    unwrapped.raw_arguments = None;
    unwrapped
        .metadata
        .insert("bridge_tool".to_string(), serde_json::json!(TOOL_CALL_NAME));
    BridgeToolCallResolution::Unwrapped(unwrapped)
}

pub fn resolve_bridge_read_call(
    call: &ToolCall,
    toolkit: &AgentToolkit,
    config: &ToolDisclosureConfig,
) -> BridgeReadResolution {
    if call.name == TOOL_SEARCH_NAME {
        let Some(query) = call.arguments.get("query").and_then(|value| value.as_str()) else {
            return BridgeReadResolution::Resolved(Box::new(ToolResult::error(
                call.id.clone(),
                "tool_search requires a string query.",
            )));
        };
        let limit = call
            .arguments
            .get("limit")
            .and_then(|value| value.as_u64())
            .map(|value| value as usize);
        let catalog = DeferredToolCatalog::from_toolkit(toolkit, config);
        let candidates = catalog.search(query, limit);
        BridgeReadResolution::Resolved(Box::new(json_tool_result(
            &call.id,
            TOOL_SEARCH_NAME,
            &candidates,
        )))
    } else if call.name == TOOL_DESCRIBE_NAME {
        let Some(name) = call.arguments.get("name").and_then(|value| value.as_str()) else {
            return BridgeReadResolution::Resolved(Box::new(ToolResult::error(
                call.id.clone(),
                "tool_describe requires a deferred tool name.",
            )));
        };
        let catalog = DeferredToolCatalog::from_toolkit(toolkit, config);
        match catalog.describe(name) {
            Some(definition) => BridgeReadResolution::Resolved(Box::new(json_tool_result(
                &call.id,
                TOOL_DESCRIBE_NAME,
                &definition,
            ))),
            None => BridgeReadResolution::Resolved(Box::new(ToolResult::error(
                call.id.clone(),
                format!("tool_describe can only describe deferred tools in the current session catalog; '{name}' is not available."),
            ))),
        }
    } else {
        BridgeReadResolution::NotBridge
    }
}

pub fn assemble_tool_definitions(
    toolkit: &AgentToolkit,
    config: &ToolDisclosureConfig,
    context_window_tokens: Option<usize>,
) -> ToolDisclosureAssembly {
    let inventory = toolkit.disclosure_inventory();
    let mut direct_tools = Vec::new();
    let mut deferred_tools = Vec::new();
    let mut direct_tool_names = Vec::new();
    let mut deferred_tool_names = Vec::new();
    let mut deferred_schema_tokens = 0usize;

    for item in inventory {
        match item.disclosure.class {
            DisclosureClass::Direct => {
                direct_tool_names.push(item.definition.name.clone());
                direct_tools.push(item.definition);
            }
            DisclosureClass::DeferredCandidate => {
                deferred_schema_tokens += item
                    .disclosure
                    .schema_tokens
                    .unwrap_or_else(|| estimate_tool_schema_tokens(&item.definition));
                deferred_tool_names.push(item.definition.name.clone());
                deferred_tools.push(item.definition);
            }
            DisclosureClass::Hidden => {}
        }
    }

    let threshold_tokens = context_window_tokens.map(|tokens| {
        let pct = if config.threshold_pct.is_finite() && config.threshold_pct > 0.0 {
            config.threshold_pct
        } else {
            default_threshold_pct()
        };
        ((tokens as f32) * (pct / 100.0)).round() as usize
    });

    let activated = match config.enabled {
        ToolDisclosureMode::Off => false,
        ToolDisclosureMode::On => !deferred_tools.is_empty(),
        ToolDisclosureMode::Auto => {
            !deferred_tools.is_empty()
                && threshold_tokens
                    .map(|threshold| deferred_schema_tokens > threshold)
                    .unwrap_or(false)
        }
    };

    let tools = if activated {
        let mut tools = direct_tools;
        tools.extend(bridge_tool_definitions(config));
        tools
    } else {
        let mut tools = direct_tools;
        tools.extend(deferred_tools);
        tools
    };

    ToolDisclosureAssembly {
        tools,
        activated,
        direct_tool_names,
        deferred_tool_names,
        deferred_schema_tokens,
        threshold_tokens,
    }
}

pub fn estimate_tool_schema_tokens(definition: &ToolDefinition) -> usize {
    let bytes = serde_json::to_string(definition)
        .map(|s| s.len())
        .unwrap_or_else(|_| {
            definition.name.len()
                + definition.description.len()
                + definition.parameters.to_string().len()
        });
    bytes.div_ceil(4).max(1)
}

pub fn bridge_tool_definitions(config: &ToolDisclosureConfig) -> Vec<ToolDefinition> {
    vec![
        tool_search_definition(config),
        tool_describe_definition(),
        tool_call_definition(),
    ]
}

fn top_level_parameter_names(parameters: &serde_json::Value) -> Vec<String> {
    parameters
        .get("properties")
        .and_then(|properties| properties.as_object())
        .map(|properties| properties.keys().cloned().collect())
        .unwrap_or_default()
}

fn tokenize_query(query: &str) -> Vec<String> {
    query
        .to_lowercase()
        .split(|ch: char| !ch.is_alphanumeric() && ch != '_')
        .filter(|term| !term.is_empty())
        .map(|term| term.to_string())
        .collect()
}

fn is_bridge_tool_name(name: &str) -> bool {
    name == TOOL_SEARCH_NAME || name == TOOL_DESCRIBE_NAME || name == TOOL_CALL_NAME
}

fn score_entry(entry: &DeferredToolCatalogEntry, query_terms: &[String]) -> usize {
    if query_terms.is_empty() {
        return 1;
    }

    query_terms.iter().fold(0, |score, term| {
        score
            + if entry.index_name.contains(term) {
                3
            } else {
                0
            }
            + if entry.index_description.contains(term) {
                2
            } else {
                0
            }
            + if entry.index_parameters.contains(term) {
                1
            } else {
                0
            }
    })
}

fn scope_error_result(call_id: &str, target_name: &str) -> ToolResult {
    ToolResult::error(
        call_id.to_string(),
        format!(
            "tool_call can invoke only deferred tools in the current session catalog; '{target_name}' is not an available deferred tool."
        ),
    )
}

fn arguments_error_result(call_id: &str, target_name: &str) -> ToolResult {
    ToolResult::error(
        call_id.to_string(),
        format!(
            "tool_call arguments for deferred tool '{target_name}' must be a JSON object. Use tool_describe first to inspect the required parameters."
        ),
    )
}

fn json_tool_result<T: serde::Serialize>(call_id: &str, name: &str, value: &T) -> ToolResult {
    let content = serde_json::to_string(value).unwrap_or_else(|_| "null".to_string());
    let mut result = ToolResult::text(call_id, content);
    result.name = Some(name.to_string());
    result
}

fn tool_search_definition(config: &ToolDisclosureConfig) -> ToolDefinition {
    let max_limit = config.search_max_limit.max(1);
    let default_limit = config.search_default_limit.clamp(1, max_limit);
    ToolDefinition {
        name: TOOL_SEARCH_NAME.to_string(),
        description: format!(
            "Search deferred tools available in this session. Returns short candidates; default limit {default_limit}, max {max_limit}."
        ),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Natural language description of the needed tool."
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": max_limit,
                    "default": default_limit
                }
            },
            "required": ["query"],
            "additionalProperties": false
        }),
        strict: Some(true),
        annotations: Default::default(),
        metadata: Default::default(),
    }
}

fn tool_describe_definition() -> ToolDefinition {
    ToolDefinition {
        name: TOOL_DESCRIBE_NAME.to_string(),
        description: "Return the full parameters schema for one deferred tool by name.".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "description": "Deferred tool name." }
            },
            "required": ["name"],
            "additionalProperties": false
        }),
        strict: Some(true),
        annotations: Default::default(),
        metadata: Default::default(),
    }
}

fn tool_call_definition() -> ToolDefinition {
    ToolDefinition {
        name: TOOL_CALL_NAME.to_string(),
        description: "Invoke one deferred tool by name with JSON object arguments.".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "description": "Deferred tool name." },
                "arguments": {
                    "type": "object",
                    "description": "Arguments matching the deferred tool schema."
                }
            },
            "required": ["name", "arguments"],
            "additionalProperties": false
        }),
        strict: Some(true),
        annotations: Default::default(),
        metadata: Default::default(),
    }
}
