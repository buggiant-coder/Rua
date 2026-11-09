use rua_config::types::acp::{default_builtin_runners, AcpConfig, ToolParseMode};

#[test]
fn empty_config_backfill_produces_four_runners() {
    let mut config = AcpConfig::default();
    assert!(config.agents.is_empty());

    config.backfill_builtin_runners();
    assert_eq!(config.agents.len(), 4);

    let oc = &config.agents["opencode"];
    assert_eq!(oc.command, "opencode");
    assert_eq!(oc.args, vec!["acp"]);
    assert!(oc.enabled);
    assert!(!oc.trusted);

    let qw = &config.agents["qwen"];
    assert_eq!(qw.command, "qwen");
    assert_eq!(qw.args, vec!["--acp"]);

    let cc = &config.agents["claude"];
    assert_eq!(cc.command, "claude-agent-acp");
    assert!(cc.args.is_empty());

    let cx = &config.agents["codex"];
    assert_eq!(cx.command, "codex-acp");
    assert!(cx.args.is_empty());
}

#[test]
fn user_override_preserved_after_backfill() {
    let json = serde_json::json!({
        "agents": {
            "opencode": {
                "enabled": false,
                "command": "/custom/opencode",
                "args": ["--custom"],
                "trusted": false
            }
        }
    });
    let mut config: AcpConfig = serde_json::from_value(json).unwrap();
    config.backfill_builtin_runners();

    assert_eq!(config.agents.len(), 4);

    let oc = &config.agents["opencode"];
    assert!(!oc.enabled);
    assert_eq!(oc.command, "/custom/opencode");
    assert_eq!(oc.args, vec!["--custom"]);
    assert!(!oc.trusted);

    assert!(config.agents.contains_key("qwen"));
    assert!(config.agents.contains_key("claude"));
    assert!(config.agents.contains_key("codex"));
}

#[test]
fn empty_builtin_command_is_repaired_after_backfill() {
    let json = serde_json::json!({
        "agents": {
            "codex": {
                "enabled": false,
                "command": "",
                "args": [],
                "trusted": false
            }
        }
    });
    let mut config: AcpConfig = serde_json::from_value(json).unwrap();
    config.backfill_builtin_runners();

    let codex = &config.agents["codex"];
    assert!(!codex.enabled);
    assert_eq!(codex.command, "codex-acp");
    assert!(codex.args.is_empty());
    assert!(!codex.trusted);
}

#[test]
fn custom_runner_coexists_with_builtins() {
    let json = serde_json::json!({
        "agents": {
            "my_runner": {
                "enabled": true,
                "command": "my-agent",
                "args": ["serve"],
                "trusted": false
            }
        }
    });
    let mut config: AcpConfig = serde_json::from_value(json).unwrap();
    config.backfill_builtin_runners();

    assert_eq!(config.agents.len(), 5);
    assert!(config.agents.contains_key("my_runner"));
    assert_eq!(config.agents["my_runner"].command, "my-agent");

    for name in &["opencode", "qwen", "claude", "codex"] {
        assert!(config.agents.contains_key(*name));
    }
}

#[test]
fn serde_roundtrip() {
    let mut config = AcpConfig::default();
    config.backfill_builtin_runners();

    let json = serde_json::to_value(&config).unwrap();
    let restored: AcpConfig = serde_json::from_value(json).unwrap();

    assert_eq!(restored.agents.len(), 4);
    for (key, original) in &config.agents {
        let restored_agent = &restored.agents[key];
        assert_eq!(restored_agent.command, original.command);
        assert_eq!(restored_agent.args, original.args);
        assert_eq!(restored_agent.enabled, original.enabled);
        assert_eq!(restored_agent.trusted, original.trusted);
        assert_eq!(restored_agent.tool_parse_mode, original.tool_parse_mode);
    }
}

#[test]
fn tool_parse_mode_serde() {
    let json = serde_json::json!({
        "agents": {
            "test": {
                "command": "test",
                "tool_parse_mode": "update_detail"
            }
        }
    });
    let config: AcpConfig = serde_json::from_value(json).unwrap();
    assert_eq!(
        config.agents["test"].tool_parse_mode,
        ToolParseMode::UpdateDetail
    );
}

#[test]
fn default_builtin_runners_are_enabled_but_fail_closed() {
    let runners = default_builtin_runners();
    for runner in runners.values() {
        assert!(runner.enabled);
        assert!(!runner.trusted);
    }
}

#[test]
fn root_config_with_acp_field() {
    let json = serde_json::json!({
        "acp": {
            "agents": {
                "opencode": {
                    "enabled": true,
                    "command": "opencode",
                    "args": ["acp"]
                }
            }
        }
    });
    let config: rua_config::types::RootConfig = serde_json::from_value(json).unwrap();
    assert_eq!(config.acp.agents.len(), 1);
    assert_eq!(config.acp.agents["opencode"].command, "opencode");
}

#[test]
fn root_config_missing_acp_defaults_empty() {
    let json = serde_json::json!({});
    let config: rua_config::types::RootConfig = serde_json::from_value(json).unwrap();
    assert!(config.acp.agents.is_empty());
}

#[test]
fn root_config_null_acp_defaults_empty() {
    let json = serde_json::json!({"acp": null});
    let config: rua_config::types::RootConfig = serde_json::from_value(json).unwrap();
    assert!(config.acp.agents.is_empty());
}
