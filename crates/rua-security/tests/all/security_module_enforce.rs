use rua_config::types::security::{SecurityConfig, ToolGuardRuleConfig};
use rua_security::{
    ApprovalDecision, ApprovalReuseMatcher, ApprovalTrigger, ApprovalTriggerSource,
    AuthorityStatus, PermissionRuleEffect, PermissionRuleInput, SecurityConfigStore,
    SecurityModule, SecurityVerdict, ToolCallContext, ToolCallSnapshot,
};
use std::sync::Arc;

fn make_module(dir: &std::path::Path) -> Arc<SecurityModule> {
    Arc::new(SecurityModule::new(
        Arc::new(SecurityConfigStore::new(dir.to_path_buf())),
        None,
    ))
}

fn make_ctx(tool_name: &str) -> ToolCallContext {
    ToolCallContext::new("agent-1", "session-1", tool_name, serde_json::json!({}))
}

fn make_bash_ctx(command: &str) -> ToolCallContext {
    ToolCallContext::new(
        "agent-1",
        "session-1",
        "bash",
        serde_json::json!({"command": command}),
    )
}

#[tokio::test]
async fn deny_permission_rule_returns_deny() {
    let tmp = tempfile::tempdir().unwrap();
    let module = make_module(tmp.path());
    let ctx = make_ctx("bash");
    module
        .approval_authority()
        .create_agent_rule(
            "agent-1",
            PermissionRuleInput {
                effect: PermissionRuleEffect::Deny,
                enabled: true,
                label: "Block bash".into(),
                matcher: ApprovalReuseMatcher::ToolCall {
                    tool_name: "bash".into(),
                    input_params: None,
                },
            },
        )
        .await
        .expect("create deny rule");

    let verdict = module.enforce(&ctx, &[]).await;
    match verdict {
        SecurityVerdict::Deny { reason } => {
            assert!(reason.contains("bash"), "reason should mention tool name");
            assert!(reason.contains("Block bash"), "reason should mention rule");
        }
        other => panic!("expected Deny, got {other:?}"),
    }
}

#[tokio::test]
async fn allowed_tool_returns_allow() {
    let tmp = tempfile::tempdir().unwrap();
    let module = make_module(tmp.path());
    let ctx = make_ctx("read");

    let verdict = module.enforce(&ctx, &[]).await;
    assert!(
        matches!(verdict, SecurityVerdict::Allow),
        "expected Allow, got {verdict:?}"
    );
}

#[tokio::test]
async fn default_hands_off_ignores_approval_required() {
    let tmp = tempfile::tempdir().unwrap();
    let module = make_module(tmp.path());
    let ctx = make_ctx("bash");
    let approval_required = vec!["bash".to_string()];

    let verdict = module.enforce(&ctx, &approval_required).await;
    assert!(
        matches!(verdict, SecurityVerdict::Allow),
        "default hands_off should ignore approval_required, got {verdict:?}"
    );
}

#[tokio::test]
async fn default_hands_off_allows_ordinary_bash_without_authority_request() {
    let tmp = tempfile::tempdir().unwrap();
    let module = make_module(tmp.path());
    let ctx = ToolCallContext::new(
        "agent-1",
        "session-1",
        "bash",
        serde_json::json!({"command": "git status"}),
    );

    let verdict = module.enforce(&ctx, &[]).await;

    assert!(
        matches!(verdict, SecurityVerdict::Allow),
        "default hands_off should allow ordinary Bash calls, got {verdict:?}"
    );
    let snapshot = module
        .approval_authority()
        .snapshot_for_runtime_session("agent-1", "session-1")
        .await;
    assert!(snapshot.pending.is_empty());
}

