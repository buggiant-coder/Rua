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
async fn restricted_policy_captures_selection_without_expanding_available_tool_names() {
    let temp = tempfile::tempdir().unwrap();
    let rua_root = RuaRoot::new(temp.path().to_path_buf());
    let workspace = agent_workspace(&rua_root, "research");
    workspace.ensure().unwrap();
    rua_config::atomic_write_json(&workspace.as_path().join("agent.json"), &serde_json::json!({
        "id":"research", "name":"Research",
        "tools":{"selection_policy":{"mode":"restricted","allowed_tools":[{"kind":"builtin","name":"read"}]},"builtin_tools":{}},
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
    assert!(configuration.mcp_servers.contains_key("explicit-fixture"));
    let policy = configuration.selection_policy;
    assert!(policy.selects(
        &rua_config::types::tools::ToolIdentity::builtin("read"),
        None
    ));
    assert!(!policy.selects(
        &rua_config::types::tools::ToolIdentity::builtin("future_tool"),
        None
    ));
    assert!(!policy.selects(
        &rua_config::types::tools::ToolIdentity::mcp("explicit-fixture", "new_tool"),
        None
    ));
    assert_eq!(runtime.stop().await, vec!["work_authority"]);
}

#[tokio::test]
async fn missing_tool_policy_is_rejected_without_rewriting() {
    let temp = tempfile::tempdir().unwrap();
    let root = RuaRoot::new(temp.path());
    let workspace = agent_workspace(&root, "legacy");
    workspace.ensure().unwrap();
    let path = workspace.as_path().join("agent.json");
    rua_config::atomic_write_json(
        &path,
        &serde_json::json!({
            "id":"legacy", "name":"Legacy", "tools":{"builtin_tools":{"read":{"max_bytes":100}}}
        }),
    )
    .unwrap();
    let before = std::fs::read(&path).unwrap();
    let runtime = WorkspaceRuntime::new(
        "legacy",
        "Legacy",
        workspace,
        PlatformProfile::Mobile,
        test_context(temp.path().to_path_buf()),
    );
    runtime.initialize_core().await.unwrap();
    runtime.initialize_metadata().await.unwrap();
    runtime.initialize_runtime().await.unwrap();
    let error = runtime.capability_configuration(None).await.unwrap_err();
    assert!(error.to_string().contains("required"));
    assert_eq!(std::fs::read(&path).unwrap(), before);
    runtime.stop().await;
}

#[tokio::test]
async fn captured_tool_behavior_preserves_extensions_and_is_independent_of_agent_defaults() {
    let temp = tempfile::tempdir().unwrap();
    let root = RuaRoot::new(temp.path());
    let workspace = agent_workspace(&root, "behavior-snapshot");
    workspace.ensure().unwrap();
    let path = workspace.as_path().join("agent.json");
    let mut agent = serde_json::json!({
        "id":"behavior-snapshot", "name":"Behavior",
        "tools":{"selection_policy":{"mode":"default_enabled","disabled_tools":[],"disabled_capabilities":[]},
            "builtin_tools":{
                "read":{"name":"read","enabled":false,"description":"catalog description","display_group":"file","parameters":{"type":"object"},"config":{"api_key":"private-secret"},"async_execution":true,"display_to_user":false,"future_behavior":{"mode":"snapshot"}},
                "off_device_tool":{"name":"off_device_tool","enabled":true,"async_execution":true,"future_behavior":7}
            }}
    });
    rua_config::atomic_write_json(&path, &agent).unwrap();
    let runtime = WorkspaceRuntime::new(
        "behavior-snapshot",
        "Behavior",
        workspace,
        PlatformProfile::Mobile,
        test_context(temp.path().to_path_buf()),
    );
    runtime.initialize_core().await.unwrap();
    runtime.initialize_metadata().await.unwrap();
    runtime.initialize_runtime().await.unwrap();
    let captured = runtime.capability_configuration(None).await.unwrap();
    assert_eq!(
        captured.extra["tool_behaviors"]["read"],
        serde_json::json!({"async_execution":true,"display_to_user":false,"future_behavior":{"mode":"snapshot"}})
    );
    assert_eq!(
        captured.extra["tool_behaviors"]["off_device_tool"],
        serde_json::json!({"async_execution":true,"future_behavior":7})
    );
    assert!(!serde_json::to_string(&captured)
        .unwrap()
        .contains("private-secret"));
    agent["tools"]["builtin_tools"]["read"]["async_execution"] = false.into();
    agent["tools"]["builtin_tools"]["read"]["display_to_user"] = true.into();
    rua_config::atomic_write_json(&path, &agent).unwrap();
    let next = runtime.capability_configuration(None).await.unwrap();
    assert_eq!(
        next.extra["tool_behaviors"]["read"]["async_execution"],
        false
    );
    assert_eq!(
        captured.extra["tool_behaviors"]["read"]["async_execution"],
        true
    );
    assert_eq!(
        captured.extra["tool_behaviors"]["read"]["display_to_user"],
        false
    );
    runtime.stop().await;
}
