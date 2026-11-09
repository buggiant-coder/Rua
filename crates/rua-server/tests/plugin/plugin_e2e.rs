use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use rua_plugin_runtime::client::{PluginClientConfig, PluginHealth};
use rua_plugin_runtime::protocol::{InvokeCliParams, InvokeHookParams, InvokeSkillParams};
use rua_plugin_runtime::{HookPoint, ToolSpec};
use rua_plugin_runtime::{PluginClient, PluginRegistry};

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../rua-plugin-runtime/tests/fixtures/hello_world_plugin")
}

fn python_sdk_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../python_sdk")
}

fn has_python() -> bool {
    std::process::Command::new("python3")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn hello_world_config() -> PluginClientConfig {
    let fixture = fixture_dir();
    let sdk = python_sdk_dir();
    let pythonpath = format!("{}:{}", sdk.display(), fixture.display());

    let mut env = BTreeMap::new();
    env.insert("PYTHONPATH".into(), pythonpath);

    let mut cfg = PluginClientConfig::new("hello_world", "hello_world");
    cfg.command = Some("python3".into());
    cfg.args = Some(vec!["-m".into(), "hello_world".into()]);
    cfg.working_dir = fixture.clone();
    cfg.env = env;
    cfg.startup_timeout = Duration::from_secs(10);
    cfg.call_timeout = Duration::from_secs(5);
    cfg.heartbeat_interval = Duration::from_secs(120);
    cfg
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn plugin_e2e_full_lifecycle() {
    if !has_python() {
        eprintln!("python3 not found, skipping");
        return;
    }

    let tmp = tempfile::TempDir::new().expect("tempdir");
    let plugins_json = tmp.path().join("plugins.json");

    let plugin_mgr =
        rua_plugin::PluginManager::with_persistence(plugins_json.clone()).expect("plugin mgr");
    plugin_mgr
        .install(rua_plugin::InstallRequest {
            plugin_id: "hello_world".into(),
            version: Some("1.0.0".into()),
            ..Default::default()
        })
        .await
        .expect("install");

    assert!(plugin_mgr.is_enabled("hello_world").await);

    let cfg = hello_world_config();
    let mut client = PluginClient::spawn(cfg).await.expect("spawn");

    assert!(client.is_running());
    assert_eq!(client.health(), PluginHealth::Healthy);

    let tools = client.list_tools().await.expect("list_tools");
    assert_eq!(tools.len(), 1, "expected exactly 1 tool");
    assert_eq!(tools[0].name, "say_hi");

    let result = client
        .call_tool("say_hi", serde_json::json!({"name": "rust"}))
        .await
        .expect("call_tool");
    assert!(!result.is_error);
    assert_eq!(result.text(), "hi rust");

    let hook_result = client
        .invoke_hook(InvokeHookParams {
            hook_point: "after_tool_call".into(),
            payload: r#"{"tool":"say_hi"}"#.into(),
        })
        .await
        .expect("invoke_hook");
    assert_eq!(hook_result.results.len(), 1);
    assert_eq!(hook_result.results[0].hook_name, "post_tool_call_log");
    assert!(hook_result.results[0].output.contains("logged"));

    let skill_result = client
        .invoke_skill(InvokeSkillParams {
            plugin_id: "hello_world".into(),
            skill_name: "greeting".into(),
            input: "rua".into(),
        })
        .await
        .expect("invoke_skill");
    assert_eq!(skill_result.output, "greetings, rua!");

    let cli_result = client
        .invoke_cli(InvokeCliParams {
            plugin_id: "hello_world".into(),
            cmd_name: "hi".into(),
            args: serde_json::json!({"name": "world"}),
        })
        .await
        .expect("invoke_cli");
    assert_eq!(cli_result.output, "hi world");

    let registry = Arc::new(PluginRegistry::new());
    let plugin_mgr = Arc::new(plugin_mgr);
    {
        let pm = Arc::clone(&plugin_mgr);
        registry.set_enabled_checker(Arc::new(move |id| pm.is_enabled_sync(id)));
    }

    registry
        .register_tool(
            "hello_world",
            ToolSpec {
                name: "say_hi".into(),
                description: "Say hi to someone".into(),
                parameters: serde_json::json!({}),
            },
        )
        .expect("register_tool");
    registry
        .register_hook(
            "hello_world",
            rua_plugin_runtime::HookSpec {
                hook_point: HookPoint::AfterToolCall,
                hook_name: "post_tool_call_log".into(),
                callback: "on_post_tool_call".into(),
                priority: 100,
            },
        )
        .await
        .expect("register_hook");
    registry
        .register_cli(
            "hello_world",
            rua_plugin_runtime::CliCommandSpec {
                name: "hi".into(),
                description: "Say hi from CLI".into(),
                args: vec![],
                callback: "cli_hi".into(),
            },
        )
        .expect("register_cli");
    registry
        .track_skill("hello_world", "greeting")
        .expect("track_skill");

    assert_eq!(registry.list_tools("hello_world").len(), 1);
    assert_eq!(registry.list_hooks("hello_world").await.len(), 1);
    assert_eq!(registry.list_cli_commands("hello_world").len(), 1);
    assert_eq!(registry.tracked_skills("hello_world").len(), 1);

    plugin_mgr
        .toggle("hello_world", false)
        .await
        .expect("toggle off");
    assert!(!plugin_mgr.is_enabled("hello_world").await);
    assert!(!registry.is_plugin_enabled("hello_world"));

    let hooks_disabled = registry.get_hooks_for_point(HookPoint::AfterToolCall).await;
    assert_eq!(hooks_disabled.len(), 1, "hook still registered but...");
    let dispatch_result = registry
        .dispatch_hook(HookPoint::AfterToolCall, r#"{"tool":"say_hi"}"#)
        .await
        .expect("dispatch_hook disabled");
    assert!(
        dispatch_result.is_empty(),
        "dispatch should skip disabled plugin"
    );

    plugin_mgr
        .toggle("hello_world", true)
        .await
        .expect("toggle on");
    assert!(registry.is_plugin_enabled("hello_world"));

    registry.remove_plugin_tools("hello_world");
    registry.remove_plugin_hooks("hello_world").await;
    registry.remove_plugin_cli_commands("hello_world");
    registry.untrack_plugin_skills("hello_world");

    assert!(registry.list_tools("hello_world").is_empty());
    assert!(registry.list_hooks("hello_world").await.is_empty());
    assert!(registry.list_cli_commands("hello_world").is_empty());
    assert!(registry.tracked_skills("hello_world").is_empty());

    plugin_mgr
        .uninstall("hello_world")
        .await
        .expect("uninstall");

    let records = plugin_mgr.list().await;
    assert!(
        records.is_empty(),
        "plugins.json should have no records after uninstall"
    );

    let persisted = std::fs::read_to_string(&plugins_json).unwrap_or_default();
    assert!(
        !persisted.contains("hello_world"),
        "plugin ID should not appear in persisted plugins.json"
    );

    client.shutdown().await.expect("shutdown");
    assert!(!client.is_running());
    assert_eq!(client.health(), PluginHealth::Stopped);
}