#[tokio::test]
async fn ask_first_approval_mode_fallback_creates_authority_request() {
    let tmp = tempfile::tempdir().unwrap();
    let module = make_module(tmp.path());
    let mut config = SecurityConfig::default();
    config.tool_guard.approval_mode = "ask_first".into();
    module.config_store().save("agent-1", &config).unwrap();
    let mut ctx = ToolCallContext::new(
        "agent-1",
        "session-1",
        "bash",
        serde_json::json!({"command": "git status"}),
    );
    ctx.metadata
        .insert("tool_call_id".into(), serde_json::json!("call-git-status"));
    ctx.metadata
        .insert("turn_id".into(), serde_json::json!("turn-1"));

    let verdict = module.enforce(&ctx, &[]).await;

    let request_id = match verdict {
        SecurityVerdict::RequiresApproval { request_id, prompt } => {
            assert!(
                !request_id.is_empty(),
                "request_id must come from authority"
            );
            assert!(prompt.contains("Ask First"));
            request_id
        }
        other => panic!("expected approval-mode RequiresApproval, got {other:?}"),
    };
    let snapshot = module
        .approval_authority()
        .snapshot_for_runtime_session("agent-1", "session-1")
        .await;
    assert_eq!(snapshot.pending.len(), 1);
    let request = &snapshot.pending[0];
    assert_eq!(request.id, request_id);
    assert_eq!(request.tool_call_id, "call-git-status");
    assert_eq!(request.turn_id, "turn-1");
    assert_eq!(request.trigger.source, ApprovalTriggerSource::ApprovalMode);
    assert!(request.trigger.message.contains("Ask First"));
}

#[tokio::test]
async fn approval_request_without_turn_metadata_does_not_fallback_to_session_id() {
    let tmp = tempfile::tempdir().unwrap();
    let module = make_module(tmp.path());
    let mut config = SecurityConfig::default();
    config.tool_guard.approval_mode = "ask_first".into();
    module.config_store().save("agent-1", &config).unwrap();
    let mut ctx = ToolCallContext::new(
        "agent-1",
        "session-1",
        "bash",
        serde_json::json!({"command": "git status"}),
    );
    ctx.metadata
        .insert("tool_call_id".into(), serde_json::json!("call-no-turn"));

    let verdict = module.enforce(&ctx, &[]).await;

    assert!(
        matches!(verdict, SecurityVerdict::RequiresApproval { .. }),
        "Ask First bash should ask for approval"
    );
    let snapshot = module
        .approval_authority()
        .snapshot_for_runtime_session("agent-1", "session-1")
        .await;
    assert_eq!(snapshot.pending.len(), 1);
    assert_eq!(snapshot.pending[0].tool_call_id, "call-no-turn");
    assert_eq!(
        snapshot.pending[0].turn_id, "",
        "ApprovalRequest must not use session_id as a fake turn_id"
    );
}

#[tokio::test]
async fn approval_mode_is_trimmed_and_case_insensitive_for_hands_off() {
    let tmp = tempfile::tempdir().unwrap();
    let module = make_module(tmp.path());
    let mut config = SecurityConfig::default();
    config.tool_guard.approval_mode = " HANDS_OFF ".into();
    module.config_store().save("agent-1", &config).unwrap();

    let verdict = module
        .enforce(&make_ctx("bash"), &["bash".to_string()])
        .await;

    assert!(
        matches!(verdict, SecurityVerdict::Allow),
        "Hands-off approval mode should bypass approval_required independent of casing"
    );
}

#[tokio::test]
async fn approval_mode_is_trimmed_and_case_insensitive_for_ask_first() {
    let tmp = tempfile::tempdir().unwrap();
    let module = make_module(tmp.path());
    let mut config = SecurityConfig::default();
    config.tool_guard.approval_mode = " ASK_FIRST ".into();
    module.config_store().save("agent-1", &config).unwrap();

    let verdict = module.enforce(&make_ctx("read"), &[]).await;

    assert!(
        matches!(verdict, SecurityVerdict::RequiresApproval { .. }),
        "Ask First approval mode should require approval independent of casing"
    );
}

#[tokio::test]
async fn chat_scope_reuse_allows_matching_bash_call_only_in_same_chat() {
    let tmp = tempfile::tempdir().unwrap();
    let module = make_module(tmp.path());
    let mut config = SecurityConfig::default();
    config.tool_guard.approval_mode = "ask_first".into();
    module.config_store().save("agent-1", &config).unwrap();
    let request = module
        .approval_authority()
        .create_request(rua_security::ApprovalRequestInput {
            agent_id: "agent-1".into(),
            owner: rua_security::ApprovalOwner::chat(rua_core::ChatAddress::agent(
                "agent-1",
                "session-1",
            )),
            turn_id: "turn-1".into(),
            tool_call_id: "call-git-status".into(),
            tool_name: "bash".into(),
            trigger: ApprovalTrigger::approval_mode("Balanced requires approval"),
            tool_call: ToolCallSnapshot {
                display: "bash git status --short".into(),
                args: serde_json::json!({ "command": "git status --short" }),
            },
        })
        .await;
    module
        .approval_authority()
        .decide(
            &request.id,
            ApprovalDecision::AllowChat {
                decided_by: "user".into(),
                matcher: ApprovalReuseMatcher::ToolCall {
                    tool_name: "bash".into(),
                    input_params: Some("git status *".into()),
                },
            },
        )
        .await
        .expect("allow chat");

    let same_chat_ctx = make_bash_ctx("git status")
        .with_chat_address(rua_core::ChatAddress::agent("agent-1", "session-1"));
    let same_chat = module.enforce(&same_chat_ctx, &["bash".into()]).await;
    assert!(
        matches!(same_chat, SecurityVerdict::Allow),
        "same chat git status should be allowed by Chat Scope reuse, got {same_chat:?}"
    );

    let other_chat = ToolCallContext::new(
        "agent-1",
        "session-2",
        "bash",
        serde_json::json!({"command": "git status"}),
    )
    .with_chat_address(rua_core::ChatAddress::agent("agent-1", "session-2"));
    let verdict = module.enforce(&other_chat, &["bash".into()]).await;
    assert!(
        matches!(verdict, SecurityVerdict::RequiresApproval { .. }),
        "different chat should not use Chat Scope reuse, got {verdict:?}"
    );
}

