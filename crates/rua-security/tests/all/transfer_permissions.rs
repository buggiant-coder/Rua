use std::collections::HashSet;
use std::sync::Arc;

use rua_core::ChatAddress;
use rua_security::approval_authority::{
    ApprovalDecision, ApprovalRequestInput, ApprovalReuseMatcher, ApprovalReuseScope,
    ApprovalStatus, ApprovalTrigger, PermissionRuleEffect, PermissionRuleInput, ToolCallSnapshot,
};
use rua_security::security_module::{FileReadAdmission, FileReadPolicyError};
use rua_security::{SecurityConfigStore, SecurityModule, ToolCallContext};
use serde_json::{json, Value};

fn context(local: &str) -> ToolCallContext {
    let mut ctx = ToolCallContext::new(
        "agent",
        "chat",
        "sync_send",
        json!({
            "source_device_id":"phone", "source_paths":["/album/picture*.jpg"],
            "target_device_id":"desktop", "target_dir":"/imports", "conflict":"fail",
        }),
    )
    .with_chat_address(ChatAddress::new("chat"));
    ctx.local_device_id = Some(local.into());
    ctx.metadata.insert("turn_id".into(), "turn".into());
    ctx.metadata
        .insert("tool_call_id".into(), "transfer".into());
    ctx
}

fn operation(ctx: &ToolCallContext, tool: &str, path: &str) -> ToolCallContext {
    let mut operation = ctx.clone();
    operation.tool_name = tool.into();
    operation.tool_input = if tool == "read" {
        json!({"device_id":ctx.local_device_id,"file_path":path})
    } else {
        json!({"device_id":ctx.local_device_id,"path":path})
    };
    operation
}

async fn approve(module: &SecurityModule, ctx: &ToolCallContext, persistent: bool) -> String {
    let authority = module.approval_authority();
    let request = authority
        .create_target_file_confirmation(ApprovalRequestInput {
            agent_id: ctx.agent_id.clone(),
            owner: ctx.approval_owner.clone(),
            turn_id: "turn".into(),
            tool_call_id: "transfer".into(),
            tool_name: "sync_send".into(),
            trigger: ApprovalTrigger::approval_mode("Copy the selected phone file to the desktop"),
            tool_call: ToolCallSnapshot {
                display: "sync_send".into(),
                args: ctx.tool_input.clone(),
            },
        })
        .await
        .unwrap();
    assert_eq!(request.reuse_options.len(), 2);
    let matcher = request
        .reuse_options
        .iter()
        .find(|option| option.scope == ApprovalReuseScope::Agent)
        .unwrap()
        .matcher
        .clone();
    let decision = if persistent {
        ApprovalDecision::AllowAgent {
            matcher,
            decided_by: "console".into(),
        }
    } else {
        ApprovalDecision::RunOnce {
            decided_by: "console".into(),
        }
    };
    authority
        .decide_for_authenticated_user("agent", &request.id, decision, "user")
        .await
        .unwrap();
    request.id
}

#[tokio::test]
async fn one_real_transfer_approval_covers_each_live_participant_permit_once() {
    let root = tempfile::tempdir().unwrap();
    let module = SecurityModule::new(Arc::new(SecurityConfigStore::new(root.path().into())), None);
    let ctx = context("desktop");
    let id = approve(&module, &ctx, false).await;
    let authority = module.approval_authority();
    let mut permits: HashSet<&str> = ["phone", "desktop"].into();
    assert!(authority
        .consume_authenticated_file_confirmation_if(&ctx, "phone", |_| panic!(
            "ordinary file consumer must not claim a transfer"
        ))
        .await
        .is_none());
    for device in ["phone", "desktop"] {
        assert_eq!(
            authority
                .consume_authenticated_transfer_confirmation_if(&ctx, device, |actual| actual == id
                    && permits.remove(device))
                .await,
            Some(id.clone())
        );
        assert_eq!(
            authority.request(&id).await.unwrap().status,
            ApprovalStatus::Consumed
        );
    }
    assert!(authority
        .consume_authenticated_transfer_confirmation_if(&ctx, "phone", |_| permits.remove("phone"))
        .await
        .is_none());
    assert!(authority
        .consume_authenticated_transfer_confirmation_if(&ctx, "other-device", |_| panic!(
            "not a participant"
        ))
        .await
        .is_none());
    for field in ["turn_id", "tool_call_id"] {
        let mut changed = ctx.clone();
        changed.metadata.insert(field.into(), "another".into());
        assert!(authority
            .consume_authenticated_transfer_confirmation_if(&changed, "phone", |_| panic!(
                "not the approved call"
            ))
            .await
            .is_none());
    }
    let mut changed = ctx.clone();
    changed.tool_input["target_dir"] = "/other".into();
    assert!(authority
        .consume_authenticated_transfer_confirmation_if(&changed, "phone", |_| panic!(
            "not the approved destination"
        ))
        .await
        .is_none());
    assert_eq!(
        authority
            .snapshot_at(&ChatAddress::new("chat"))
            .await
            .resolved
            .len(),
        1
    );
}

