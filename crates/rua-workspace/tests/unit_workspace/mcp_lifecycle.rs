use rua_workspace::{PlatformProfile, RuaRoot, WorkspaceRuntime};
use std::sync::Arc;

use crate::test_helpers::{agent_workspace, test_context};

#[tokio::test]
async fn first_mcp_application_does_not_replace_an_initialized_react_runtime() {
    let temp = tempfile::tempdir().unwrap();
    let root = RuaRoot::new(temp.path());
    let workspace = agent_workspace(&root, "first-mcp");
    workspace.ensure().unwrap();
    let runtime = WorkspaceRuntime::new(
        "first-mcp",
        "First MCP",
        workspace,
        PlatformProfile::Mobile,
        test_context(temp.path().to_path_buf()),
    );
    runtime.initialize_core().await.unwrap();
    runtime.initialize_metadata().await.unwrap();
    runtime.initialize_runtime().await.unwrap();
    assert!(runtime.mcp_manager().await.is_none());
    let react = runtime.react_runtime().await.unwrap();
    let config: mcp_manager::types::McpConfig = serde_json::from_value(serde_json::json!({
        "clients": {"explicit": {"name":"Explicit", "enabled":false, "transport":"stdio", "command":"fixture", "auto_connect":"manual"}}
    })).unwrap();
    let report = runtime
        .apply_mcp_configuration(|| async { Ok(config) })
        .await
        .unwrap();
    assert!(report.errors.is_empty());
    assert!(runtime.mcp_manager().await.is_some());
    assert!(Arc::ptr_eq(&react, &runtime.react_runtime().await.unwrap()));
    runtime.stop().await;
}

#[tokio::test]
async fn concurrent_runtime_initialization_keeps_one_mcp_manager_and_stop_closes_it() {
    let temp = tempfile::tempdir().unwrap();
    let rua_root = RuaRoot::new(temp.path().to_path_buf());
    let workspace = agent_workspace(&rua_root, "default");
    workspace.ensure().unwrap();
    rua_config::atomic_write_json(&workspace.as_path().join("agent.json"), &serde_json::json!({
        "id":"default", "name":"Default",
        "mcp": {"clients":{"disabled-fixture":{
            "name":"fixture", "enabled":false, "transport":"stdio", "command":"no-driver-needed", "auto_connect":"manual"
        }}}
    })).unwrap();
    let runtime = Arc::new(WorkspaceRuntime::new(
        "default",
        "Default",
        workspace,
        PlatformProfile::Desktop,
        test_context(temp.path().to_path_buf()),
    ));
    runtime.initialize_core().await.unwrap();
    runtime.initialize_metadata().await.unwrap();
    let (a, b) = tokio::join!(runtime.initialize_runtime(), runtime.initialize_runtime());
    a.unwrap();
    b.unwrap();
    let first = runtime.mcp_manager().await.unwrap();
    let servers = first.list_servers().await.unwrap();
    assert!(servers
        .iter()
        .any(|server| server.key.as_str() == "disabled-fixture"));
    runtime.initialize_runtime().await.unwrap();
    assert!(Arc::ptr_eq(&first, &runtime.mcp_manager().await.unwrap()));
    assert_eq!(first.list_servers().await.unwrap().len(), servers.len());
    assert_eq!(runtime.stop().await, vec!["work_authority"]);
    assert!(
        first.list_servers().await.unwrap().is_empty(),
        "MCP manager must be registered with the shutdown owner"
    );
}

#[tokio::test]
async fn default_disabled_tools_preserve_explicit_mcp_without_bundled_expansion() {
    let temp = tempfile::tempdir().unwrap();
    let rua_root = RuaRoot::new(temp.path().to_path_buf());
    let workspace = agent_workspace(&rua_root, "research");
    workspace.ensure().unwrap();
    rua_config::atomic_write_json(&workspace.as_path().join("agent.json"), &serde_json::json!({
        "id":"research", "name":"Research",
        "tools":{"default_enabled":false,"builtin_tools":{}},
        "mcp":{"clients":{"explicit-fixture":{
            "name":"Explicit", "enabled":true,"transport":"stdio","command":"no-driver-needed","auto_connect":"manual"
        }}}
    })).unwrap();
    let runtime = WorkspaceRuntime::new(
        "research",
        "Research",
        workspace,
        PlatformProfile::Desktop,
        test_context(temp.path().to_path_buf()),
    );
    runtime.initialize_core().await.unwrap();
    runtime.initialize_metadata().await.unwrap();
    runtime.initialize_runtime().await.unwrap();
    let configuration = runtime.capability_configuration(None).await.unwrap();
    assert_eq!(
        configuration
            .mcp_servers
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["explicit-fixture"]
    );
    assert!(configuration.plugin_ids.is_empty());
    assert!(!configuration
        .tools
        .iter()
        .any(|name| name == "bash" || name == "edit"));
    assert_eq!(runtime.stop().await, vec!["work_authority"]);
}
