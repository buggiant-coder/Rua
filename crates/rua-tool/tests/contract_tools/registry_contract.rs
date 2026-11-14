use std::collections::{BTreeSet, HashSet};
use std::sync::Arc;

use llm_multiplex::providers::gemini::{
    flatten_json_schema, lower_gemini_tool_parameters, validate_gemini_tool_parameters,
};
use rua_config::types::tools::default_builtin_tools;
use rua_tool::{
    register_all_builtin_tools, register_extended_tools, tool_catalog_from_specs, CommandExecutor,
    MockCommandExecutor, ToolProfile, ToolRegistry,
};

fn full_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    let executor: Arc<dyn CommandExecutor> = Arc::new(MockCommandExecutor::new());

    register_all_builtin_tools(&mut registry, executor, None).expect("builtin tools must register");
    register_extended_tools(&mut registry, ToolProfile::ServerFull)
        .expect("extended tools must register");

    registry
}

#[test]
fn runtime_catalog_carries_authoritative_display_groups() {
    let registry = full_registry();
    let specs = ["bash", "python", "update_plan"]
        .iter()
        .map(|name| registry.get_spec(name).unwrap())
        .collect::<Vec<_>>();
    let catalog = tool_catalog_from_specs(specs);

    assert_eq!(catalog["bash"]["display_group"], "shell");
    assert_eq!(catalog["python"]["display_group"], "shell");
    assert_eq!(catalog["update_plan"]["display_group"], "plan");
}

fn required_fields(registry: &ToolRegistry, name: &str) -> Vec<String> {
    registry
        .get_spec(name)
        .unwrap_or_else(|| panic!("missing tool spec: {name}"))
        .parameters
        .get("required")
        .and_then(|v| v.as_array())
        .unwrap_or_else(|| panic!("{name} required fields must be an array"))
        .iter()
        .map(|v| {
            v.as_str()
                .expect("required field must be string")
                .to_string()
        })
        .collect()
}

#[test]
fn default_tool_config_catalog_matches_default_runtime_builtin_tools() {
    let registry = full_registry();
    let mut runtime_names: BTreeSet<String> = registry
        .list_enabled()
        .into_iter()
        .map(str::to_string)
        .collect();

    runtime_names.insert("memory_search".to_string());
    runtime_names.insert("memory_get".to_string());
    runtime_names.insert("memory_write".to_string());
    runtime_names.insert("memory_update".to_string());
    runtime_names.insert("memory_delete".to_string());

    let catalog_names: BTreeSet<String> = default_builtin_tools()
        .into_iter()
        .map(|(name, _)| name)
        .collect();

    assert_eq!(
        catalog_names, runtime_names,
        "default tool config catalog must match the default runtime tool set"
    );
}

#[test]
fn builtin_registry_exports_stable_canonical_tool_names() {
    let registry = full_registry();
    let names: HashSet<&str> = registry.list_enabled().into_iter().collect();

    for name in [
        "bash",
        "read",
        "edit",
        "grep",
        "glob",
        "python",
        "conversation_search",
        "conversation_read_span",
        "recall_tool_result",
        "list_tools",
        "runtime_info",
    ] {
        assert!(names.contains(name), "missing expected tool: {name}");
    }

    for name in ["ask_user", "send_file"] {
        assert!(
            !names.contains(name),
            "retired tool must not be registered: {name}"
        );
    }
}

#[test]
fn model_task_surface_contains_only_the_six_tool_contract() {
    let registry = full_registry();
    let names: HashSet<&str> = registry.list_enabled().into_iter().collect();
    for name in [
        "task_create",
        "task_read",
        "task_list",
        "task_cancel",
        "task_send_input",
        "task_retry",
    ] {
        assert!(names.contains(name), "missing Task tool: {name}");
    }
    for retired in [
        "Task_Get",
        "task_await",
        "Task_Watch",
        "Task_Unwatch",
        "Task_HandoffWrite",
    ] {
        assert!(
            !names.contains(retired),
            "retired model Task tool must not be registered: {retired}"
        );
    }
}

#[test]
fn collaboration_and_sync_tools_share_the_standard_registry() {
    let registry = full_registry();
    let names: HashSet<_> = registry.list_enabled().into_iter().collect();
    for name in [
        "chat_send",
        "chat_read",
        "chat_stop",
        "sync_ls",
        "sync_send",
    ] {
        assert!(
            names.contains(name),
            "missing standard Registry tool: {name}"
        );
    }
}

#[test]
fn chat_send_documents_acp_runner_keys_not_product_names() {
    let registry = full_registry();
    let spec = registry.get_spec("chat_send").expect("chat_send spec");
    let description = spec.parameters["properties"]["acp_runner"]["description"]
        .as_str()
        .expect("acp_runner parameter description");
    assert!(description.contains("claude (Claude Code"));
    assert!(description.contains("not a product or CLI name"));
    assert!(description.contains("Custom runner keys"));
}

