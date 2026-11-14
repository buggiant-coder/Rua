use std::sync::Arc;

use react_agent::toolkit::{
    ContractToolExecutor, DisclosureClass, ToolDisclosureMeta, ToolExecutor, ToolSourceKind,
};
use react_agent::{ToolDisclosureConfig, ToolDisclosureMode};

const CORE_TOOLS: &[&str] = &["bash", "read", "edit", "grep", "glob"];

const DEFERRED_BUNDLED_TOOLS: &[&str] = &["python", "token_usage"];

pub fn builtin_tool_disclosure(name: &str) -> ToolDisclosureMeta {
    if CORE_TOOLS.contains(&name) {
        return ToolDisclosureMeta {
            class: DisclosureClass::Direct,
            source_kind: ToolSourceKind::Builtin,
            source_name: Some("builtin".to_string()),
            category: Some("core".to_string()),
            schema_tokens: None,
            original_name: None,
            product_capability: None,
        };
    }

    let deferred = name.starts_with("task_") || DEFERRED_BUNDLED_TOOLS.contains(&name);
    ToolDisclosureMeta {
        class: if deferred {
            DisclosureClass::DeferredCandidate
        } else {
            DisclosureClass::Direct
        },
        source_kind: ToolSourceKind::Builtin,
        source_name: Some("builtin".to_string()),
        category: Some("bundled".to_string()),
        schema_tokens: None,
        original_name: None,
        product_capability: None,
    }
}

pub fn mcp_tool_disclosure(source_name: Option<String>) -> ToolDisclosureMeta {
    ToolDisclosureMeta {
        class: DisclosureClass::DeferredCandidate,
        source_kind: ToolSourceKind::Mcp,
        source_name,
        category: Some("external".to_string()),
        schema_tokens: None,
        original_name: None,
        product_capability: None,
    }
}

pub fn plugin_tool_disclosure(plugin_id: &str) -> ToolDisclosureMeta {
    ToolDisclosureMeta {
        class: DisclosureClass::DeferredCandidate,
        source_kind: ToolSourceKind::Plugin,
        source_name: Some(plugin_id.to_string()),
        category: Some("external".to_string()),
        schema_tokens: None,
        original_name: None,
        product_capability: None,
    }
}

pub fn react_tool_disclosure_config(
    running: &rua_config::AgentsRunningConfig,
) -> ToolDisclosureConfig {
    let config = running.tool_disclosure.normalized();
    ToolDisclosureConfig {
        enabled: match config.enabled {
            rua_config::types::ToolDisclosureEnabled::Auto => ToolDisclosureMode::Auto,
            rua_config::types::ToolDisclosureEnabled::On => ToolDisclosureMode::On,
            rua_config::types::ToolDisclosureEnabled::Off => ToolDisclosureMode::Off,
        },
        threshold_pct: config.threshold_pct as f32,
        search_default_limit: config.search_default_limit as usize,
        search_max_limit: config.search_max_limit as usize,
    }
}

pub fn populate_toolkit_from_registry(
    toolkit: &mut react_agent::AgentToolkit,
    registry: &rua_tool::ToolRegistry,
) {
    for (name, definition, tool) in registry.iter_tools() {
        let executor: Arc<dyn ToolExecutor> =
            Arc::new(ContractToolExecutor::new(tool).with_schema(definition.parameters.clone()));
        let mut disclosure =
            if registry.namespace_for(name) == Some(&rua_tool::ToolNamespace::Builtin) {
                builtin_tool_disclosure(name)
            } else {
                ToolDisclosureMeta::default()
            };
        disclosure.original_name = Some(name.to_string());
        if disclosure.source_kind == ToolSourceKind::Builtin {
            disclosure.product_capability =
                rua_tool::registry::builtin_product_capability(name).map(str::to_string);
        }
        toolkit.register_with_disclosure(definition, executor, disclosure);
    }
}

/// Readiness supplements selection; execution must still recheck native permissions.
pub fn phone_tool_unavailability(
    name: &str,
    status: &rua_core::phone_use::PhoneStatus,
) -> Option<&'static str> {
    match name {
        "phone_status" | "phone_list_apps" | "phone_list_cameras" => None,
        "phone_take_photo" if !status.camera_permission => {
            Some("Phone Use requires camera permission")
        }
        "phone_take_photo" => None,
        _ if !status.accessibility => Some("Phone Use requires the Android accessibility service"),
        _ => None,
    }
}
