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

const READ_PATH: &str = "/fixture/note.txt";

fn device_read(local: &str, target: Option<&str>, call_id: &str) -> ToolCallContext {
    let mut args = serde_json::json!({"file_path": READ_PATH});
    if let Some(device) = target {
        args["device_id"] = device.into();
    }
    let mut ctx = ToolCallContext::new("agent", "chat", "read", args)
        .with_chat_address(rua_core::ChatAddress::new("chat"));
    ctx.local_device_id = Some(local.to_owned());
    ctx.metadata.insert("turn_id".into(), "turn".into());
    ctx.metadata.insert("tool_call_id".into(), call_id.into());
    ctx
}

#[tokio::test]
async fn device_read_allow_chat_recognizes_the_phone_after_restart() {
    use rua_security::security_module::FileReadAdmission::{Allowed, RequiresUserConfirmation};
    use rua_security::{ApprovalDecision, ApprovalReuseScope};
    let tmp = tempfile::tempdir().unwrap();
    let mut config = SecurityConfig::default();
    config.tool_guard.approval_mode = "ask_first".into();
    {
        let module = make_module(tmp.path());
        module.save_and_invalidate("agent", &config).unwrap();
        let original =
            device_read("phone", None, "first").with_security_snapshot(Arc::new(config.clone()));
        let SecurityVerdict::RequiresApproval { request_id, .. } =
            module.enforce(&original, &[]).await
        else {
            panic!("expected an Ask First card for the phone file")
        };
        let authority = module.approval_authority();
        let card = authority.request(&request_id).await.unwrap();
        let option = card
            .reuse_options
            .iter()
            .find(|o| o.scope == ApprovalReuseScope::Chat)
            .unwrap();
        authority
            .decide_for_authenticated_user(
                "agent",
                &request_id,
                ApprovalDecision::AllowChat {
                    decided_by: "webui".into(),
                    matcher: option.matcher.clone(),
                },
                "user",
            )
            .await
            .unwrap();
    }
    let reopened = make_module(tmp.path());
    for (local, target, expected) in [
        ("phone", None, Allowed),
        ("phone", Some("phone"), Allowed),
        ("desktop", Some("phone"), Allowed),
        ("desktop", None, RequiresUserConfirmation),
        ("desktop", Some("desktop"), RequiresUserConfirmation),
    ] {
        let call = format!("{local}-{target:?}");
        let ctx =
            device_read(local, target, &call).with_security_snapshot(Arc::new(config.clone()));
        let verdict = reopened.enforce(&ctx, &[]).await;
        assert!(
            matches!(
                (&expected, &verdict),
                (Allowed, SecurityVerdict::Allow)
                    | (
                        RequiresUserConfirmation,
                        SecurityVerdict::RequiresApproval { .. }
                    )
            ),
            "{call}: {verdict:?}"
        );
        if target == Some(local) {
            assert_eq!(
                reopened
                    .inspect_chat_file_read(&ctx, &config, &[READ_PATH])
                    .await
                    .unwrap(),
                expected,
                "{call}"
            );
        }
    }
}

#[tokio::test]
async fn device_read_manual_deny_coexists_with_the_identical_legacy_allow_after_restart() {
    use rua_security::{ApprovalReuseMatcher, PermissionRuleEffect, PermissionRuleInput};
    let tmp = tempfile::tempdir().unwrap();
    let module = make_module(tmp.path());
    let config = legacy_path_only_allow();
    module.save_and_invalidate("agent", &config).unwrap();
    let implicit = device_read("phone", None, "implicit");
    let deny = module
        .approval_authority()
        .create_agent_rule(
            "agent",
            PermissionRuleInput {
                effect: PermissionRuleEffect::Deny,
                enabled: true,
                label: "Local fixture".into(),
                matcher: ApprovalReuseMatcher::ToolCall {
                    tool_name: "read".into(),
                    input_params: Some(implicit.tool_input.to_string()),
                },
            },
        )
        .await
        .unwrap();
    drop(module);
    let module = make_module(tmp.path());
    let rules = module.approval_authority().list_agent_rules("agent").await;
    assert_eq!(rules.len(), 2);
    assert!(rules
        .iter()
        .any(|r| r.id == "rule" && r.effect == PermissionRuleEffect::Allow));
    assert!(rules
        .iter()
        .any(|r| r.id == deny.id && r.effect == PermissionRuleEffect::Deny));
    let captured = Arc::new(module.config_store().load("agent").unwrap());
    for call in [implicit, device_read("phone", Some("phone"), "explicit")] {
        let ctx = call.with_security_snapshot(captured.clone());
        let verdict = module.enforce(&ctx, &[]).await;
        assert!(
            matches!(&verdict, SecurityVerdict::Deny { reason } if reason.contains("Local fixture")),
            "{verdict:?}"
        );
    }
}

