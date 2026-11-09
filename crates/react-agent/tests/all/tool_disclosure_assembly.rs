use std::sync::Arc;

use agent_types::ToolDefinition;
use react_agent::tool_disclosure::{
    assemble_tool_definitions, DeferredToolCatalog, ToolDisclosureConfig, ToolDisclosureMode,
};
use react_agent::toolkit::{
    AgentToolkit, DisclosureClass, MockToolExecutor, ToolDisclosureMeta, ToolSourceKind,
};

fn tool_definition(name: &str) -> ToolDefinition {
    ToolDefinition {
        name: name.to_string(),
        description: format!("{name} test tool"),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "input": { "type": "string" }
            }
        }),
        strict: None,
        annotations: Default::default(),
        metadata: Default::default(),
    }
}

fn meta(class: DisclosureClass, schema_tokens: usize) -> ToolDisclosureMeta {
    ToolDisclosureMeta {
        class,
        source_kind: ToolSourceKind::Builtin,
        source_name: Some("test".into()),
        category: Some("test".into()),
        schema_tokens: Some(schema_tokens),
    }
}

fn toolkit_with_direct_and_deferred(deferred_schema_tokens: usize) -> AgentToolkit {
    let mut toolkit = AgentToolkit::new();
    toolkit.register_with_disclosure(
        tool_definition("read"),
        Arc::new(MockToolExecutor::new("read ok")),
        meta(DisclosureClass::Direct, 10),
    );
    toolkit.register_with_disclosure(
        tool_definition("task_create"),
        Arc::new(MockToolExecutor::new("task ok")),
        meta(DisclosureClass::DeferredCandidate, deferred_schema_tokens),
    );
    toolkit
}

fn register_deferred_tool(
    toolkit: &mut AgentToolkit,
    name: &str,
    description: &str,
    parameter_names: &[&str],
) {
    let properties = parameter_names
        .iter()
        .map(|name| ((*name).to_string(), serde_json::json!({ "type": "string" })))
        .collect::<serde_json::Map<String, serde_json::Value>>();
    let mut definition = tool_definition(name);
    definition.description = description.to_string();
    definition.parameters = serde_json::json!({
        "type": "object",
        "properties": properties
    });
    toolkit.register_with_disclosure(
        definition,
        Arc::new(MockToolExecutor::new("ok")),
        ToolDisclosureMeta {
            class: DisclosureClass::DeferredCandidate,
            source_kind: ToolSourceKind::Plugin,
            source_name: Some("test-plugin".into()),
            category: Some("external".into()),
            schema_tokens: Some(10),
        },
    );
}

fn tool_names(defs: &[ToolDefinition]) -> Vec<&str> {
    let mut names: Vec<&str> = defs.iter().map(|def| def.name.as_str()).collect();
    names.sort_unstable();
    names
}

#[test]
fn rebuilt_toolkits_preserve_model_tool_schema_order() {
    for (mode, context_window) in [
        (ToolDisclosureMode::Off, 1_000),
        (ToolDisclosureMode::On, 1_000),
        (ToolDisclosureMode::Auto, 1_000),
        (ToolDisclosureMode::Auto, 100),
    ] {
        let config = ToolDisclosureConfig {
            enabled: mode,
            ..ToolDisclosureConfig::default()
        };
        let mut previous_names = None;

        for generation in 0..8 {
            let mut toolkit = AgentToolkit::new();
            for offset in 0..24 {
                let index = (offset + generation) % 24;
                let class = match index % 3 {
                    0 => DisclosureClass::Direct,
                    1 => DisclosureClass::DeferredCandidate,
                    _ => DisclosureClass::Hidden,
                };
                toolkit.register_with_disclosure(
                    tool_definition(&format!("Tool_{index:02}")),
                    Arc::new(MockToolExecutor::new("ok")),
                    meta(class, 10),
                );
            }
            let toolkit = toolkit.create_filtered(&["Tool_23".to_string()]);
            let assembly = assemble_tool_definitions(&toolkit, &config, Some(context_window));
            let names = assembly
                .tools
                .iter()
                .map(|definition| definition.name.clone())
                .collect::<Vec<_>>();
            if let Some(previous) = previous_names.replace(names.clone()) {
                assert_eq!(
                    names, previous,
                    "unchanged tool schemas reordered at capability snapshot {generation}"
                );
            }
        }
    }
}

#[test]
fn off_mode_preserves_full_tool_list() {
    let toolkit = toolkit_with_direct_and_deferred(1_000);
    let config = ToolDisclosureConfig {
        enabled: ToolDisclosureMode::Off,
        ..ToolDisclosureConfig::default()
    };

    let assembly = assemble_tool_definitions(&toolkit, &config, Some(1_000));

    assert!(!assembly.activated);
    assert_eq!(tool_names(&assembly.tools), vec!["read", "task_create"]);
}

#[test]
fn on_mode_replaces_deferred_tools_with_bridge_schemas() {
    let toolkit = toolkit_with_direct_and_deferred(1);
    let config = ToolDisclosureConfig {
        enabled: ToolDisclosureMode::On,
        ..ToolDisclosureConfig::default()
    };

    let assembly = assemble_tool_definitions(&toolkit, &config, Some(1_000));

    assert!(assembly.activated);
    assert_eq!(
        tool_names(&assembly.tools),
        vec!["read", "tool_call", "tool_describe", "tool_search"]
    );
    assert_eq!(assembly.direct_tool_names, vec!["read"]);
    assert_eq!(assembly.deferred_tool_names, vec!["task_create"]);
}

