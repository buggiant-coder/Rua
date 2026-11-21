use std::sync::Arc;

use rua_core::ChatAddress;
use rua_security::approval_authority::{
    ApprovalDecision, ApprovalRequestInput, ApprovalReuseMatcher, ApprovalReuseScope,
    ApprovalStatus, ApprovalTrigger, PermissionRuleEffect, PermissionRuleInput, ToolCallSnapshot,
};
use rua_security::security_module::{FileReadAdmission, FileReadPolicyError};
use rua_security::{SecurityConfigStore, SecurityModule, ToolCallContext};
use serde_json::json;

fn context() -> ToolCallContext {
    let mut ctx = ToolCallContext::new(
        "agent",
        "chat",
        "bash",
        json!({
            "device_id":"phone", "cwd":"/albums/*-literal",
            "command":"printf '%s' photo*.jpg", "yield_seconds":1,
        }),
    )
    .with_chat_address(ChatAddress::new("chat"));
    ctx.local_device_id = Some("phone".into());
    ctx.metadata.insert("turn_id".into(), "turn".into());
    ctx.metadata.insert("tool_call_id".into(), "command".into());
    ctx
}

fn ask_first(module: &SecurityModule) -> rua_config::types::security::SecurityConfig {
    let mut config = module.config_store().load("agent").unwrap();
    config.tool_guard.approval_mode = "ask_first".into();
    module.save_and_invalidate("agent", &config).unwrap();
    config
}

async fn approve(
    module: &SecurityModule,
    ctx: &ToolCallContext,
    scope: Option<ApprovalReuseScope>,
) -> String {
    let authority = module.approval_authority();
    let request = authority
        .create_target_file_confirmation(ApprovalRequestInput {
            agent_id: ctx.agent_id.clone(),
            owner: ctx.approval_owner.clone(),
            turn_id: "turn".into(),
            tool_call_id: "command".into(),
            tool_name: "bash".into(),
            trigger: ApprovalTrigger::approval_mode("Run the selected command on the phone"),
            tool_call: ToolCallSnapshot {
                display: "bash".into(),
                args: ctx.tool_input.clone(),
            },
        })
        .await
        .unwrap();
    // A remote approval offers the exact invocation, never a command-prefix wildcard.
    assert_eq!(request.reuse_options.len(), 2);
    for option in &request.reuse_options {
        let ApprovalReuseMatcher::ToolCall { input_params, .. } = &option.matcher;
        let recorded: serde_json::Value =
            serde_json::from_str(input_params.as_deref().unwrap()).unwrap();
        assert_eq!(recorded, ctx.tool_input);
    }
    let decision = match scope {
        Some(scope) => {
            let matcher = request
                .reuse_options
                .iter()
                .find(|option| option.scope == scope)
                .unwrap()
                .matcher
                .clone();
            match scope {
                ApprovalReuseScope::Chat => ApprovalDecision::AllowChat {
                    matcher,
                    decided_by: "console".into(),
                },
                ApprovalReuseScope::Agent => ApprovalDecision::AllowAgent {
                    matcher,
                    decided_by: "console".into(),
                },
                _ => unreachable!(),
            }
        }
        None => ApprovalDecision::RunOnce {
            decided_by: "console".into(),
        },
    };
    authority
        .decide_for_authenticated_user("agent", &request.id, decision, "user")
        .await
        .unwrap();
    request.id
}

