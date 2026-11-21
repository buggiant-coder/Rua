use std::sync::Arc;

use rua_core::ChatAddress;
use rua_security::approval_authority::{
    ApprovalDecision, ApprovalOwner, ApprovalRequestInput, ApprovalReuseMatcher,
    ApprovalReuseScope, ApprovalTrigger, PermissionRuleEffect, PermissionRuleInput,
    ToolCallSnapshot,
};
use rua_security::security_module::{FileReadAdmission, FileReadPolicyError};
use rua_security::{SecurityConfigStore, SecurityModule, ToolCallContext};
use serde_json::{json, Value};

fn context(tool: &str, args: Value) -> ToolCallContext {
    let mut ctx = ToolCallContext::new("agent", "chat", tool, args)
        .with_chat_address(ChatAddress::new("chat"));
    ctx.local_device_id = Some("phone".into());
    ctx.metadata.insert("turn_id".into(), "turn".into());
    ctx.metadata.insert("tool_call_id".into(), tool.into());
    ctx
}

async fn remember(
    module: &SecurityModule,
    ctx: &ToolCallContext,
    scope: ApprovalReuseScope,
) -> String {
    let authority = module.approval_authority();
    let request = authority
        .create_target_file_confirmation(ApprovalRequestInput {
            agent_id: ctx.agent_id.clone(),
            owner: ctx.approval_owner.clone(),
            turn_id: "turn".into(),
            tool_call_id: ctx.tool_name.clone(),
            tool_name: ctx.tool_name.clone(),
            trigger: ApprovalTrigger::approval_mode("Confirm requested file operation"),
            tool_call: ToolCallSnapshot {
                display: ctx.tool_name.clone(),
                args: ctx.tool_input.clone(),
            },
        })
        .await
        .unwrap();
    assert_eq!(request.reuse_options.len(), 2);
    let matcher = request
        .reuse_options
        .iter()
        .find(|option| option.scope == scope)
        .unwrap()
        .matcher
        .clone();
    let decision = match scope {
        ApprovalReuseScope::Chat => ApprovalDecision::AllowChat {
            matcher,
            decided_by: "console".into(),
        },
        ApprovalReuseScope::Agent => ApprovalDecision::AllowAgent {
            matcher,
            decided_by: "console".into(),
        },
        _ => unreachable!(),
    };
    authority
        .decide_for_authenticated_user("agent", &request.id, decision, "user")
        .await
        .unwrap();
    request.id
}

#[tokio::test]
async fn shared_file_permissions_cover_exact_device_operation_and_literal_arguments() {
    for scope in [ApprovalReuseScope::Chat, ApprovalReuseScope::Agent] {
        for (tool, args) in [
            (
                "read",
                json!({"device_id":"phone","file_path":"/album/","after":"picture-*.jpg","limit":1}),
            ),
            (
                "grep",
                json!({"device_id":"phone","path":"/album","pattern":"camera*","max_results":1}),
            ),
            (
                "glob",
                json!({"device_id":"phone","path":"/album","pattern":"**/*.jpg","offset":1}),
            ),
            (
                "edit",
                json!({"device_id":"phone","patch":"*** Begin Patch\n*** Add File: /album/report.txt\n+camera*\n*** End Patch"}),
            ),
        ] {
            let root = tempfile::tempdir().unwrap();
            let module =
                SecurityModule::new(Arc::new(SecurityConfigStore::new(root.path().into())), None);
            let mut captured = module.config_store().load("agent").unwrap();
            captured.tool_guard.approval_mode = "ask_first".into();
            module.save_and_invalidate("agent", &captured).unwrap();
            let ctx = context(tool, args);
            let request_id = remember(&module, &ctx, scope).await;
            assert_eq!(
                module
                    .inspect_chat_file_operation(&ctx, &captured, &[], false)
                    .await
                    .unwrap(),
                FileReadAdmission::Allowed,
                "{tool}"
            );
            assert_eq!(
                module
                    .approval_authority()
                    .consume_authenticated_file_confirmation_if(&ctx, "phone", |id| id
                        == request_id)
                    .await,
                Some(request_id.clone())
            );

            for (field, value) in [("device_id", json!("desktop")), ("limit", json!(9))] {
                let mut different = ctx.clone();
                different.tool_input[field] = value;
                assert_eq!(
                    module
                        .inspect_chat_file_operation(&different, &captured, &[], false)
                        .await
                        .unwrap(),
                    FileReadAdmission::RequiresUserConfirmation,
                    "{tool} {field}"
                );
                assert!(module
                    .approval_authority()
                    .consume_authenticated_file_confirmation_if(&different, "phone", |_| true)
                    .await
                    .is_none());
            }
            let mut other_tool = ctx.clone();
            other_tool.tool_name = if tool == "grep" { "glob" } else { "grep" }.into();
            assert_eq!(
                module
                    .inspect_chat_file_operation(&other_tool, &captured, &[], false)
                    .await
                    .unwrap(),
                FileReadAdmission::RequiresUserConfirmation
            );
            if tool == "grep" || tool == "glob" {
                let mut expanded = ctx.clone();
                expanded.tool_input["pattern"] = "camera-and-other-files.jpg".into();
                assert_eq!(
                    module
                        .inspect_chat_file_operation(&expanded, &captured, &[], false)
                        .await
                        .unwrap(),
                    FileReadAdmission::RequiresUserConfirmation
                );
            }

            let reopened =
                SecurityModule::new(Arc::new(SecurityConfigStore::new(root.path().into())), None);
            assert_eq!(
                reopened
                    .inspect_chat_file_operation(&ctx, &captured, &[], false)
                    .await
                    .unwrap(),
                FileReadAdmission::Allowed
            );
            match scope {
                ApprovalReuseScope::Chat => module
                    .approval_authority()
                    .delete_chat_rules(&ChatAddress::new("chat"))
                    .await
                    .unwrap(),
                ApprovalReuseScope::Agent => {
                    let rules = module.approval_authority().list_agent_rules("agent").await;
                    assert!(module
                        .approval_authority()
                        .delete_agent_rule("agent", &rules[0].id)
                        .await
                        .unwrap());
                }
                _ => unreachable!(),
            }
            assert_eq!(
                reopened
                    .inspect_chat_file_operation(&ctx, &captured, &[], false)
                    .await
                    .unwrap(),
                FileReadAdmission::RequiresUserConfirmation
            );
            assert!(module
                .approval_authority()
                .consume_authenticated_file_confirmation_if(&ctx, "phone", |_| true)
                .await
                .is_none());
        }
    }
}

