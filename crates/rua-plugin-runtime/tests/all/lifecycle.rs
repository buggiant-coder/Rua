use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use rua_plugin_runtime::client::{PluginClientConfig, PluginHealth};
use rua_plugin_runtime::protocol::{InvokeCliParams, InvokeHookParams, InvokeSkillParams};
use rua_plugin_runtime::PluginClient;

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("mock_plugin.py")
}

fn has_python() -> bool {
    std::process::Command::new("python3")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn mock_config(env: BTreeMap<String, String>) -> PluginClientConfig {
    let mut cfg = PluginClientConfig::new("mock-test", "unused");
    cfg.command = Some("python3".into());
    cfg.args = Some(vec![fixture_path().to_string_lossy().into_owned()]);
    cfg.env = env;
    cfg.startup_timeout = Duration::from_secs(10);
    cfg.call_timeout = Duration::from_secs(5);
    cfg.heartbeat_interval = Duration::from_secs(120);
    cfg
}

#[tokio::test]
async fn spawn_init_list_call_shutdown() {
    if !has_python() {
        eprintln!("python3 not found, skipping");
        return;
    }

    let cfg = mock_config(BTreeMap::new());
    let mut client = PluginClient::spawn(cfg).await.expect("spawn");

    assert!(client.is_running());
    assert_eq!(client.health(), PluginHealth::Healthy);

    let init = client.initialize_result().await.expect("init result");
    assert_eq!(init.server_name, "mock-plugin");

    let tools = client.list_tools().await.expect("list_tools");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "greet");

    let result = client
        .call_tool("greet", serde_json::json!({"name": "rust"}))
        .await
        .expect("call_tool");
    assert!(!result.is_error);
    assert_eq!(result.text(), "hello rust");

    client.shutdown().await.expect("shutdown");
    assert!(!client.is_running());
    assert_eq!(client.health(), PluginHealth::Stopped);
}

#[tokio::test]
async fn invoke_plugin_methods() {
    if !has_python() {
        eprintln!("python3 not found, skipping");
        return;
    }

    let cfg = mock_config(BTreeMap::new());
    let mut client = PluginClient::spawn(cfg).await.expect("spawn");

    let hook_result = client
        .invoke_hook(InvokeHookParams {
            hook_point: "pre_prompt".into(),
            payload: "{}".into(),
        })
        .await
        .expect("invoke_hook");
    assert_eq!(hook_result.results.len(), 1);
    assert_eq!(hook_result.results[0].output, "hooked:pre_prompt");

    let skill_result = client
        .invoke_skill(InvokeSkillParams {
            plugin_id: "mock".into(),
            skill_name: "summarize".into(),
            input: "hello".into(),
        })
        .await
        .expect("invoke_skill");
    assert_eq!(skill_result.output, "skill:summarize:hello");

    let cli_result = client
        .invoke_cli(InvokeCliParams {
            plugin_id: "mock".into(),
            cmd_name: "status".into(),
            args: serde_json::json!({}),
        })
        .await
        .expect("invoke_cli");
    assert_eq!(cli_result.output, "cli:status");

    client.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn heartbeat_detects_crash() {
    if !has_python() {
        eprintln!("python3 not found, skipping");
        return;
    }

    let mut env = BTreeMap::new();
    env.insert("MOCK_PLUGIN_CRASH_AFTER".into(), "1".into());

    let mut cfg = mock_config(env);
    cfg.heartbeat_interval = Duration::from_secs(1);

    let mut client = PluginClient::spawn(cfg).await.expect("spawn");

    let tools = client.list_tools().await.expect("list_tools");
    assert_eq!(tools.len(), 1);

    let mut health_rx = client.health_watch();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if matches!(
            *health_rx.borrow_and_update(),
            PluginHealth::Unhealthy { .. }
        ) {
            break;
        }
        if tokio::time::Instant::now() > deadline {
            panic!("timed out waiting for unhealthy");
        }
        tokio::select! {
            _ = health_rx.changed() => {}
            _ = tokio::time::sleep(Duration::from_millis(100)) => {}
        }
    }

    client.shutdown().await.ok();
}

#[tokio::test]
async fn explicitly_removed_environment_is_not_inherited_or_reintroduced() {
    std::env::set_var("PLUGIN_FIXTURE_REMOVE", "parent-secret");
    let mut cfg = mock_config(BTreeMap::from([
        (
            "MOCK_PLUGIN_REQUIRE_ABSENT".into(),
            "PLUGIN_FIXTURE_REMOVE".into(),
        ),
        ("PLUGIN_FIXTURE_REMOVE".into(), "declared-secret".into()),
    ]));
    cfg.env_remove.insert("PLUGIN_FIXTURE_REMOVE".into());
    let mut client = PluginClient::spawn(cfg).await.unwrap();
    assert_eq!(
        client
            .call_tool("greet", serde_json::json!({"name":"removed"}))
            .await
            .unwrap()
            .text(),
        "hello removed"
    );
    client.shutdown().await.unwrap();
}