#[test]
fn auto_mode_passes_through_when_deferred_schema_is_below_threshold() {
    let toolkit = toolkit_with_direct_and_deferred(99);

    let assembly =
        assemble_tool_definitions(&toolkit, &ToolDisclosureConfig::default(), Some(1_000));

    assert!(!assembly.activated);
    assert_eq!(assembly.threshold_tokens, Some(100));
    assert_eq!(tool_names(&assembly.tools), vec!["read", "task_create"]);
}

#[test]
fn auto_mode_activates_when_deferred_schema_exceeds_default_ten_percent_threshold() {
    let toolkit = toolkit_with_direct_and_deferred(101);

    let assembly =
        assemble_tool_definitions(&toolkit, &ToolDisclosureConfig::default(), Some(1_000));

    assert!(assembly.activated);
    assert_eq!(assembly.threshold_tokens, Some(100));
    assert_eq!(
        tool_names(&assembly.tools),
        vec!["read", "tool_call", "tool_describe", "tool_search"]
    );
}

#[test]
fn deferred_catalog_searches_name_description_and_top_level_parameter_names() {
    let mut toolkit = AgentToolkit::new();
    toolkit.register_with_disclosure(
        tool_definition("read"),
        Arc::new(MockToolExecutor::new("read ok")),
        meta(DisclosureClass::Direct, 10),
    );
    register_deferred_tool(
        &mut toolkit,
        "plugin__jira__create_issue",
        "Create a project issue",
        &["title", "body"],
    );
    register_deferred_tool(
        &mut toolkit,
        "plugin__files__archive",
        "Archive a file",
        &["path", "destination"],
    );

    let catalog = DeferredToolCatalog::from_toolkit(&toolkit, &ToolDisclosureConfig::default());

    let by_name = catalog.search("jira create", Some(5));
    assert_eq!(by_name[0].name, "plugin__jira__create_issue");

    let by_description = catalog.search("project issue", Some(5));
    assert_eq!(by_description[0].name, "plugin__jira__create_issue");

    let by_parameter = catalog.search("destination path", Some(5));
    assert_eq!(by_parameter[0].name, "plugin__files__archive");
    assert_eq!(by_parameter[0].category.as_deref(), Some("external"));
    assert_eq!(by_parameter[0].source_kind, ToolSourceKind::Plugin);
    assert!(by_parameter[0].parameters.is_none());
}

#[test]
fn deferred_catalog_limits_results_with_default_and_max_bounds() {
    let mut toolkit = AgentToolkit::new();
    for idx in 0..7 {
        register_deferred_tool(
            &mut toolkit,
            &format!("plugin__test__tool_{idx}"),
            "Shared lookup tool",
            &["query"],
        );
    }
    let config = ToolDisclosureConfig {
        search_default_limit: 5,
        search_max_limit: 6,
        ..ToolDisclosureConfig::default()
    };
    let catalog = DeferredToolCatalog::from_toolkit(&toolkit, &config);

    assert_eq!(catalog.search("lookup", None).len(), 5);
    assert_eq!(catalog.search("lookup", Some(20)).len(), 6);
}

#[test]
fn deferred_catalog_describes_only_deferred_tools_in_current_scope() {
    let mut toolkit = AgentToolkit::new();
    toolkit.register_with_disclosure(
        tool_definition("read"),
        Arc::new(MockToolExecutor::new("read ok")),
        meta(DisclosureClass::Direct, 10),
    );
    register_deferred_tool(
        &mut toolkit,
        "plugin__files__archive",
        "Archive a file",
        &["path", "destination"],
    );

    let catalog = DeferredToolCatalog::from_toolkit(&toolkit, &ToolDisclosureConfig::default());
    let described = catalog
        .describe("plugin__files__archive")
        .expect("deferred tool should be described");

    assert_eq!(described.name, "plugin__files__archive");
    assert!(described.parameters.get("properties").is_some());
    assert!(catalog.describe("read").is_none());
    assert!(catalog.describe("plugin__other__outside_scope").is_none());
}

#[test]
fn hidden_tools_are_absent_from_direct_and_deferred_model_surfaces() {
    let mut toolkit = toolkit_with_direct_and_deferred(1);
    toolkit.register_with_disclosure(
        tool_definition("Internal_Diagnostic"),
        Arc::new(MockToolExecutor::new("hidden")),
        meta(DisclosureClass::Hidden, 10),
    );

    for mode in [ToolDisclosureMode::Off, ToolDisclosureMode::On] {
        let assembly = assemble_tool_definitions(
            &toolkit,
            &ToolDisclosureConfig {
                enabled: mode,
                ..ToolDisclosureConfig::default()
            },
            Some(1_000),
        );
        assert!(!tool_names(&assembly.tools).contains(&"Internal_Diagnostic"));
        assert!(!assembly
            .direct_tool_names
            .iter()
            .any(|name| name == "Internal_Diagnostic"));
        assert!(!assembly
            .deferred_tool_names
            .iter()
            .any(|name| name == "Internal_Diagnostic"));
    }

    let catalog = DeferredToolCatalog::from_toolkit(&toolkit, &ToolDisclosureConfig::default());
    assert!(catalog.search("internal diagnostic", Some(5)).is_empty());
    assert!(catalog.describe("Internal_Diagnostic").is_none());
}
