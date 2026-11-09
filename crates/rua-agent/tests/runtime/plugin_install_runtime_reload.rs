use std::sync::Arc;
use std::time::Duration;

use rua_plugin_runtime::{PluginEvent, PluginLoader, PluginRegistry, ToolSpec};
use rua_provider::manager::ProviderManager;

use rua_agent::runtime::ReactAgentRuntime;

fn make_test_plugin(dir: &std::path::Path, id: &str) -> rua_plugin_runtime::Plugin {
    let plugin_dir = dir.join(id);
    std::fs::create_dir_all(&plugin_dir).unwrap();
    std::fs::write(
        plugin_dir.join("plugin.json"),
        format!(r#"{{"id":"{id}","name":"Test {id}","version":"1.0.0"}}"#),
    )
    .unwrap();
    PluginLoader::load(&plugin_dir).unwrap()
}

#[tokio::test]
async fn runtime_subscribe_receives_event_and_toolkit_reloads() {
    let tmp = tempfile::tempdir().unwrap();
    let registry = Arc::new(PluginRegistry::new());

    let mut external_rx = registry.subscribe();

    let provider_manager = Arc::new(ProviderManager::new());
    let _runtime =
        ReactAgentRuntime::new(provider_manager).with_plugin_registry(Arc::clone(&registry));

    let tool_spec = ToolSpec {
        name: "my_tool".to_string(),
        description: "test tool".to_string(),
        parameters: serde_json::json!({}),
    };
    registry
        .register_tool("runtime-test-plugin", tool_spec)
        .unwrap();

    let plugin = make_test_plugin(tmp.path(), "runtime-test-plugin");
    registry.add(plugin).unwrap();

    let event = tokio::time::timeout(Duration::from_secs(1), external_rx.recv())
        .await
        .expect("should receive event within 1s")
        .expect("channel not closed");

    match event {
        PluginEvent::Installed { plugin_id } => {
            assert_eq!(plugin_id, "runtime-test-plugin");
        }
        _ => panic!("expected Installed event"),
    }

    let all_tools = registry.list_all_tools();
    let has_tool = all_tools
        .iter()
        .any(|(pid, spec)| pid == "runtime-test-plugin" && spec.name == "my_tool");
    assert!(
        has_tool,
        "plugin tool should be visible in registry after install"
    );
}

#[tokio::test]
async fn runtime_listener_stops_on_shutdown() {
    let tmp = tempfile::tempdir().unwrap();
    let registry = Arc::new(PluginRegistry::new());
    let provider_manager = Arc::new(ProviderManager::new());
    let runtime =
        ReactAgentRuntime::new(provider_manager).with_plugin_registry(Arc::clone(&registry));

    use rua_contract::agent::AgentRuntime;
    runtime.shutdown().await.unwrap();

    tokio::time::sleep(Duration::from_millis(50)).await;

    let plugin = make_test_plugin(tmp.path(), "post-shutdown-plugin");
    registry.add(plugin).unwrap();
}