#[tokio::test]
async fn chat_scope_reuse_survives_security_module_recreate_without_settings_visibility_payload() {
    let tmp = tempfile::tempdir().unwrap();
    let module = make_module(tmp.path());
    let request = module
        .approval_authority()
        .create_request(rua_security::ApprovalRequestInput {
            agent_id: "agent-1".into(),
            owner: rua_security::ApprovalOwner::chat(rua_core::ChatAddress::agent(
                "agent-1",
                "session-1",
            )),
            turn_id: "turn-1".into(),
            tool_call_id: "call-du".into(),
            tool_name: "bash".into(),
            trigger: ApprovalTrigger::approval_mode("Balanced requires approval"),
            tool_call: ToolCallSnapshot {
                display: "bash du -sh target".into(),
                args: serde_json::json!({ "command": "du -sh target" }),
            },
        })
        .await;

    module
        .approval_authority()
        .decide(
            &request.id,
            ApprovalDecision::AllowChat {
                decided_by: "user".into(),
                matcher: ApprovalReuseMatcher::ToolCall {
                    tool_name: "bash".into(),
                    input_params: Some("du -sh *".into()),
                },
            },
        )
        .await
        .expect("allow chat");

    let path = tmp
        .path()
        .join("workspaces/agent-1/sessions/session-1/permission_rules.json");
    let stored: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("chat rules file"))
            .expect("valid chat rules json");
    assert!(stored[0].get("agent_id").is_none());
    assert!(stored[0].get("chat_id").is_none());
    assert!(stored[0].get("scope").is_none());

    let recreated = make_module(tmp.path());
    let verdict = recreated
        .enforce(&make_bash_ctx("du -sh other-target"), &["bash".into()])
        .await;
    assert!(
        matches!(verdict, SecurityVerdict::Allow),
        "same chat should reuse persisted Chat Scope rule after recreate, got {verdict:?}"
    );
}

#[tokio::test]
async fn deny_permission_rule_takes_precedence_over_chat_scope_reuse() {
    let tmp = tempfile::tempdir().unwrap();
    let module = make_module(tmp.path());
    let request = module
        .approval_authority()
        .create_request(rua_security::ApprovalRequestInput {
            agent_id: "agent-1".into(),
            owner: rua_security::ApprovalOwner::chat(rua_core::ChatAddress::agent(
                "agent-1",
                "session-1",
            )),
            turn_id: "turn-1".into(),
            tool_call_id: "call-git-status".into(),
            tool_name: "bash".into(),
            trigger: ApprovalTrigger::approval_mode("Balanced requires approval"),
            tool_call: ToolCallSnapshot {
                display: "bash git status --short".into(),
                args: serde_json::json!({ "command": "git status --short" }),
            },
        })
        .await;
    module
        .approval_authority()
        .decide(
            &request.id,
            ApprovalDecision::AllowChat {
                decided_by: "user".into(),
                matcher: ApprovalReuseMatcher::ToolCall {
                    tool_name: "bash".into(),
                    input_params: Some("git status *".into()),
                },
            },
        )
        .await
        .expect("allow chat");
    module
        .approval_authority()
        .create_agent_rule(
            "agent-1",
            PermissionRuleInput {
                effect: PermissionRuleEffect::Deny,
                enabled: true,
                label: "Block git status".into(),
                matcher: ApprovalReuseMatcher::ToolCall {
                    tool_name: "bash".into(),
                    input_params: Some("git status *".into()),
                },
            },
        )
        .await
        .expect("create deny rule");

    let verdict = module.enforce(&make_bash_ctx("git status"), &[]).await;
    assert!(
        matches!(verdict, SecurityVerdict::Deny { .. }),
        "hard deny must win over Chat Scope reuse, got {verdict:?}"
    );
}

