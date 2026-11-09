use mcp_manager::{
    manager::McpManager,
    types::{McpConfig, McpServerId, ToolFilter},
};
use rua_core::runtime_authority::{RuntimeAuthority, RuntimeAuthorityConfig};
use rua_core::runtime_layout::{HostProfile, LinuxCompatLayout, RuntimeLayout};
use rua_mcp::{
    manager::{stdio_config_with_runtime_authority, McpManagerImpl},
    transports::stdio::StdioConfig,
    McpMobilePolicy, PlatformProfile,
};
use std::{collections::BTreeMap, path::Path, time::Duration};

fn touch(path: &Path) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, "").unwrap();
}
fn config() -> StdioConfig {
    StdioConfig {
        server_key: "portable".into(),
        server_name: "portable".into(),
        command: "node".into(),
        args: vec!["fixture.js".into()],
        env: BTreeMap::from([("TEST_SECRET".into(), "fixture-secret".into())]),
        cwd: None,
        startup_timeout: Duration::from_secs(2),
        call_timeout: Duration::from_secs(2),
        stderr_log_prefix: None,
    }
}
fn authority(root: &Path) -> RuntimeAuthority {
    let home = root.join("home");
    let linux = LinuxCompatLayout::android(&root.join("native"), &root.join("data"), &home);
    RuntimeAuthority::from_layout_with_probe(
        RuntimeLayout {
            host_profile: HostProfile::MobileApk,
            os_family: "android".into(),
            linux_compat: Some(linux),
        },
        RuntimeAuthorityConfig {
            workspace_dir: home.join("workspace"),
            cache_dir: home.join("cache"),
            path_entries: vec![root.join("host-bin")],
            pathext: vec![],
            windows_shell_preference: None,
        },
        |path| path.is_file(),
    )
}

#[test]
fn portable_stdio_follows_workspace_and_missing_guest_node_or_cwd_rejects_material() {
    for _ in 0..2 {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        std::fs::create_dir_all(root.join("home/workspace/subdir")).unwrap();
        touch(&root.join("native/libproot.so"));
        touch(&root.join("host-bin/node"));
        let guest_env = root.join("data/runtime/debian/usr/bin/env");
        let guest_node = root.join("data/runtime/debian/usr/local/bin/node");
        let error = stdio_config_with_runtime_authority(config(), &authority(root)).unwrap_err();
        assert!(error.to_string().contains("Linux Compatibility Runtime"));
        touch(&guest_env);
        assert!(
            stdio_config_with_runtime_authority(config(), &authority(root))
                .unwrap_err()
                .to_string()
                .contains("node")
        );
        touch(&guest_node);
        let planned = stdio_config_with_runtime_authority(config(), &authority(root)).unwrap();
        assert!(planned.command.ends_with("native/librua-runtime-gate.so"));
        assert!(planned
            .args
            .iter()
            .any(|arg| arg.ends_with("/native/libproot.so")));
        assert!(planned
            .args
            .contains(&format!("--cwd={}", root.join("home/workspace").display())));
        assert!(!planned
            .args
            .iter()
            .any(|arg| arg.contains("fixture-secret")));
        assert_eq!(planned.env["TEST_SECRET"], "fixture-secret");
        let mut relative = config();
        relative.cwd = Some("subdir".into());
        assert!(
            stdio_config_with_runtime_authority(relative, &authority(root))
                .unwrap()
                .args
                .contains(&format!(
                    "--cwd={}",
                    root.join("home/workspace/subdir").display()
                ))
        );
        let mut invalid = config();
        invalid.cwd = Some("missing-directory".into());
        assert!(
            stdio_config_with_runtime_authority(invalid, &authority(root))
                .unwrap_err()
                .to_string()
                .contains("Working directory")
        );
    }
}