fn legacy_path_only_allow() -> SecurityConfig {
    let mut config = SecurityConfig::default();
    config.tool_guard.approval_mode = "ask_first".into();
    config.extra.insert(
        "permission_rules".into(),
        legacy_file_approval("allow", READ_PATH),
    );
    config
}

#[tokio::test]
async fn device_read_legacy_path_only_approval_does_not_authorize_another_device() {
    use rua_security::security_module::FileReadAdmission;
    let tmp = tempfile::tempdir().unwrap();
    let module = make_module(tmp.path());
    let config = legacy_path_only_allow();
    module.save_and_invalidate("agent", &config).unwrap();
    let implicit = device_read("desktop", None, "implicit-desktop")
        .with_security_snapshot(Arc::new(config.clone()));
    assert!(module.enforce(&implicit, &[]).await.requires_approval());
    let ctx = device_read("tablet", Some("tablet"), "tablet")
        .with_security_snapshot(Arc::new(config.clone()));
    assert!(module.enforce(&ctx, &[]).await.requires_approval());
    assert_eq!(
        module
            .inspect_chat_file_read(&ctx, &config, &[READ_PATH])
            .await
            .unwrap(),
        FileReadAdmission::RequiresUserConfirmation
    );
}

#[tokio::test]
async fn device_read_legacy_approval_uses_the_matcher_selected_in_settings_after_restart() {
    use rua_security::security_module::FileReadAdmission::Allowed;
    use rua_security::{ApprovalReuseMatcher, PermissionRuleEffect, PermissionRuleInput};
    let other_path = "/fixture/another-note.txt";
    for (tool, input_params, device_arg) in [
        (
            "read",
            Some(serde_json::json!({"file_path":other_path}).to_string()),
            None,
        ),
        ("read", None, Some("tablet")),
        ("rea*", None, Some("tablet")),
    ] {
        let path_limited = input_params.is_some();
        let tmp = tempfile::tempdir().unwrap();
        let module = make_module(tmp.path());
        module
            .save_and_invalidate("agent", &legacy_path_only_allow())
            .unwrap();
        module
            .approval_authority()
            .update_agent_rule(
                "agent",
                "rule",
                PermissionRuleInput {
                    effect: PermissionRuleEffect::Allow,
                    enabled: true,
                    label: "Updated in settings".into(),
                    matcher: ApprovalReuseMatcher::ToolCall {
                        tool_name: tool.into(),
                        input_params,
                    },
                },
            )
            .await
            .unwrap()
            .unwrap();
        drop(module);
        let reopened = make_module(tmp.path());
        let mut ctx = device_read("tablet", device_arg, "updated");
        ctx.tool_input["file_path"] = other_path.into();
        let verdict = reopened.enforce(&ctx, &[]).await;
        assert!(
            verdict.is_allow(),
            "{tool} with {device_arg:?}: {verdict:?}"
        );
        if device_arg == ctx.local_device_id.as_deref() {
            let captured = reopened.config_store().load("agent").unwrap();
            assert_eq!(
                reopened
                    .inspect_chat_file_read(&ctx, &captured, &[other_path])
                    .await
                    .unwrap(),
                Allowed
            );
        }
        if path_limited {
            ctx.tool_input["file_path"] = READ_PATH.into();
            assert!(reopened.enforce(&ctx, &[]).await.requires_approval());
        }
        ctx.tool_name = "glob".into();
        assert!(reopened.enforce(&ctx, &[]).await.requires_approval());
    }
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
            owner: rua_security::ApprovalOwner::chat(rua_core::ChatAddress::new("sess-1")),
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
        .snapshot_at(&rua_core::ChatAddress::new("sess-1"))
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
    .with_chat_address(rua_core::ChatAddress::new("chat-one"))
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
    .with_chat_address(rua_core::ChatAddress::new("chat-two"))
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
    .with_chat_address(rua_core::ChatAddress::new("chat"))
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
    let other = ctx.with_chat_address(rua_core::ChatAddress::new("other"));
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
        rua_security::models::EFFECTIVE_SECURITY_CONFIG_METADATA_KEY.into(),
        serde_json::json!({"tool_guard": {}}),
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
        .with_chat_address(rua_core::ChatAddress::new("chat"))
        .with_security_snapshot(Arc::new(SecurityConfig::default()));
    let path = tmp.path().join("chats/sessions/chat/permission_rules.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, "invalid-json").unwrap();
    assert!(matches!(
        module.enforce(&ctx, &[]).await,
        SecurityVerdict::Deny { .. }
    ));
}

#[tokio::test]
async fn chat_rule_replacement_and_clear_take_effect_for_an_already_running_module() {
    use rua_core::ChatAddress;
    use rua_security::{ApprovalDecision, ApprovalReuseScope, PermissionRuleEffect};

    let tmp = tempfile::tempdir().unwrap();
    let module = make_module(tmp.path());
    let mut config = SecurityConfig::default();
    config.tool_guard.approval_mode = "ask_first".into();
    module.save_and_invalidate("agent", &config).unwrap();
    let address = ChatAddress::new("chat");
    let ctx = ToolCallContext::new("agent", "chat", "calendar_list", serde_json::json!({}))
        .with_chat_address(address.clone());
    let captured = ctx.clone().with_security_snapshot(Arc::new(config));
    let SecurityVerdict::RequiresApproval { request_id, .. } = module.enforce(&ctx, &[]).await
    else {
        panic!("expected an approval card");
    };
    let authority = module.approval_authority();
    let card = authority.request(&request_id).await.unwrap();
    let matcher = card
        .reuse_options
        .iter()
        .find(|option| option.scope == ApprovalReuseScope::Chat)
        .unwrap()
        .matcher
        .clone();
    authority
        .decide_for_authenticated_user(
            "agent",
            &request_id,
            ApprovalDecision::AllowChat {
                decided_by: "webui".into(),
                matcher,
            },
            "user",
        )
        .await
        .unwrap();
    for call in [&ctx, &captured] {
        assert!(module.enforce(call, &[]).await.is_allow());
    }

    let path = tmp.path().join("chats/sessions/chat/permission_rules.json");
    let mut stored: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    stored[0]["effect"] = serde_json::json!("deny");
    rua_config::atomic_write_json(&path, &stored).unwrap();
    assert_eq!(
        authority.list_chat_rules(&address).await[0].effect,
        PermissionRuleEffect::Deny
    );
    for call in [&ctx, &captured] {
        assert!(matches!(
            module.enforce(call, &[]).await,
            SecurityVerdict::Deny { .. }
        ));
    }
    // A lookup must never write a cached row or a hit count back over replicated policy.
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&std::fs::read(&path).unwrap()).unwrap(),
        stored
    );

    rua_config::atomic_write_json(&path, &serde_json::json!([])).unwrap();
    assert!(authority.list_chat_rules(&address).await.is_empty());
    for call in [&ctx, &captured] {
        assert!(matches!(
            module.enforce(call, &[]).await,
            SecurityVerdict::RequiresApproval { .. }
        ));
    }
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
        .with_chat_address(rua_core::ChatAddress::new("chat"))
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