#[tokio::test]
async fn original_chat_approval_can_only_be_claimed_by_the_exact_remote_command_once() {
    let root = tempfile::tempdir().unwrap();
    let module = SecurityModule::new(Arc::new(SecurityConfigStore::new(root.path().into())), None);
    let ctx = context();
    let id = approve(&module, &ctx, None).await;
    let authority = module.approval_authority();
    assert!(authority
        .consume_authenticated_file_confirmation_if(&ctx, "desktop", |_| panic!(
            "a different target cannot claim the dispatch"
        ))
        .await
        .is_none());
    let mut invalid = Vec::new();
    for (key, value) in [
        ("device_id", json!("other-phone")),
        ("cwd", json!("/other-album")),
        ("command", json!("printf changed")),
    ] {
        let mut changed = ctx.clone();
        changed.tool_input[key] = value;
        changed.local_device_id = changed.tool_input["device_id"].as_str().map(str::to_owned);
        invalid.push(changed);
    }
    for field in ["turn_id", "tool_call_id"] {
        let mut changed = ctx.clone();
        changed.metadata.insert(field.into(), "another".into());
        invalid.push(changed);
    }
    invalid.push(
        ctx.clone()
            .with_chat_address(ChatAddress::new("other-chat")),
    );
    for changed in invalid {
        assert!(authority
            .consume_authenticated_file_confirmation_if(
                &changed,
                changed.local_device_id.as_deref().unwrap(),
                |_| panic!("a different invocation cannot claim the dispatch"),
            )
            .await
            .is_none());
    }
    assert_eq!(
        authority.request(&id).await.unwrap().status,
        ApprovalStatus::Approved
    );
    // A missing dispatch does not spend the user's decision.
    assert!(authority
        .consume_authenticated_file_confirmation_if(&ctx, "phone", |_| false)
        .await
        .is_none());
    assert_eq!(
        authority
            .consume_authenticated_file_confirmation_if(&ctx, "phone", |actual| actual == id)
            .await,
        Some(id.clone())
    );
    assert_eq!(
        authority.request(&id).await.unwrap().status,
        ApprovalStatus::Consumed
    );
    assert!(authority
        .consume_authenticated_file_confirmation_if(&ctx, "phone", |_| panic!(
            "the same command cannot consume RunOnce twice"
        ))
        .await
        .is_none());
    let snapshot = authority.snapshot_at(&ChatAddress::new("chat")).await;
    assert!(snapshot.pending.is_empty());
    assert_eq!(snapshot.resolved.len(), 1);
    assert_eq!(snapshot.resolved[0].id, id);
    assert!(authority
        .snapshot_at(&ChatAddress::new("other-chat"))
        .await
        .resolved
        .is_empty());
}

#[tokio::test]
async fn remembered_command_preserves_device_cwd_literal_wildcards_and_chat_scope_after_reload() {
    use FileReadAdmission::{Allowed, RequiresUserConfirmation};
    for scope in [ApprovalReuseScope::Chat, ApprovalReuseScope::Agent] {
        let root = tempfile::tempdir().unwrap();
        let store = Arc::new(SecurityConfigStore::new(root.path().into()));
        let module = SecurityModule::new(store.clone(), None);
        let captured = ask_first(&module);
        let ctx = context();
        assert_eq!(
            module
                .inspect_chat_command(&ctx, &captured, &ctx, false)
                .await
                .unwrap(),
            RequiresUserConfirmation
        );
        approve(&module, &ctx, Some(scope)).await;
        let reloaded = SecurityModule::new(store, None);
        assert_eq!(
            reloaded
                .inspect_chat_command(&ctx, &captured, &ctx, false)
                .await
                .unwrap(),
            Allowed
        );
        for (key, value) in [
            ("device_id", json!("another-phone")),
            ("cwd", json!("/albums/2026-literal")),
            ("command", json!("printf '%s' photo123.jpg")),
        ] {
            let mut changed = ctx.clone();
            changed.tool_input[key] = value;
            changed.local_device_id = changed.tool_input["device_id"].as_str().map(str::to_owned);
            assert_eq!(
                reloaded
                    .inspect_chat_command(&changed, &captured, &changed, false)
                    .await
                    .unwrap(),
                RequiresUserConfirmation,
                "{scope:?} {key}"
            );
        }
        let other_chat = ctx.with_chat_address(ChatAddress::new("other-chat"));
        assert_eq!(
            reloaded
                .inspect_chat_command(&other_chat, &captured, &other_chat, false)
                .await
                .unwrap(),
            if scope == ApprovalReuseScope::Agent {
                Allowed
            } else {
                RequiresUserConfirmation
            }
        );
    }
}

