use std::sync::Arc;

use react_agent::AgentToolkit;
use rua_agent::plugin_tool_handler::populate_plugin_tools;
use rua_plugin_runtime::{PluginLoader, PluginRegistry, ToolSpec};

fn make_test_plugin(dir: &std::path::Path) -> rua_plugin_runtime::Plugin {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(
        dir.join("plugin.json"),
        r#"{"id":"test-add-plugin","name":"Test Add Plugin","version":"1.0.0"}"#,
    )
    .unwrap();
    PluginLoader::load(dir).expect("plugin load failed")
}

#[tokio::test]
async fn populate_plugin_tools_injects_into_toolkit() {
    let tmp = tempfile::tempdir().unwrap();
    let plugin_dir = tmp.path().join("test-add-plugin");

    let registry = Arc::new(PluginRegistry::new());
    let plugin = make_test_plugin(&plugin_dir);

    registry.add(plugin).expect("registry add");
    registry
        .register_tool(
            "test-add-plugin",
            ToolSpec {
                name: "add".to_string(),
                description: "Add two numbers".to_string(),
                parameters: serde_json::json!({}),
            },
        )
        .expect("register tool");

    let mut toolkit = AgentToolkit::new();
    populate_plugin_tools(&mut toolkit, &registry);

    assert!(toolkit.has_tool("plugin__test-add-plugin__add"));
    assert_eq!(toolkit.len(), 1);

    let def = toolkit
        .get_definition("plugin__test-add-plugin__add")
        .expect("definition should exist");
    assert_eq!(def.description, "Add two numbers");
}

#[tokio::test]
async fn uninstall_removes_tools_from_registry() {
    let tmp = tempfile::tempdir().unwrap();
    let plugin_dir = tmp.path().join("test-add-plugin");

    let registry = Arc::new(PluginRegistry::new());
    let plugin = make_test_plugin(&plugin_dir);

    registry.add(plugin).expect("registry add");
    registry
        .register_tool(
            "test-add-plugin",
            ToolSpec {
                name: "add".to_string(),
                description: "Add two numbers".to_string(),
                parameters: serde_json::json!({}),
            },
        )
        .expect("register tool");

    assert!(!registry.list_all_tools().is_empty());

    registry.remove_plugin_tools("test-add-plugin");
    let _ = registry.remove("test-add-plugin");

    assert!(registry.list_all_tools().is_empty());
    assert!(registry.is_empty().unwrap());
}