fn legacy_file_approval(effect: &str, path: &str) -> serde_json::Value {
    serde_json::json!([{
        "id": "rule", "label": "Reports", "source": "approval", "source_approval_id": "original",
        "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-02T00:00:00Z",
        "effect": effect, "hit_count": 0, "matcher": {
            "type": "tool_call", "tool_name": "read",
            "input_params": serde_json::json!({"file_path": path}).to_string()
        }
    }])
}

fn write_legacy_approval_deny(dir: &std::path::Path) {
    let mut config = SecurityConfig::default();
    config.extra.insert(
        "permission_rules".into(),
        legacy_file_approval("deny", "/fixture/report-*.txt"),
    );
    write_agent_security(dir, "agent", config);
}

#[tokio::test]
async fn legacy_approval_deny_keeps_its_file_pattern_after_reload() {
    let tmp = tempfile::tempdir().unwrap();
    write_legacy_approval_deny(tmp.path());
    let module = make_module(tmp.path());
    for (path, expected) in [
        ("/fixture/report-1.txt", Some("rule")),
        ("/fixture/note.txt", None),
    ] {
        let args = serde_json::json!({"file_path": path});
        let denied = module
            .approval_authority()
            .find_deny_rule("agent", "read", &args)
            .await;
        assert_eq!(
            denied.as_ref().map(|rule| rule.id.as_str()),
            expected,
            "{path}"
        );
    }
}