#[tokio::test]
async fn explicit_and_legacy_command_denials_override_confirmed_or_remembered_permission() {
    for scope in [None, Some(ApprovalReuseScope::Agent)] {
        for denied_location in ["requested", "resolved", "legacy-command"] {
            let root = tempfile::tempdir().unwrap();
            let module =
                SecurityModule::new(Arc::new(SecurityConfigStore::new(root.path().into())), None);
            let captured = ask_first(&module);
            let ctx = context();
            let mut resolved = ctx.clone();
            resolved.tool_input["cwd"] = "/private/photo-runtime".into();
            approve(&module, &ctx, scope).await;
            assert_eq!(
                module
                    .inspect_chat_command(&ctx, &captured, &resolved, true)
                    .await
                    .unwrap(),
                FileReadAdmission::Allowed
            );
            let pattern = match denied_location {
                "requested" => ctx.tool_input.to_string(),
                "resolved" => resolved.tool_input.to_string(),
                // Existing local command rules predate explicit Device/cwd JSON.
                "legacy-command" => "printf *".into(),
                _ => unreachable!(),
            };
            module
                .approval_authority()
                .create_agent_rule(
                    "agent",
                    PermissionRuleInput {
                        effect: PermissionRuleEffect::Deny,
                        enabled: true,
                        label: "Protect this command".into(),
                        matcher: ApprovalReuseMatcher::ToolCall {
                            tool_name: "bash".into(),
                            input_params: Some(pattern),
                        },
                    },
                )
                .await
                .unwrap();
            for confirmed in [false, true] {
                let result = module
                    .inspect_chat_command(&ctx, &captured, &resolved, confirmed)
                    .await;
                assert!(
                    matches!(result, Err(FileReadPolicyError::Denied(_))),
                    "{scope:?} {denied_location} confirmed={confirmed}: {result:?}"
                );
            }
            if denied_location != "resolved" {
                assert!(module
                    .approval_authority()
                    .consume_authenticated_file_confirmation_if(&ctx, "phone", |_| panic!(
                        "a live denial cannot spend a pending approval"
                    ))
                    .await
                    .is_none());
            }
        }
    }
}

#[tokio::test]
async fn resolving_command_cwd_cannot_change_target_command_or_chat_invocation() {
    let root = tempfile::tempdir().unwrap();
    let module = SecurityModule::new(Arc::new(SecurityConfigStore::new(root.path().into())), None);
    let captured = ask_first(&module);
    let mut ctx = context();
    ctx.tool_input.as_object_mut().unwrap().remove("cwd");
    let mut resolved = ctx.clone();
    resolved.tool_input["cwd"] = "/home/mobile".into();
    assert_eq!(
        module
            .inspect_chat_command(&ctx, &captured, &resolved, true)
            .await
            .unwrap(),
        FileReadAdmission::Allowed
    );
    let mut invalid = Vec::new();
    for (key, value) in [
        ("device_id", json!("desktop")),
        ("command", json!("printf changed")),
        ("cwd", json!("relative/path")),
    ] {
        let mut changed = resolved.clone();
        changed.tool_input[key] = value;
        invalid.push(changed);
    }
    let mut wrong_call = resolved.clone();
    wrong_call
        .metadata
        .insert("tool_call_id".into(), "other".into());
    invalid.push(wrong_call);
    invalid.push(
        resolved
            .clone()
            .with_chat_address(ChatAddress::new("other-chat")),
    );
    let mut wrong_executor = resolved.clone();
    wrong_executor.local_device_id = Some("desktop".into());
    invalid.push(wrong_executor);
    for changed in invalid {
        let result = module
            .inspect_chat_command(&ctx, &captured, &changed, true)
            .await;
        assert!(
            matches!(result, Err(FileReadPolicyError::Denied(_))),
            "{result:?}"
        );
    }
    ctx.local_device_id = Some("desktop".into());
    resolved.local_device_id = Some("desktop".into());
    assert!(matches!(
        module
            .inspect_chat_command(&ctx, &captured, &resolved, true)
            .await,
        Err(FileReadPolicyError::Denied(_))
    ));
}
