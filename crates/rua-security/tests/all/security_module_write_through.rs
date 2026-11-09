use std::sync::Arc;

use rua_config::types::security::{SecurityConfig, ToolGuardRuleConfig};
use rua_security::{SecurityConfigStore, SecurityModule, SecurityVerdict, ToolCallContext};

fn make_module(dir: &std::path::Path) -> Arc<SecurityModule> {
    Arc::new(SecurityModule::new(
        Arc::new(SecurityConfigStore::new(dir.to_path_buf())),
        None,
    ))
}

#[tokio::test]
async fn save_and_invalidate_clears_cached_engine() {
    let tmp = tempfile::tempdir().unwrap();
    let module = make_module(tmp.path());
    let mut initial = SecurityConfig::default();
    initial.tool_guard.approval_mode = "hands_off".into();
    module.config_store().save("agent-1", &initial).unwrap();

    let ctx = ToolCallContext::new(
        "agent-1",
        "session-1",
        "bash",
        serde_json::json!({"command": "echo blocked"}),
    );

    let v = module.enforce(&ctx, &[]).await;
    assert!(matches!(v, SecurityVerdict::Allow));

    let mut cfg = SecurityConfig::default();
    cfg.tool_guard.approval_mode = "hands_off".into();
    cfg.tool_guard.custom_rules = vec![custom_command_rule(
        "CUSTOM_DENY_ECHO",
        "Deny",
        vec!["echo"],
    )];

    module.save_and_invalidate("agent-1", &cfg).unwrap();

    let v2 = module.enforce(&ctx, &[]).await;
    assert!(
        matches!(v2, SecurityVerdict::Deny { .. }),
        "engine should deny echo after save_and_invalidate, got {v2:?}"
    );
}

#[tokio::test]
async fn save_and_invalidate_persists_to_disk() {
    let tmp = tempfile::tempdir().unwrap();
    let module = make_module(tmp.path());

    let mut cfg = SecurityConfig::default();
    cfg.tool_guard.approval_mode = "hands_off".into();
    module.save_and_invalidate("agent-x", &cfg).unwrap();

    let store2 = SecurityConfigStore::new(tmp.path().to_path_buf());
    let loaded = store2.load("agent-x").unwrap();
    assert_eq!(loaded.tool_guard.approval_mode, "hands_off");
}

fn custom_command_rule(id: &str, action: &str, command_set: Vec<&str>) -> ToolGuardRuleConfig {
    serde_json::from_value(serde_json::json!({
        "id": id,
        "tools": ["bash"],
        "category": "command_injection",
        "severity": "HIGH",
        "action": action,
        "command_set": command_set
    }))
    .unwrap()
}
