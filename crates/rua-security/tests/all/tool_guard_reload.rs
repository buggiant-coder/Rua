use std::sync::Arc;

use rua_config::types::security::{SecurityConfig, ToolGuardRuleConfig};
use rua_security::models::ToolCallContext;
use rua_security::{GuardDecision, ToolGuardEngine};

fn make_engine(config: &SecurityConfig) -> Arc<std::sync::RwLock<ToolGuardEngine>> {
    Arc::new(std::sync::RwLock::new(ToolGuardEngine::new(config, None)))
}

fn bash_ctx(command: &str) -> ToolCallContext {
    ToolCallContext::new(
        "agent",
        "sess",
        "bash",
        serde_json::json!({"command": command}),
    )
}

#[tokio::test]
async fn reload_updates_enabled_flag() {
    let config = SecurityConfig::default();
    let engine = make_engine(&config);
    assert!(engine.read().unwrap().is_enabled());

    let mut disabled = SecurityConfig::default();
    disabled.tool_guard.enabled = false;
    *engine.write().unwrap() = ToolGuardEngine::new(&disabled, None);
    assert!(!engine.read().unwrap().is_enabled());

    *engine.write().unwrap() = ToolGuardEngine::new(&SecurityConfig::default(), None);
    assert!(engine.read().unwrap().is_enabled());
}

#[tokio::test]
async fn evaluate_reflects_reloaded_rules() {
    let config = SecurityConfig::default();
    let engine = make_engine(&config);

    let ctx = bash_ctx("ls");
    let result = engine.read().unwrap().evaluate(&ctx);
    assert!(!matches!(
        result.decision,
        rua_security::GuardDecision::Deny
    ));

    let mut new_config = SecurityConfig::default();
    new_config.tool_guard.custom_rules =
        vec![custom_command_rule("CUSTOM_DENY_LS", "Deny", vec!["ls"])];
    *engine.write().unwrap() = ToolGuardEngine::new(&new_config, None);

    let result = engine.read().unwrap().evaluate(&ctx);
    assert!(matches!(result.decision, GuardDecision::Deny));
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
