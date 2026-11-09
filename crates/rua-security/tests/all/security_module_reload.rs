use std::sync::Arc;

use rua_config::types::security::{SecurityConfig, ToolGuardConfig};
use rua_security::{
    ApprovalRequestInput, ApprovalTrigger, SecurityConfigStore, SecurityModule, SecurityVerdict,
    ToolCallContext, ToolCallSnapshot,
};

fn make_module(dir: &std::path::Path) -> Arc<SecurityModule> {
    Arc::new(SecurityModule::new(
        Arc::new(SecurityConfigStore::new(dir.to_path_buf())),
        None,
    ))
}

fn write_agent_security_config(dir: &std::path::Path, agent_id: &str) {
    let ws_dir = dir.join("workspaces").join(agent_id);
    std::fs::create_dir_all(&ws_dir).unwrap();
    let config_path = ws_dir.join("security.json");
    let config = serde_json::json!({
        "tool_guard": {
            "approval_mode": "hands_off",
            "custom_rules": [{
                "id": "CUSTOM_DENY_ECHO",
                "tools": ["bash"],
                "category": "command_injection",
                "severity": "HIGH",
                "action": "Deny",
                "command_set": ["echo"]
            }]
        }
    });
    std::fs::write(&config_path, serde_json::to_string_pretty(&config).unwrap()).unwrap();
}

fn write_agent_security(dir: &std::path::Path, agent_id: &str, config: SecurityConfig) {
    let ws_dir = dir.join("workspaces").join(agent_id);
    std::fs::create_dir_all(&ws_dir).unwrap();
    let config_path = ws_dir.join("security.json");
    std::fs::write(&config_path, serde_json::to_string_pretty(&config).unwrap()).unwrap();
}

#[tokio::test]
async fn invalidate_clears_cached_engine() {
    let tmp = tempfile::tempdir().unwrap();
    let module = make_module(tmp.path());
    let mut initial = SecurityConfig::default();
    initial.tool_guard.approval_mode = "hands_off".into();
    write_agent_security(tmp.path(), "agent-1", initial);

    let ctx = ToolCallContext::new(
        "agent-1",
        "session-1",
        "bash",
        serde_json::json!({"command": "echo blocked"}),
    );
    let v = module.enforce(&ctx, &[]).await;
    assert!(matches!(v, SecurityVerdict::Allow));

    write_agent_security_config(tmp.path(), "agent-1");

    let v_cached = module.enforce(&ctx, &[]).await;
    assert!(
        matches!(v_cached, SecurityVerdict::Allow),
        "cached engine should not see new config yet"
    );

    module.invalidate("agent-1");

    let v_fresh = module.enforce(&ctx, &[]).await;
    assert!(
        matches!(v_fresh, SecurityVerdict::Deny { .. }),
        "fresh engine should deny echo after invalidate, got {v_fresh:?}"
    );
}

#[tokio::test]
async fn invalidate_does_not_affect_active_authority_requests() {
    let tmp = tempfile::tempdir().unwrap();
    let module = make_module(tmp.path());

    let request = module
        .approval_authority()
        .create_request(ApprovalRequestInput {
            agent_id: "agent-1".into(),
            owner: rua_security::ApprovalOwner::chat(rua_core::ChatAddress::agent(
                "agent-1", "sess-1",
            )),
            turn_id: "turn-1".into(),
            tool_call_id: "call-1".into(),
            tool_name: "bash".into(),
            trigger: ApprovalTrigger::approval_mode("Balanced requires approval"),
            tool_call: ToolCallSnapshot {
                display: "Bash echo hi".into(),
                args: serde_json::json!({"cmd": "echo hi"}),
            },
        })
        .await;

    module.invalidate("agent-1");

    let snapshot = module
        .approval_authority()
        .snapshot_at(&rua_core::ChatAddress::agent("agent-1", "sess-1"))
        .await;
    assert_eq!(snapshot.pending.len(), 1);
    assert_eq!(snapshot.pending[0].id, request.id);
}