#[tokio::test]
async fn legacy_approval_origin_survives_a_matcher_edit_and_reload() {
    let tmp = tempfile::tempdir().unwrap();
    write_legacy_approval_deny(tmp.path());
    let module = make_module(tmp.path());
    let args = serde_json::json!({"file_path": "/fixture/one.txt"});
    let input = rua_security::PermissionRuleInput {
        effect: rua_security::PermissionRuleEffect::Deny,
        enabled: true,
        label: "One report".into(),
        matcher: rua_security::ApprovalReuseMatcher::ToolCall {
            tool_name: "read".into(),
            input_params: Some(args.to_string()),
        },
    };
    module
        .approval_authority()
        .update_agent_rule("agent", "rule", input)
        .await
        .unwrap()
        .unwrap();
    let reopened = make_module(tmp.path());
    let rule = reopened
        .approval_authority()
        .find_deny_rule("agent", "read", &args)
        .await
        .expect("edited matcher persisted");
    assert_eq!(rule.source, rua_security::PermissionRuleSource::Approval);
    assert_eq!(rule.source_approval_id, "original");
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

#[tokio::test]
async fn handoff_snapshot_initializes_only_missing_chat_rules() {
    let temp = tempfile::tempdir().unwrap();
    let module = make_module(temp.path());
    let address = rua_core::ChatAddress::new("handoff-empty");
    let path = rua_core::ChatStore::resolve(temp.path(), address.clone())
        .root()
        .join("sessions/handoff-empty/permission_rules.json");
    module
        .approval_authority()
        .ensure_chat_rules_snapshot(&address)
        .await
        .unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"[]");
    let rule = rua_security::approval_authority::StoredApprovalReuseRule {
        id: "already-approved".into(),
        label: "Own file".into(),
        matcher: rua_security::ApprovalReuseMatcher::ToolCall {
            tool_name: "read".into(),
            input_params: Some(r#"{"file_path":"/photo.jpg"}"#.into()),
        },
        exact_read_args: Some(true),
        source_approval_id: "original-card".into(),
        created_at: chrono::Utc::now(),
        updated_at: None,
        enabled: true,
        effect: rua_security::PermissionRuleEffect::Allow,
        source: rua_security::PermissionRuleSource::Approval,
        hit_count: 0,
    };
    let original = serde_json::to_vec(&vec![rule]).unwrap();
    rua_config::atomic_write(&path, &original).unwrap();
    module
        .approval_authority()
        .ensure_chat_rules_snapshot(&address)
        .await
        .unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert_eq!(
        module
            .approval_authority()
            .list_chat_rules(&address)
            .await
            .len(),
        1
    );
}