#[tokio::test]
async fn agent_scope_reuse_allows_later_chats_for_same_agent_only() {
    let tmp = tempfile::tempdir().unwrap();
    let module = make_module(tmp.path());
    let mut config = SecurityConfig::default();
    config.tool_guard.approval_mode = "ask_first".into();
    module.config_store().save("agent-1", &config).unwrap();
    module.config_store().save("agent-2", &config).unwrap();
    let request = module
        .approval_authority()
        .create_request(rua_security::ApprovalRequestInput {
            agent_id: "agent-1".into(),
            owner: rua_security::ApprovalOwner::chat(rua_core::ChatAddress::agent(
                "agent-1",
                "session-1",
            )),
            turn_id: "turn-1".into(),
            tool_call_id: "call-git-status".into(),
            tool_name: "bash".into(),
            trigger: ApprovalTrigger::approval_mode("Balanced requires approval"),
            tool_call: ToolCallSnapshot {
                display: "bash git status --short".into(),
                args: serde_json::json!({ "command": "git status --short" }),
            },
        })
        .await;
    module
        .approval_authority()
        .decide(
            &request.id,
            ApprovalDecision::AllowAgent {
                decided_by: "user".into(),
                matcher: ApprovalReuseMatcher::ToolCall {
                    tool_name: "bash".into(),
                    input_params: Some("git status *".into()),
                },
            },
        )
        .await
        .expect("allow agent");

    let later_chat = ToolCallContext::new(
        "agent-1",
        "session-2",
        "bash",
        serde_json::json!({"command": "git status"}),
    );
    let verdict = module.enforce(&later_chat, &["bash".into()]).await;
    assert!(
        matches!(verdict, SecurityVerdict::Allow),
        "Agent Scope reuse should apply across chats for the same agent, got {verdict:?}"
    );

    let other_agent = ToolCallContext::new(
        "agent-2",
        "session-2",
        "bash",
        serde_json::json!({"command": "git status"}),
    );
    let verdict = module.enforce(&other_agent, &["bash".into()]).await;
    assert!(
        matches!(verdict, SecurityVerdict::RequiresApproval { .. }),
        "Agent Scope reuse must not cross agents, got {verdict:?}"
    );
}

#[tokio::test]
async fn agent_scope_reuse_survives_security_module_recreate() {
    let tmp = tempfile::tempdir().unwrap();
    let module = make_module(tmp.path());
    let request = module
        .approval_authority()
        .create_request(rua_security::ApprovalRequestInput {
            agent_id: "agent-1".into(),
            owner: rua_security::ApprovalOwner::chat(rua_core::ChatAddress::agent(
                "agent-1",
                "session-1",
            )),
            turn_id: "turn-1".into(),
            tool_call_id: "call-git-status".into(),
            tool_name: "bash".into(),
            trigger: ApprovalTrigger::approval_mode("Balanced requires approval"),
            tool_call: ToolCallSnapshot {
                display: "bash git status --short".into(),
                args: serde_json::json!({ "command": "git status --short" }),
            },
        })
        .await;
    module
        .approval_authority()
        .decide(
            &request.id,
            ApprovalDecision::AllowAgent {
                decided_by: "user".into(),
                matcher: ApprovalReuseMatcher::ToolCall {
                    tool_name: "bash".into(),
                    input_params: Some("git status *".into()),
                },
            },
        )
        .await
        .expect("allow agent");

    let restarted = make_module(tmp.path());
    let later_chat = ToolCallContext::new(
        "agent-1",
        "session-2",
        "bash",
        serde_json::json!({"command": "git status"}),
    );
    let verdict = restarted.enforce(&later_chat, &["bash".into()]).await;
    assert!(
        matches!(verdict, SecurityVerdict::Allow),
        "durable Agent Scope reuse should load after module recreate, got {verdict:?}"
    );
}