#[tokio::test]
async fn invalidate_all_clears_every_agent() {
    let tmp = tempfile::tempdir().unwrap();
    let module = make_module(tmp.path());

    let ctx_a = ToolCallContext::new("agent-a", "s1", "read", serde_json::json!({}));
    let ctx_b = ToolCallContext::new("agent-b", "s2", "read", serde_json::json!({}));
    module.enforce(&ctx_a, &[]).await;
    module.enforce(&ctx_b, &[]).await;

    module.invalidate_all();

    let va = module.enforce(&ctx_a, &[]).await;
    let vb = module.enforce(&ctx_b, &[]).await;
    assert!(matches!(va, SecurityVerdict::Allow));
    assert!(matches!(vb, SecurityVerdict::Allow));
}

#[tokio::test]
async fn chat_security_snapshot_isolated_from_mutable_agent_defaults() {
    let tmp = tempfile::tempdir().unwrap();
    let module = make_module(tmp.path());
    let mut restricted = SecurityConfig::default();
    restricted.tool_guard.approval_mode = "ask_first".into();
    let restricted = Arc::new(restricted);
    let allowed = Arc::new(SecurityConfig::default());
    let ctx = ToolCallContext::new(
        "same-agent",
        "chat-one",
        "read",
        serde_json::json!({"file_path": "/tmp/readme"}),
    )
    .with_chat_address(rua_core::ChatAddress::agent("same-agent", "chat-one"))
    .with_security_snapshot(restricted.clone());
    module.config_store().save("same-agent", &allowed).unwrap();
    assert!(matches!(
        module.enforce(&ctx, &[]).await,
        SecurityVerdict::RequiresApproval { .. }
    ));
    module
        .config_store()
        .save("same-agent", &restricted)
        .unwrap();
    let other = ToolCallContext::new(
        "same-agent",
        "chat-two",
        "read",
        serde_json::json!({"file_path": "/tmp/readme"}),
    )
    .with_chat_address(rua_core::ChatAddress::agent("same-agent", "chat-two"))
    .with_security_snapshot(allowed);
    assert!(matches!(
        module.enforce(&other, &[]).await,
        SecurityVerdict::Allow
    ));
    module.config_store().delete("same-agent").unwrap();
    assert!(matches!(
        module.enforce(&ctx, &[]).await,
        SecurityVerdict::RequiresApproval { .. }
    ));
}

#[tokio::test]
async fn approved_invocation_recheck_is_scoped_to_exact_call_and_arguments() {
    let tmp = tempfile::tempdir().unwrap();
    let module = make_module(tmp.path());
    let mut config = SecurityConfig::default();
    config.tool_guard.approval_mode = "ask_first".into();
    let mut ctx = ToolCallContext::new(
        "agent",
        "chat",
        "read",
        serde_json::json!({"file_path": "/tmp/a"}),
    )
    .with_chat_address(rua_core::ChatAddress::agent("agent", "chat"))
    .with_security_snapshot(Arc::new(config));
    ctx.metadata
        .insert("turn_id".into(), serde_json::json!("turn"));
    ctx.metadata
        .insert("tool_call_id".into(), serde_json::json!("call"));
    let SecurityVerdict::RequiresApproval { request_id, .. } = module.enforce(&ctx, &[]).await
    else {
        panic!("approval required")
    };
    module
        .approval_authority()
        .decide(
            &request_id,
            rua_security::ApprovalDecision::RunOnce {
                decided_by: "user".into(),
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        module.enforce(&ctx, &[]).await,
        SecurityVerdict::Allow
    ));
    let mut changed = ctx.clone();
    changed.tool_input = serde_json::json!({"file_path": "/tmp/b"});
    assert!(matches!(
        module.enforce(&changed, &[]).await,
        SecurityVerdict::RequiresApproval { .. }
    ));
    let other = ctx.with_chat_address(rua_core::ChatAddress::agent("agent", "other"));
    assert!(matches!(
        module.enforce(&other, &[]).await,
        SecurityVerdict::RequiresApproval { .. }
    ));
}

#[tokio::test]
async fn missing_snapshot_and_malformed_permission_rules_fail_closed() {
    let tmp = tempfile::tempdir().unwrap();
    let module = make_module(tmp.path());
    let mut ctx = ToolCallContext::new("agent", "chat", "read", serde_json::json!({}));
    ctx.metadata.insert(
        "chat_configuration_epoch".into(),
        serde_json::json!("epoch"),
    );
    assert!(matches!(
        module.enforce(&ctx, &[]).await,
        SecurityVerdict::Deny { .. }
    ));
    assert!(module.hard_deny_reason(&ctx).await.is_some());
    let mut config = SecurityConfig::default();
    config.extra.insert(
        "permission_rules".into(),
        serde_json::json!({"malformed": true}),
    );
    ctx.security_snapshot = Some(Arc::new(config));
    assert!(matches!(
        module.enforce(&ctx, &[]).await,
        SecurityVerdict::Deny { .. }
    ));
    assert!(module.hard_deny_reason(&ctx).await.is_some());
}

