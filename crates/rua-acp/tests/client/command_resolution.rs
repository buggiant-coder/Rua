use std::path::PathBuf;

use rua_acp::client::acp_runtime_launch_plan;
use rua_config::types::AcpAgentConfig;
use rua_core::runtime_layout::RuntimeKind;

#[test]
fn acp_runtime_launch_plan_preserves_config_and_adds_runtime_env() {
    let config = AcpAgentConfig {
        enabled: true,
        command: "npx".to_string(),
        args: vec![
            "-y".to_string(),
            "@agentclientprotocol/claude-agent-acp@0.59.0".to_string(),
        ],
        env: [("ANTHROPIC_API_KEY".to_string(), "secret".to_string())].into(),
        trusted: true,
        tool_parse_mode: Default::default(),
        stdio_buffer_limit_bytes: Some(1024),
        extra: Default::default(),
    };
    let cwd = PathBuf::from("/tmp/rua-acp-cwd");
    let data_dir = PathBuf::from("/tmp/rua-cache");

    let plan = acp_runtime_launch_plan(&config, &cwd, &data_dir);

    assert_eq!(plan.runtime_kind, RuntimeKind::Native);
    assert_eq!(plan.process().expect("valid launch").args, config.args);
    assert_eq!(
        plan.process().expect("valid launch").env["ANTHROPIC_API_KEY"],
        "secret"
    );
    assert_eq!(
        plan.process().expect("valid launch").env["NPM_CONFIG_CACHE"],
        "/tmp/rua-cache/npm"
    );
    assert_eq!(config.command, "npx");
}

#[test]
fn acp_runtime_launch_plan_uses_runtime_cache_dir_not_probe_cwd_for_cache() {
    let config = AcpAgentConfig {
        enabled: true,
        command: "npx".to_string(),
        args: vec!["codex-acp".to_string()],
        env: Default::default(),
        trusted: true,
        tool_parse_mode: Default::default(),
        stdio_buffer_limit_bytes: Some(1024),
        extra: Default::default(),
    };
    let cwd = PathBuf::from("/tmp/rua-acp-probe-cwd");
    let data_dir = PathBuf::from("/tmp/rua-runtime-cache");

    let plan = acp_runtime_launch_plan(&config, &cwd, &data_dir);

    assert_eq!(
        plan.process().expect("valid launch").env["NPM_CONFIG_CACHE"],
        "/tmp/rua-runtime-cache/npm"
    );
    assert_ne!(
        plan.process().expect("valid launch").env["NPM_CONFIG_CACHE"],
        "/tmp/rua-acp-probe-cwd/npm"
    );
}

#[tokio::test]
async fn native_probe_starts_relative_runner_in_the_requested_workspace() {
    let root = tempfile::tempdir().unwrap();
    std::fs::copy(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mock_acp_agent.py"),
        root.path().join("runner.py"),
    )
    .unwrap();
    let config: AcpAgentConfig = serde_json::from_value(serde_json::json!({
        "enabled": true, "command": if cfg!(windows) { "python" } else { "python3" },
        "args": ["runner.py"], "env": {}, "trusted": true
    }))
    .unwrap();
    rua_acp::client::AcpClientAdapter::probe_initialize_for_runtime_with_timeout(
        &config,
        root.path(),
        &root.path().join("cache"),
        std::time::Duration::from_secs(3),
    )
    .await
    .unwrap();
}

#[test]
fn android_runner_plans_refresh_materials_without_recording_device_paths() {
    // Nextest gives this test its own process, including environment variables.
    for _ in 0..2 {
        let root = tempfile::tempdir().unwrap();
        let native = root.path().join("native");
        let data = root.path().join("data");
        let home = root.path().join("home");
        let workspace = home.join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        std::env::set_var("RUA_HOST_PROFILE", "mobile-apk");
        std::env::set_var("RUA_ANDROID_NATIVE_DIR", &native);
        std::env::set_var("RUA_ANDROID_DATA_DIR", &data);
        std::env::set_var("HOME", &home);
        let config: AcpAgentConfig = serde_json::from_value(serde_json::json!({
            "enabled": true, "command": "codex-acp", "args": [],
            "env": {"TEST_SECRET":"fixture-secret"}, "trusted": false
        }))
        .unwrap();
        let cache = home.join("cache");
        let plan = acp_runtime_launch_plan(&config, &workspace, &cache);
        assert!(rua_acp::client::AcpClientAdapter::build_acp_agent_from_plan(&plan).is_err());
        for path in [
            native.join("libproot.so"),
            data.join("runtime/debian/usr/bin/env"),
        ] {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "").unwrap();
        }
        let missing = acp_runtime_launch_plan(&config, &workspace, &cache);
        assert!(missing.process().unwrap_err().to_string().contains("node"));
        let npm = data.join("runtime/debian/usr/local/bin/npm");
        std::fs::create_dir_all(npm.parent().unwrap()).unwrap();
        std::fs::write(npm, "").unwrap();
        let installer = rua_core::runtime_authority::RuntimeAuthority::from_current_process(
            None, &workspace, &cache,
        );
        assert!(installer
            .plan(
                rua_core::runtime_authority::ProcessShape::Install,
                rua_core::runtime_authority::ToolchainIntent::NodePackageManager,
                Default::default(),
            )
            .process()
            .unwrap_err()
            .to_string()
            .contains("node"));
        let node = data.join("runtime/debian/usr/local/bin/node");
        std::fs::create_dir_all(node.parent().unwrap()).unwrap();
        std::fs::write(node, "").unwrap();
        let refreshed = acp_runtime_launch_plan(&config, &workspace, &cache);
        let process = refreshed.process().unwrap();
        assert_eq!(process.command, native.join("librua-runtime-gate.so"));
        assert!(process
            .args
            .contains(&native.join("libproot.so").display().to_string()));
        assert!(process
            .args
            .contains(&format!("--cwd={}", workspace.display())));
        assert!(!process
            .args
            .iter()
            .any(|arg| arg.contains("fixture-secret")));
        assert!(process.env["PATH"].starts_with("/root/.local/bin:"));
        assert!(!process.env["PATH"].contains("node-v"));
        assert_eq!(config.command, "codex-acp");
        assert!(
            acp_runtime_launch_plan(&config, &workspace.join("absent"), &cache)
                .process()
                .is_err()
        );
    }
}
