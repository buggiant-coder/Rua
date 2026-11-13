use rua_config::{load_config, save_config, RootConfig};
use serde_json::json;

#[test]
fn fresh_root_contains_only_global_configuration() {
    let value = serde_json::to_value(RootConfig::default()).unwrap();
    for key in [
        "channels",
        "mcp",
        "tools",
        "acp",
        "last_dispatch",
        "show_tool_details",
    ] {
        assert!(value.get(key).is_none(), "Agent field at root: {key}");
    }
    for key in [
        "defaults",
        "running",
        "language",
        "system_prompt_files",
        "heartbeat",
        "active_hours",
    ] {
        assert!(
            value["agents"].get(key).is_none(),
            "Agent default at root: {key}"
        );
    }
}

#[test]
fn root_roundtrip_removes_retired_fields_without_changing_agent_or_extensions() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    let input = json!({
        "tools": {"builtin_tools": {"bash": {"enabled": false}}},
        "channels": {"console": {"enabled": true}}, "mcp": {"clients": {}},
        "acp": {"agents": {}}, "last_dispatch": {"user_id": "old"},
        "show_tool_details": true, "claude_code": {"enabled": true},
        "agents": {"active_agent": "research", "profiles": {},
            "defaults": {"tools": {}}, "running": {"max_iters": 2},
            "language": "en", "heartbeat": {"enabled": true},
            "active_hours": {}, "system_prompt_files": ["old.md"], "future_registry": 7},
        "security": {"tool_guard": {"enabled": false}, "file_guard": {"enabled": false},
            "allow_no_auth_hosts": ["::1"], "skill_scanner": {"mode": "warn"}, "future_security": 3},
        "user_timezone": "Asia/Shanghai", "future_global": {"enabled": true}
    });
    rua_config::atomic_write_json(&path, &input).unwrap();
    let agent_path = dir.path().join("agent.json");
    std::fs::write(&agent_path, b"unchanged Agent configuration").unwrap();
    let config = load_config(&path, dir.path()).unwrap();
    save_config(&config, &path).unwrap();
    let output: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    for key in [
        "tools",
        "channels",
        "mcp",
        "acp",
        "last_dispatch",
        "show_tool_details",
        "claude_code",
    ] {
        assert!(output.get(key).is_none(), "retired field survived: {key}");
    }
    assert_eq!(output["agents"]["active_agent"], "research");
    assert_eq!(output["agents"]["future_registry"], 7);
    assert!(output["agents"].get("defaults").is_none());
    assert_eq!(output["future_global"], input["future_global"]);
    assert_eq!(output["user_timezone"], input["user_timezone"]);
    assert!(output["security"].get("tool_guard").is_none());
    assert!(output["security"].get("file_guard").is_none());
    assert_eq!(output["security"]["allow_no_auth_hosts"], json!(["::1"]));
    assert_eq!(output["security"]["future_security"], 3);
    assert_eq!(
        std::fs::read(agent_path).unwrap(),
        b"unchanged Agent configuration"
    );
}