#[tokio::test]
async fn remembered_transfer_keeps_both_devices_and_wildcard_filenames_literal() {
    let root = tempfile::tempdir().unwrap();
    let store = Arc::new(SecurityConfigStore::new(root.path().into()));
    let module = SecurityModule::new(store.clone(), None);
    let mut captured = store.load("agent").unwrap();
    captured.tool_guard.approval_mode = "ask_first".into();
    module.save_and_invalidate("agent", &captured).unwrap();
    let ctx = context("phone");
    approve(&module, &ctx, true).await;
    // Reload the existing settings store, as another reader of the synchronized rule does.
    let reloaded = SecurityModule::new(store, None);
    for (local, tool, path) in [
        ("phone", "read", "/album/picture*.jpg"),
        ("desktop", "edit", "/imports/picture*.jpg"),
    ] {
        let ctx = context(local);
        assert_eq!(
            reloaded
                .inspect_chat_transfer_operation(
                    &ctx,
                    &captured,
                    &[operation(&ctx, tool, path)],
                    false
                )
                .await
                .unwrap(),
            FileReadAdmission::Allowed
        );
    }
    for (key, value) in [
        ("source_device_id", json!("another-phone")),
        ("source_paths", json!(["/album/picture123.jpg"])),
        ("target_device_id", json!("another-desktop")),
        ("target_dir", json!("/other-imports")),
        ("conflict", json!("overwrite")),
    ] {
        let mut changed = ctx.clone();
        changed.tool_input[key] = value;
        changed.local_device_id = changed.tool_input["source_device_id"]
            .as_str()
            .map(str::to_owned);
        let read = operation(
            &changed,
            "read",
            changed.tool_input["source_paths"][0].as_str().unwrap(),
        );
        assert_eq!(
            reloaded
                .inspect_chat_transfer_operation(&changed, &captured, &[read], false)
                .await
                .unwrap(),
            FileReadAdmission::RequiresUserConfirmation,
            "{key}"
        );
    }
    let config = reloaded.config_store().load("agent").unwrap();
    let args = config.extra["permission_rules"][0]["matcher"]["input_params"]
        .as_str()
        .unwrap();
    let args: Value = serde_json::from_str(args).unwrap();
    assert_eq!(args, ctx.tool_input);
    assert!(args.get("device_id").is_none());
}

#[tokio::test]
async fn source_and_destination_denials_override_a_confirmed_transfer() {
    for (local, tool, requested, resolved) in [
        ("phone", "read", "/album/picture*.jpg", "/private/photo.jpg"),
        (
            "desktop",
            "edit",
            "/imports/picture*.jpg",
            "/private/output.jpg",
        ),
    ] {
        let root = tempfile::tempdir().unwrap();
        let module =
            SecurityModule::new(Arc::new(SecurityConfigStore::new(root.path().into())), None);
        let captured = module.config_store().load("agent").unwrap();
        let ctx = context(local);
        let denied = operation(&ctx, tool, resolved);
        module
            .approval_authority()
            .create_agent_rule(
                "agent",
                PermissionRuleInput {
                    effect: PermissionRuleEffect::Deny,
                    enabled: true,
                    label: "Protect actual path".into(),
                    matcher: ApprovalReuseMatcher::ToolCall {
                        tool_name: tool.into(),
                        input_params: Some(denied.tool_input.to_string()),
                    },
                },
            )
            .await
            .unwrap();
        let result = module
            .inspect_chat_transfer_operation(
                &ctx,
                &captured,
                &[operation(&ctx, tool, requested), denied],
                true,
            )
            .await;
        assert!(
            matches!(result, Err(FileReadPolicyError::Denied(_))),
            "{result:?}"
        );
        let snapshot = module
            .approval_authority()
            .snapshot_at(&ChatAddress::new("chat"))
            .await;
        assert!(snapshot.pending.is_empty() && snapshot.resolved.is_empty());
    }
}

#[tokio::test]
async fn transfer_projection_cannot_change_device_role_or_invocation() {
    let root = tempfile::tempdir().unwrap();
    let module = SecurityModule::new(Arc::new(SecurityConfigStore::new(root.path().into())), None);
    let captured = module.config_store().load("agent").unwrap();
    let ctx = context("phone");
    let read = operation(&ctx, "read", "/album/picture*.jpg");
    let mut wrong_device = read.clone();
    wrong_device.tool_input["device_id"] = "desktop".into();
    let mut wrong_call = read.clone();
    wrong_call
        .metadata
        .insert("tool_call_id".into(), "other".into());
    for invalid in [
        wrong_device,
        wrong_call,
        operation(&ctx, "edit", "/imports/picture*.jpg"),
    ] {
        assert!(matches!(
            module
                .inspect_chat_transfer_operation(&ctx, &captured, &[invalid], true)
                .await,
            Err(FileReadPolicyError::Denied(_))
        ));
    }
}