#[tokio::test]
async fn agent_scope_reuse_applies_in_project_chat() {
    let tmp = tempfile::tempdir().unwrap();
    let module = make_module(tmp.path());
    let mut config = SecurityConfig::default();
    config.tool_guard.approval_mode = "ask_first".into();
    module.config_store().save("agent-1", &config).unwrap();
    module
        .approval_authority()
        .create_agent_rule(
            "agent-1",
            PermissionRuleInput {
                effect: PermissionRuleEffect::Allow,
                enabled: true,
                label: "Allow git status".into(),
                matcher: ApprovalReuseMatcher::ToolCall {
                    tool_name: "bash".into(),
                    input_params: Some("git status *".into()),
                },
            },
        )
        .await
        .expect("create Agent rule");
    let ctx = ToolCallContext::new(
        "agent-1",
        "shared-chat",
        "bash",
        serde_json::json!({"command": "git status"}),
    )
    .with_chat_address(rua_core::ChatAddress::project("project-1", "shared-chat"));

    let verdict = module.enforce(&ctx, &[]).await;

    assert!(
        matches!(verdict, SecurityVerdict::Allow),
        "Agent Scope reuse should apply in Project Chat, got {verdict:?}"
    );
}

#[tokio::test]
async fn deleting_project_chat_rules_removes_durable_file_and_preserves_agent_rules() {
    let tmp = tempfile::tempdir().unwrap();
    let module = make_module(tmp.path());
    let address = rua_core::ChatAddress::project("project-1", "chat-1");
    let request = module
        .approval_authority()
        .create_request(rua_security::ApprovalRequestInput {
            agent_id: "agent-1".into(),
            owner: rua_security::ApprovalOwner::chat(address.clone()),
            turn_id: "turn-1".into(),
            tool_call_id: "call-git-status".into(),
            tool_name: "bash".into(),
            trigger: ApprovalTrigger::approval_mode("approval required"),
            tool_call: ToolCallSnapshot {
                display: "bash git status --short".into(),
                args: serde_json::json!({"command": "git status --short"}),
            },
        })
        .await;
    module
        .approval_authority()
        .decide(
            &request.id,
            ApprovalDecision::AllowChat {
                decided_by: "user".into(),
                matcher: ApprovalReuseMatcher::ToolCall {
                    tool_name: "bash".into(),
                    input_params: Some("git status *".into()),
                },
            },
        )
        .await
        .expect("allow Project Chat");
    let agent_rule = module
        .approval_authority()
        .create_agent_rule(
            "agent-1",
            PermissionRuleInput {
                effect: PermissionRuleEffect::Allow,
                enabled: true,
                label: "Allow Agent git status".into(),
                matcher: ApprovalReuseMatcher::ToolCall {
                    tool_name: "bash".into(),
                    input_params: Some("git status *".into()),
                },
            },
        )
        .await
        .expect("create Agent rule");
    let path = tmp
        .path()
        .join("projects/project-1/sessions/chat-1/permission_rules.json");
    assert!(path.is_file());

    module
        .approval_authority()
        .delete_chat_rules(&address)
        .await
        .expect("delete Project Chat rules");

    assert!(!path.exists());
    let recreated = make_module(tmp.path());
    assert!(recreated
        .approval_authority()
        .list_chat_rules(&address)
        .await
        .is_empty());
    assert!(recreated
        .approval_authority()
        .list_agent_rules("agent-1")
        .await
        .iter()
        .any(|rule| rule.id == agent_rule.id));
}

#[tokio::test]
async fn file_guard_ask_finding_creates_structured_pending_approval() {
    let tmp = tempfile::tempdir().unwrap();
    let module = make_module(tmp.path());
    let ctx = ToolCallContext::new(
        "agent-1",
        "session-1",
        "read",
        serde_json::json!({"path": "project/.env"}),
    );

    let verdict = module.enforce(&ctx, &[]).await;

    let request_id = match verdict {
        SecurityVerdict::RequiresApproval { request_id, prompt } => {
            assert!(prompt.contains("FILE-SENSITIVE"));
            request_id
        }
        other => panic!("expected RequiresApproval from File Guard, got {other:?}"),
    };
    let snapshot = module
        .approval_authority()
        .snapshot_for_runtime_session("agent-1", "session-1")
        .await;
    assert_eq!(snapshot.pending.len(), 1);
    let request = &snapshot.pending[0];
    assert_eq!(request.id, request_id);
    assert_eq!(request.status, AuthorityStatus::Pending);
    assert_eq!(request.tool_name, "read");
    assert_eq!(request.trigger.source, ApprovalTriggerSource::FileGuard);
    assert_eq!(request.trigger.rule_id.as_deref(), Some("FILE-SENSITIVE"));
    assert_eq!(request.trigger.guardian.as_deref(), Some("file_guardian"));
    assert_eq!(request.trigger.severity.as_deref(), Some("HIGH"));
    assert!(request.trigger.message.contains("sensitive file"));
}

