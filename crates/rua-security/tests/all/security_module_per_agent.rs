use std::sync::Arc;

use rua_security::{SecurityConfigStore, SecurityModule, SecurityVerdict, ToolCallContext};

fn make_module(dir: &std::path::Path) -> Arc<SecurityModule> {
    Arc::new(SecurityModule::new(
        Arc::new(SecurityConfigStore::new(dir.to_path_buf())),
        None,
    ))
}

fn write_agent_security(dir: &std::path::Path, agent_id: &str, deny_echo: bool) {
    let ws = dir.join("workspaces").join(agent_id);
    std::fs::create_dir_all(&ws).unwrap();
    let custom_rules = if deny_echo {
        serde_json::json!([{
            "id": "CUSTOM_DENY_ECHO",
            "tools": ["bash"],
            "category": "command_injection",
            "severity": "HIGH",
            "action": "Deny",
            "command_set": ["echo"]
        }])
    } else {
        serde_json::json!([])
    };
    let cfg = serde_json::json!({
        "tool_guard": {
            "approval_mode": "hands_off",
            "custom_rules": custom_rules
        }
    });
    std::fs::write(
        ws.join("security.json"),
        serde_json::to_string_pretty(&cfg).unwrap(),
    )
    .unwrap();
}

#[tokio::test]
async fn cross_agent_isolation_different_safety_rules() {
    let tmp = tempfile::tempdir().unwrap();

    write_agent_security(tmp.path(), "agent-a", true);
    write_agent_security(tmp.path(), "agent-b", false);

    let module = make_module(tmp.path());

    let ctx_a = ToolCallContext::new(
        "agent-a",
        "s1",
        "bash",
        serde_json::json!({"command": "echo blocked"}),
    );
    let ctx_b = ToolCallContext::new("agent-b", "s2", "read", serde_json::json!({}));

    let va = module.enforce(&ctx_a, &[]).await;
    assert!(
        matches!(va, SecurityVerdict::Deny { .. }),
        "agent-a should deny Bash, got {va:?}"
    );

    let vb = module.enforce(&ctx_b, &[]).await;
    assert!(
        matches!(vb, SecurityVerdict::Allow),
        "agent-b should allow Read, got {vb:?}"
    );
}

#[tokio::test]
async fn invalidate_rebuilds_single_agent_engine() {
    let tmp = tempfile::tempdir().unwrap();

    let module = make_module(tmp.path());
    let ctx_a = ToolCallContext::new("agent-a", "s1", "read", serde_json::json!({}));
    let ctx_b = ToolCallContext::new("agent-b", "s2", "read", serde_json::json!({}));

    let va = module.enforce(&ctx_a, &[]).await;
    assert!(matches!(va, SecurityVerdict::Allow));
    let vb = module.enforce(&ctx_b, &[]).await;
    assert!(matches!(vb, SecurityVerdict::Allow));

    write_agent_security(tmp.path(), "agent-a", true);

    module.invalidate("agent-a");

    let ctx_a = ToolCallContext::new(
        "agent-a",
        "s1",
        "bash",
        serde_json::json!({"command": "echo blocked"}),
    );

    let va2 = module.enforce(&ctx_a, &[]).await;
    assert!(
        matches!(va2, SecurityVerdict::Deny { .. }),
        "agent-a should deny Read after invalidate, got {va2:?}"
    );

    let vb2 = module.enforce(&ctx_b, &[]).await;
    assert!(
        matches!(vb2, SecurityVerdict::Allow),
        "agent-b cache should be untouched, got {vb2:?}"
    );
}

#[tokio::test]
async fn config_store_accessor_returns_working_store() {
    let tmp = tempfile::tempdir().unwrap();
    let module = make_module(tmp.path());

    let store = module.config_store();
    let cfg = store.load("test-agent").unwrap();

    assert!(cfg.tool_guard.enabled);
}