#[tokio::test]
async fn corrupted_chat_rules_fail_closed_before_approval_reuse() {
    let tmp = tempfile::tempdir().unwrap();
    let module = make_module(tmp.path());
    let ctx = ToolCallContext::new("agent", "chat", "read", serde_json::json!({}))
        .with_chat_address(rua_core::ChatAddress::agent("agent", "chat"))
        .with_security_snapshot(Arc::new(SecurityConfig::default()));
    let path = tmp
        .path()
        .join("workspaces/agent/sessions/chat/permission_rules.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, "invalid-json").unwrap();
    assert!(matches!(
        module.enforce(&ctx, &[]).await,
        SecurityVerdict::Deny { .. }
    ));
}

#[tokio::test]
async fn snapshot_permission_rules_do_not_follow_agent_rule_updates() {
    let tmp = tempfile::tempdir().unwrap();
    let module = make_module(tmp.path());
    let rule = |effect: &str| {
        serde_json::json!({
            "id": "snapshot-rule", "label": "snapshot rule", "matcher": {
                "type": "tool_call", "tool_name": "read", "input_params": null
            },
            "source_approval_id": "", "created_at": "2026-01-01T00:00:00Z",
            "effect": effect, "hit_count": 0
        })
    };
    let mut snapshot = SecurityConfig::default();
    snapshot.tool_guard.approval_mode = "ask_first".into();
    snapshot.extra.insert(
        "permission_rules".into(),
        serde_json::json!([rule("allow")]),
    );
    let mut defaults = snapshot.clone();
    defaults
        .extra
        .insert("permission_rules".into(), serde_json::json!([rule("deny")]));
    module.config_store().save("agent", &defaults).unwrap();
    let ctx = ToolCallContext::new("agent", "chat", "read", serde_json::json!({}))
        .with_chat_address(rua_core::ChatAddress::agent("agent", "chat"))
        .with_security_snapshot(Arc::new(snapshot));
    assert!(matches!(
        module.enforce(&ctx, &[]).await,
        SecurityVerdict::Allow
    ));
    let denied = ctx.with_security_snapshot(Arc::new(defaults));
    assert!(matches!(
        module.enforce(&denied, &[]).await,
        SecurityVerdict::Deny { .. }
    ));
}

#[tokio::test]
async fn ask_first_mode_requires_approval_for_any_tool_call() {
    let tmp = tempfile::tempdir().unwrap();
    let module = make_module(tmp.path());
    let mut config = SecurityConfig::default();
    config.tool_guard.approval_mode = "ask_first".into();
    write_agent_security(tmp.path(), "agent-ask-first", config);

    let ctx = ToolCallContext::new(
        "agent-ask-first",
        "session-1",
        "read",
        serde_json::json!({"path": "/tmp/plain.txt"}),
    );

    let verdict = module.enforce(&ctx, &[]).await;

    assert!(
        matches!(verdict, SecurityVerdict::RequiresApproval { .. }),
        "Ask First mode should require approval before ordinary tool calls, got {verdict:?}"
    );
}

#[tokio::test]
async fn hands_off_mode_respects_disabled_builtin_rules() {
    let tmp = tempfile::tempdir().unwrap();
    let module = make_module(tmp.path());
    let config = SecurityConfig {
        tool_guard: ToolGuardConfig {
            approval_mode: "hands_off".into(),
            disabled_rules: vec!["TOOL_CMD_DANGEROUS_RM".into()],
            ..ToolGuardConfig::default()
        },
        ..SecurityConfig::default()
    };
    write_agent_security(tmp.path(), "agent-rules", config.clone());

    let ctx = ToolCallContext::new(
        "agent-rules",
        "session-1",
        "bash",
        serde_json::json!({"command": "rm -rf /tmp/rua-not-sensitive"}),
    );

    let custom_verdict = module.enforce(&ctx, &[]).await;
    assert!(
        matches!(custom_verdict, SecurityVerdict::Allow),
        "Hands-off mode should respect disabled built-in rules, got {custom_verdict:?}"
    );
}