#[tokio::test]
async fn tool_guard_ask_finding_creates_structured_pending_approval() {
    let tmp = tempfile::tempdir().unwrap();
    let module = make_module(tmp.path());
    let mut config = SecurityConfig::default();
    config.tool_guard.custom_rules = vec![custom_command_rule(
        "CUSTOM_ASK_ECHO",
        "NeedsApproval",
        vec!["bash"],
        vec!["echo"],
    )];
    module.config_store().save("agent-1", &config).unwrap();

    let verdict = module.enforce(&make_bash_ctx("echo secret"), &[]).await;

    let request_id = match verdict {
        SecurityVerdict::RequiresApproval { request_id, prompt } => {
            assert!(prompt.contains("CUSTOM_ASK_ECHO"));
            request_id
        }
        other => panic!("expected RequiresApproval from Tool Guard, got {other:?}"),
    };
    let snapshot = module
        .approval_authority()
        .snapshot_for_runtime_session("agent-1", "session-1")
        .await;
    assert_eq!(snapshot.pending.len(), 1);
    let request = &snapshot.pending[0];
    assert_eq!(request.id, request_id);
    assert_eq!(request.status, AuthorityStatus::Pending);
    assert_eq!(request.tool_name, "bash");
    assert_eq!(request.trigger.source, ApprovalTriggerSource::ToolGuard);
    assert_eq!(request.trigger.rule_id.as_deref(), Some("CUSTOM_ASK_ECHO"));
    assert_eq!(
        request.trigger.guardian.as_deref(),
        Some("ast_rule_guardian")
    );
    assert_eq!(request.trigger.severity.as_deref(), Some("HIGH"));
}

#[tokio::test]
async fn rm_outside_workspace_creates_structured_pending_approval() {
    let tmp = tempfile::tempdir().unwrap();
    let module = make_module(tmp.path());
    let mut ctx = make_bash_ctx("rm -rf /tmp/outside-workspace");
    ctx.working_dir = "/home/user/project".to_string();

    let verdict = module.enforce(&ctx, &[]).await;

    let request_id = match verdict {
        SecurityVerdict::RequiresApproval { request_id, prompt } => {
            assert!(
                prompt.contains("outside workspace boundary"),
                "prompt should describe the rm boundary finding: {prompt}"
            );
            request_id
        }
        other => panic!("expected RequiresApproval from rm boundary finding, got {other:?}"),
    };
    let snapshot = module
        .approval_authority()
        .snapshot_for_runtime_session("agent-1", "session-1")
        .await;
    assert_eq!(snapshot.pending.len(), 1);
    let request = &snapshot.pending[0];
    assert_eq!(request.id, request_id);
    assert_eq!(request.status, AuthorityStatus::Pending);
    assert_eq!(request.tool_name, "bash");
    assert_eq!(request.trigger.source, ApprovalTriggerSource::ToolGuard);
    assert_eq!(
        request.trigger.rule_id.as_deref(),
        Some("TOOL_CMD_DANGEROUS_RM")
    );
    assert_eq!(request.trigger.severity.as_deref(), Some("CRITICAL"));
}