#[tokio::test]
async fn a_confirmed_multi_file_patch_still_obeys_each_target_path_deny() {
    let root = tempfile::tempdir().unwrap();
    let module = SecurityModule::new(Arc::new(SecurityConfigStore::new(root.path().into())), None);
    let mut config = module.config_store().load("agent").unwrap();
    config.tool_guard.approval_mode = "ask_first".into();
    config
        .file_guard
        .sensitive_files
        .push("/album/report.txt".into());
    module.save_and_invalidate("agent", &config).unwrap();
    let ctx = context(
        "edit",
        json!({
            "device_id":"phone",
            "patch":"*** Begin Patch
*** Add File: /album/report.txt
+report
*** Add File: /album/index.txt
+index
*** End Patch"
        }),
    );
    let paths: Vec<_> = ["/album/report.txt", "/album/index.txt"]
        .into_iter()
        .map(|path| context("edit", json!({"device_id":"phone","path":path})))
        .collect();
    assert_eq!(
        module
            .inspect_chat_file_operation(&ctx, &config, &paths, false)
            .await
            .unwrap(),
        FileReadAdmission::RequiresUserConfirmation
    );
    remember(&module, &ctx, ApprovalReuseScope::Chat).await;
    assert_eq!(
        module
            .inspect_chat_file_operation(&ctx, &config, &paths, false)
            .await
            .unwrap(),
        FileReadAdmission::Allowed
    );
    let deny = module
        .approval_authority()
        .create_agent_rule(
            "agent",
            PermissionRuleInput {
                effect: PermissionRuleEffect::Deny,
                enabled: true,
                label: "Keep index".into(),
                matcher: ApprovalReuseMatcher::ToolCall {
                    tool_name: "edit".into(),
                    input_params: Some(paths[1].tool_input.to_string()),
                },
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        module
            .inspect_chat_file_operation(&ctx, &config, &paths, true)
            .await,
        Err(FileReadPolicyError::Denied(_))
    ));
    module
        .approval_authority()
        .update_agent_rule(
            "agent",
            &deny.id,
            PermissionRuleInput {
                effect: PermissionRuleEffect::Deny,
                enabled: true,
                label: "Local index rule".into(),
                matcher: ApprovalReuseMatcher::ToolCall {
                    tool_name: "edit".into(),
                    input_params: Some(json!({"path":"/album/index.txt"}).to_string()),
                },
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        module
            .inspect_chat_file_operation(&ctx, &config, &paths, true)
            .await,
        Err(FileReadPolicyError::Denied(_))
    ));
    let mut wrong_owner = paths[0].clone();
    wrong_owner.approval_owner = ApprovalOwner::chat(ChatAddress::new("other"));
    assert!(matches!(
        module
            .inspect_chat_file_operation(&ctx, &config, &[wrong_owner], true)
            .await,
        Err(FileReadPolicyError::Denied(_))
    ));
}
