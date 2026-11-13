use rua_cli::init::{run, InitArgs};

#[test]
fn init_creates_workspace_structure() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("test-workspace");

    run(InitArgs {}, &target).unwrap();

    assert!(target.join("workspaces").is_dir());
    assert!(target.join("local_models").is_dir());
    assert!(target.join("envs.json").is_file());
    assert!(target.join("workspaces/default/agent.json").is_file());

    let secret = dir.path().join("test-workspace.secret");
    assert!(secret.is_dir());

    let agent: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(target.join("workspaces/default/agent.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(agent["id"], "default");
    assert!(!agent["tools"]["builtin_tools"]
        .as_object()
        .unwrap()
        .is_empty());
    let root: serde_json::Value =
        serde_json::from_slice(&std::fs::read(target.join("config.json")).unwrap()).unwrap();
    for field in ["tools", "channels", "mcp", "acp", "show_tool_details"] {
        assert!(root.get(field).is_none(), "Agent setting {field} at root");
    }
}

#[test]
fn config_defaults_write_agent_settings_and_keep_tool_inventory() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("rua");
    run(InitArgs {}, &target).unwrap();
    let mut agent = rua_config::load_agent_config("default", &target).unwrap();
    let tools = agent.tools.clone();
    rua_cli::config::configure_defaults(&mut agent).unwrap();
    let workspace = std::path::Path::new(&agent.workspace_dir);
    rua_config::loader::save_agent_config(&agent, workspace).unwrap();
    assert_eq!(
        rua_config::get_heartbeat_config(&target, None).unwrap()["every"],
        "6h"
    );
    rua_config::update_last_dispatch(&target, "console", "user", "session", None).unwrap();
    let saved = rua_config::load_agent_config("default", &target).unwrap();
    assert_eq!(saved.last_dispatch.unwrap()["session_id"], "session");
    assert_eq!(
        saved.channels.unwrap()["dingtalk"]["show_tool_details"],
        true
    );
    assert_eq!(saved.tools, tools);
    let root: serde_json::Value =
        serde_json::from_slice(&std::fs::read(target.join("config.json")).unwrap()).unwrap();
    assert!(root.get("last_dispatch").is_none());
    assert!(root["agents"].get("heartbeat").is_none());
}

#[test]
fn init_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("ws");

    for _ in 0..2 {
        run(InitArgs {}, &target).unwrap();
    }

    assert!(target.join("workspaces/default/agent.json").is_file());
}

#[test]
fn config_commands_initialize_agent_and_reject_retired_root_settings() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("rua");
    let fallback = temp.path().join("must-not-use");
    let run_cli = |args: &[&str]| {
        std::process::Command::new(env!("CARGO_BIN_EXE_rua"))
            .env("RUA_ROOT", &fallback)
            .arg("--rua-root")
            .arg(&root)
            .args(args)
            .output()
            .unwrap()
    };
    let output = run_cli(&["config", "init", "--defaults", "--accept-security"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let agent = rua_config::load_agent_config("default", &root).unwrap();
    assert_eq!(agent.heartbeat.unwrap()["every"], "6h");
    assert!(
        !fallback.exists(),
        "--rua-root must take precedence over RUA_ROOT"
    );
    assert!(!agent.tools.unwrap()["builtin_tools"]
        .as_object()
        .unwrap()
        .is_empty());
    let before = std::fs::read(root.join("config.json")).unwrap();
    for (key, value) in [
        ("tools", "{}"),
        ("agents.running", "{}"),
        ("security.file_guard", "{}"),
        ("agents", r#"{"heartbeat":{"enabled":true}}"#),
    ] {
        let output = run_cli(&["config", "set", key, value]);
        assert!(!output.status.success(), "retired setting accepted: {key}");
        assert_eq!(std::fs::read(root.join("config.json")).unwrap(), before);
    }
    assert!(run_cli(&["config", "set", "user_timezone", "UTC"])
        .status
        .success());
    assert!(run_cli(&["config", "set", "security", "null"])
        .status
        .success());
    assert!(run_cli(&[
        "config",
        "set",
        "future_extension",
        r#"{"tools":{"custom":true}}"#
    ])
    .status
    .success());
    let config = rua_config::load_config(&root.join("config.json"), &root).unwrap();
    assert_eq!(config.user_timezone.as_deref(), Some("UTC"));
    assert_eq!(config.extra["future_extension"]["tools"]["custom"], true);
}