#[tokio::test]
async fn hard_deny_finding_creates_denied_record_that_cannot_be_approved() {
    let tmp = tempfile::tempdir().unwrap();
    let module = make_module(tmp.path());
    let mut config = SecurityConfig::default();
    config.tool_guard.custom_rules = vec![custom_command_rule(
        "CUSTOM_DENY_ECHO",
        "Deny",
        vec!["bash"],
        vec!["echo"],
    )];
    module.config_store().save("agent-1", &config).unwrap();

    let verdict = module.enforce(&make_bash_ctx("echo secret"), &[]).await;
    assert!(
        matches!(verdict, SecurityVerdict::Deny { .. }),
        "expected hard deny, got {verdict:?}"
    );

    let snapshot = module
        .approval_authority()
        .snapshot_for_runtime_session("agent-1", "session-1")
        .await;
    assert_eq!(snapshot.pending.len(), 0);
    assert_eq!(snapshot.resolved.len(), 1);
    let request = &snapshot.resolved[0];
    assert_eq!(request.status, AuthorityStatus::Denied);
    assert_eq!(request.trigger.source, ApprovalTriggerSource::ToolGuard);
    assert_eq!(request.trigger.rule_id.as_deref(), Some("CUSTOM_DENY_ECHO"));

    let err = module
        .approval_authority()
        .decide(
            &request.id,
            ApprovalDecision::RunOnce {
                decided_by: "user".into(),
            },
        )
        .await
        .expect_err("denied records cannot be approved");
    assert!(
        err.to_string().contains("cannot decide"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn hard_deny_finding_takes_precedence_over_agent_scope_reuse() {
    let tmp = tempfile::tempdir().unwrap();
    let module = make_module(tmp.path());
    let request = module
        .approval_authority()
        .create_request(rua_security::ApprovalRequestInput {
            agent_id: "agent-1".into(),
            owner: rua_security::ApprovalOwner::chat(rua_core::ChatAddress::agent(
                "agent-1",
                "session-1",
            )),
            turn_id: "turn-1".into(),
            tool_call_id: "call-git-status".into(),
            tool_name: "bash".into(),
            trigger: ApprovalTrigger::approval_mode("Balanced requires approval"),
            tool_call: ToolCallSnapshot {
                display: "bash git status".into(),
                args: serde_json::json!({ "command": "git status" }),
            },
        })
        .await;
    module
        .approval_authority()
        .decide(
            &request.id,
            ApprovalDecision::AllowAgent {
                decided_by: "user".into(),
                matcher: ApprovalReuseMatcher::ToolCall {
                    tool_name: "bash".into(),
                    input_params: Some("git status *".into()),
                },
            },
        )
        .await
        .expect("allow agent");

    let mut config = SecurityConfig::default();
    config.tool_guard.custom_rules = vec![custom_command_rule(
        "CUSTOM_DENY_GIT",
        "Deny",
        vec!["bash"],
        vec!["git"],
    )];
    module.config_store().save("agent-1", &config).unwrap();
    module.invalidate("agent-1");

    let verdict = module
        .enforce(&make_bash_ctx("git status"), &["bash".into()])
        .await;
    assert!(
        matches!(verdict, SecurityVerdict::Deny { .. }),
        "hard deny must win over Agent Scope reuse, got {verdict:?}"
    );

    let snapshot = module
        .approval_authority()
        .snapshot_for_runtime_session("agent-1", "session-1")
        .await;
    assert!(
        snapshot
            .resolved
            .iter()
            .any(|request| request.status == AuthorityStatus::Denied
                && request.trigger.rule_id.as_deref() == Some("CUSTOM_DENY_GIT")),
        "hard deny should be recorded in Approval Authority"
    );
}

#[tokio::test]
async fn engine_deny_result_is_preserved_as_security_deny() {
    let tmp = tempfile::tempdir().unwrap();
    let module = make_module(tmp.path());
    let mut config = SecurityConfig::default();
    config.tool_guard.custom_rules = vec![custom_command_rule(
        "CUSTOM_DENY_ECHO",
        "Deny",
        vec!["bash"],
        vec!["echo"],
    )];
    module.config_store().save("agent-1", &config).unwrap();

    let verdict = module.enforce(&make_bash_ctx("echo secret"), &[]).await;

    match verdict {
        SecurityVerdict::Deny { reason } => {
            assert!(reason.contains("CUSTOM_DENY_ECHO"));
            assert!(reason.contains("bash"));
        }
        other => panic!("expected Deny from engine decision, got {other:?}"),
    }
}

#[tokio::test]
async fn custom_rule_action_is_trimmed_and_case_insensitive() {
    let tmp = tempfile::tempdir().unwrap();
    let module = make_module(tmp.path());
    let mut config = SecurityConfig::default();
    config.tool_guard.custom_rules = vec![custom_command_rule(
        "CUSTOM_LOWERCASE_DENY",
        " deny ",
        vec!["bash"],
        vec!["echo"],
    )];
    module.config_store().save("agent-1", &config).unwrap();

    let verdict = module.enforce(&make_bash_ctx("echo secret"), &[]).await;

    assert!(
        matches!(verdict, SecurityVerdict::Deny { .. }),
        "custom rule action should accept lowercase/trimmed deny"
    );
}

#[tokio::test]
async fn engine_audit_only_result_is_preserved_as_security_audit_only() {
    let tmp = tempfile::tempdir().unwrap();
    let module = make_module(tmp.path());
    let mut config = SecurityConfig::default();
    config.tool_guard.custom_rules = vec![custom_command_rule(
        "CUSTOM_AUDIT_ECHO",
        "Audit",
        vec!["bash"],
        vec!["echo"],
    )];
    module.config_store().save("agent-1", &config).unwrap();

    let verdict = module.enforce(&make_bash_ctx("echo observed"), &[]).await;

    match verdict {
        SecurityVerdict::AuditOnly { findings } => {
            assert_eq!(findings.len(), 1);
            assert_eq!(findings[0].rule_id, "CUSTOM_AUDIT_ECHO");
        }
        other => panic!("expected AuditOnly from engine decision, got {other:?}"),
    }
}

#[tokio::test]
async fn decision_lock_serializes_concurrent_calls() {
    let tmp = tempfile::tempdir().unwrap();
    let module = make_module(tmp.path());

    let m1 = Arc::clone(&module);
    let m2 = Arc::clone(&module);

    let h1 = tokio::spawn(async move {
        let ctx = make_ctx("tool_a");
        m1.enforce(&ctx, &[]).await
    });
    let h2 = tokio::spawn(async move {
        let ctx = make_ctx("tool_b");
        m2.enforce(&ctx, &[]).await
    });

    let (r1, r2) = tokio::join!(h1, h2);
    assert!(matches!(r1.unwrap(), SecurityVerdict::Allow));
    assert!(matches!(r2.unwrap(), SecurityVerdict::Allow));
}

fn custom_command_rule(
    id: &str,
    action: &str,
    tools: Vec<&str>,
    command_set: Vec<&str>,
) -> ToolGuardRuleConfig {
    serde_json::from_value(serde_json::json!({
        "id": id,
        "tools": tools,
        "category": "command_injection",
        "severity": "HIGH",
        "action": action,
        "command_set": command_set
    }))
    .unwrap()
}

#[tokio::test]
async fn composite_checks_leaf_policy_even_when_parent_has_reusable_allow() {
    let tmp = tempfile::tempdir().unwrap();
    let module = make_module(tmp.path());
    let mut config = SecurityConfig::default();
    config.tool_guard.custom_rules = vec![custom_command_rule(
        "review-copy",
        "NeedsApproval",
        vec!["bash"],
        vec!["cp"],
    )];
    module.save_and_invalidate("agent-1", &config).unwrap();
    let mut parent = make_ctx("composite");
    parent.tool_input =
        serde_json::json!({"actions":[{"tool":"bash","arguments":{"command":"cp a b"}}]});
    parent
        .metadata
        .insert("turn_id".into(), serde_json::json!("turn-1"));
    parent
        .metadata
        .insert("tool_call_id".into(), serde_json::json!("call-1"));
    module
        .approval_authority()
        .create_agent_rule(
            "agent-1",
            PermissionRuleInput {
                effect: PermissionRuleEffect::Allow,
                enabled: true,
                label: "Composite reuse".into(),
                matcher: ApprovalReuseMatcher::ToolCall {
                    tool_name: "composite".into(),
                    input_params: None,
                },
            },
        )
        .await
        .unwrap();
    let operation = make_bash_ctx("cp a b");
    let verdict = module
        .enforce_composite(&parent, std::slice::from_ref(&operation))
        .await;
    let request_id = match verdict {
        SecurityVerdict::RequiresApproval { request_id, .. } => request_id,
        other => panic!("{other:?}"),
    };
    let snapshot = module
        .approval_authority()
        .snapshot_for_runtime_session("agent-1", "session-1")
        .await;
    assert_eq!(snapshot.pending.len(), 1);
    assert_eq!(snapshot.pending[0].tool_name, "composite");
    assert_eq!(snapshot.pending[0].tool_call.args, parent.tool_input);
    module
        .approval_authority()
        .decide(
            &request_id,
            ApprovalDecision::RunOnce {
                decided_by: "user".into(),
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        module
            .enforce_composite(&parent, std::slice::from_ref(&operation))
            .await,
        SecurityVerdict::Allow
    ));
    module
        .approval_authority()
        .create_agent_rule(
            "agent-1",
            PermissionRuleInput {
                effect: PermissionRuleEffect::Deny,
                enabled: true,
                label: "No copying".into(),
                matcher: ApprovalReuseMatcher::ToolCall {
                    tool_name: "bash".into(),
                    input_params: None,
                },
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        module.enforce_composite(&parent, &[operation]).await,
        SecurityVerdict::Deny { .. }
    ));
}