#[test]
fn public_tool_specs_have_object_parameters_and_descriptions() {
    let registry = full_registry();

    for spec in registry.list_specs() {
        assert!(!spec.name.is_empty(), "tool name must be populated");
        assert!(
            !spec.description.is_empty(),
            "{} description must be populated",
            spec.name
        );
        assert_eq!(
            spec.parameters.get("type").and_then(|v| v.as_str()),
            Some("object"),
            "{} parameters must be an object schema",
            spec.name
        );
        assert!(
            spec.parameters
                .get("properties")
                .and_then(|v| v.as_object())
                .is_some(),
            "{} must expose a properties object, even when the tool takes no args",
            spec.name
        );
        assert_eq!(spec.namespace, "builtin");
    }
}

#[test]
fn builtin_tool_schemas_lower_to_valid_gemini_parameters() {
    let registry = full_registry();

    for definition in registry.export_tool_definitions() {
        let flattened = flatten_json_schema(&definition.parameters);
        let lowered = lower_gemini_tool_parameters(&flattened);
        validate_gemini_tool_parameters(&lowered)
            .unwrap_or_else(|error| panic!("{}: {error}", definition.name));

        let source_properties = flattened["properties"]
            .as_object()
            .unwrap_or_else(|| panic!("{} source properties", definition.name));
        let lowered_properties = lowered["properties"]
            .as_object()
            .unwrap_or_else(|| panic!("{} lowered properties", definition.name));
        for (name, property) in source_properties {
            let Some(schema_type) = property.get("type").and_then(serde_json::Value::as_str) else {
                continue;
            };
            if matches!(
                schema_type,
                "string" | "number" | "integer" | "boolean" | "object" | "array"
            ) {
                assert!(
                    lowered_properties.contains_key(name),
                    "{} lost typed property {name}",
                    definition.name
                );
            }
        }

        let lowered_required: HashSet<_> = lowered["required"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(serde_json::Value::as_str)
            .collect();
        for name in flattened["required"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(serde_json::Value::as_str)
        {
            assert!(
                lowered_required.contains(name),
                "{} lost required property {name}",
                definition.name
            );
        }
    }
}

#[test]
fn qwenpaw_parity_core_required_fields_are_stable() {
    let registry = full_registry();

    assert_eq!(required_fields(&registry, "bash"), vec!["command"]);
    assert_eq!(required_fields(&registry, "read"), vec!["file_path"]);
    assert_eq!(required_fields(&registry, "edit"), vec!["patch"]);
    assert_eq!(required_fields(&registry, "grep"), vec!["pattern"]);
    assert_eq!(required_fields(&registry, "glob"), vec!["pattern"]);
    assert_eq!(required_fields(&registry, "python"), vec!["code"]);
    assert!(required_fields(&registry, "runtime_info").is_empty());
}

#[test]
fn llm_tool_definitions_preserve_schema_and_annotations() {
    let registry = full_registry();
    let definitions = registry.export_tool_definitions();

    let read = definitions
        .iter()
        .find(|definition| definition.name == "read")
        .expect("read definition");
    assert_eq!(
        read.parameters["required"],
        serde_json::json!(["file_path"])
    );
    assert!(read.parameters["properties"]["limit"].is_object());
    assert!(read.parameters["properties"].get("end_line").is_none());
    assert_eq!(read.annotations.read_only, Some(true));
    assert_eq!(read.annotations.destructive, Some(false));
    assert_eq!(read.annotations.requires_approval, Some(false));

    let edit = definitions
        .iter()
        .find(|definition| definition.name == "edit")
        .expect("edit definition");
    assert_eq!(edit.parameters["required"], serde_json::json!(["patch"]));
    assert!(edit
        .description
        .contains("Deleting, moving, or renaming files is not supported"));
    assert!(edit
        .description
        .contains("Every Update File line must start with"));
    assert!(edit.description.contains("re-read the target region"));
    assert!(edit.parameters["properties"]["patch"]["description"]
        .as_str()
        .expect("patch description")
        .contains(r#"encode it as `\\n`"#));
    assert!(!edit.description.contains("deleting, and moving"));
    assert_eq!(edit.annotations.read_only, Some(false));
    assert_eq!(edit.annotations.destructive, Some(true));
    assert_eq!(edit.annotations.requires_approval, Some(true));
}

#[test]
fn namespace_lookup_uses_registration_metadata() {
    let mut registry = ToolRegistry::new();
    registry
        .register(
            Arc::new(rua_tool::ReadFileTool::new()),
            rua_tool::ReadFileTool::spec(),
            rua_tool::ToolNamespace::Mcp,
        )
        .unwrap();
    assert_eq!(registry.get_spec("read").unwrap().namespace, "builtin");
    assert_eq!(
        registry.namespace_for("read"),
        Some(&rua_tool::ToolNamespace::Mcp)
    );
    assert_eq!(registry.namespace_for("missing"), None);
}