#[tokio::test]
async fn missing_runtime_is_reported_until_explicit_apply_refreshes_inventory() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    // Nextest isolates this test's environment from other tests.
    std::env::set_var("RUA_ANDROID_NATIVE_DIR", root.join("native"));
    std::env::set_var("RUA_ANDROID_DATA_DIR", root.join("data"));
    std::env::set_var("HOME", root.join("home"));
    std::fs::create_dir_all(root.join("home/workspace")).unwrap();
    let manager = McpManagerImpl::new(McpMobilePolicy::new(PlatformProfile::MobileDefault))
        .with_runtime_dirs(root.join("home/workspace"), root.join("home/cache"));
    let config: McpConfig = serde_json::from_value(serde_json::json!({"clients":{"portable":{
        "name":"portable", "enabled":true, "transport":"stdio", "command":"node", "args":["fixture.js"], "auto_connect":"manual"
    }}})).unwrap();
    let report = manager.load_config(config.clone()).await.unwrap();
    assert_eq!(report.errors.len(), 1);
    let statuses = manager.list_servers().await.unwrap();
    assert_eq!(statuses.len(), 1);
    assert!(statuses[0]
        .last_error
        .as_ref()
        .unwrap()
        .contains("Linux Compatibility Runtime"));
    let filter = ToolFilter {
        server_id: Some(McpServerId::new("portable")),
        ..Default::default()
    };
    assert!(manager
        .list_tools(filter)
        .await
        .unwrap_err()
        .to_string()
        .contains("Linux Compatibility Runtime"));
    touch(&root.join("native/libproot.so"));
    touch(&root.join("data/runtime/debian/usr/bin/env"));
    touch(&root.join("data/runtime/debian/usr/local/bin/node"));
    assert!(manager.list_servers().await.unwrap()[0]
        .last_error
        .is_some());
    assert!(manager.reload(config).await.unwrap().errors.is_empty());
    assert!(manager.list_servers().await.unwrap()[0]
        .last_error
        .is_none());
    assert_eq!(manager.servers.read().await.len(), 1);
    manager.shutdown_all().await.unwrap();
}

#[tokio::test]
async fn stdio_timeout_does_not_replay_a_possibly_executed_request() {
    let temp = tempfile::tempdir().unwrap();
    let log = temp.path().join("requests");
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mock_mcp_server.py");
    let manager = McpManagerImpl::new(McpMobilePolicy::new(PlatformProfile::Desktop));
    let config: McpConfig = serde_json::from_value(serde_json::json!({"clients":{"once":{
        "name":"once", "enabled":true, "transport":"stdio", "command":"python3", "args":[script],
        "auto_connect":"manual", "call_timeout_secs":1,
        "env":{"MOCK_SCENARIO":"delay", "MOCK_DELAY_MS":"1500", "MOCK_REQUEST_LOG":log}
    }}}))
    .unwrap();
    assert!(manager.load_config(config).await.unwrap().errors.is_empty());
    assert!(manager
        .call_tool(
            &McpServerId::new("once"),
            "echo",
            serde_json::json!({"msg":"once"}),
            Default::default()
        )
        .await
        .is_err());
    tokio::time::sleep(Duration::from_millis(100)).await;
    let requests = std::fs::read_to_string(log).unwrap();
    assert_eq!(requests.matches("tools/call").count(), 1);
    assert_eq!(requests.matches("notifications/cancelled").count(), 1);
    manager.shutdown_all().await.unwrap();
}

#[tokio::test]
async fn disabled_portable_configuration_is_stored_but_enabling_requires_local_material() {
    let temp = tempfile::tempdir().unwrap();
    let id = McpServerId::new("disabled");
    let manager = McpManagerImpl::new(McpMobilePolicy::new(PlatformProfile::Desktop));
    let mut config: McpConfig = serde_json::from_value(serde_json::json!({"clients":{"disabled":{
        "name":"Disabled fixture", "enabled":false, "transport":"stdio",
        "command":temp.path().join("absent-executable"), "auto_connect":"manual"
    }}}))
    .unwrap();
    assert!(manager
        .load_config(config.clone())
        .await
        .unwrap()
        .errors
        .is_empty());
    assert!(manager
        .call_tool(&id, "echo", serde_json::json!({}), Default::default())
        .await
        .unwrap_err()
        .to_string()
        .contains("disabled"));
    assert!(manager
        .connect_server(&id)
        .await
        .unwrap_err()
        .to_string()
        .contains("disabled"));
    config.clients.get_mut(&id).unwrap().enabled = true;
    assert!(manager
        .reload(config.clone())
        .await
        .unwrap()
        .errors
        .contains_key(&id));
    config.clients.get_mut(&id).unwrap().enabled = false;
    assert!(manager.reload(config).await.unwrap().errors.is_empty());
    let status = manager.list_servers().await.unwrap();
    assert!(!status[0].enabled);
    assert!(status[0].last_error.is_none());
    manager.shutdown_all().await.unwrap();
}
