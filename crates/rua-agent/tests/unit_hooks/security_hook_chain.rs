use std::path::PathBuf;
use std::sync::Arc;

use agent_types::ToolCall;
use react_agent::hooks::{AgentHook, ApprovalResult};
use rua_agent::{CompositeHook, SecurityAgentHook};
use rua_security::approval_authority::{
    ApprovalDecision, ApprovalReuseMatcher, ApprovalStatus, PermissionRuleEffect,
    PermissionRuleInput,
};
use rua_security::{SecurityVerdict, Severity, ToolGuardFinding};

fn make_test_security() -> Arc<rua_security::SecurityModule> {
    make_test_security_in(tempfile::tempdir().unwrap().keep())
}

fn make_test_security_in(dir: PathBuf) -> Arc<rua_security::SecurityModule> {
    let config_store = Arc::new(rua_security::SecurityConfigStore::new(dir));
    Arc::new(rua_security::SecurityModule::new(config_store, None))
}

fn make_call(name: &str, arguments: serde_json::Value) -> ToolCall {
    ToolCall {
        id: "tc_1".into(),
        name: name.into(),
        arguments,
        raw_arguments: None,
        provider_call_id: None,
        metadata: Default::default(),
    }
}

async fn wait_for_pending_request_id(
    security: &rua_security::SecurityModule,
    address: &rua_core::ChatAddress,
) -> String {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        let snapshot = security.approval_authority().snapshot_at(address).await;
        if let Some(request) = snapshot.pending.first() {
            return request.id.clone();
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "approval request was not created"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn default_hook_chain_has_security_agent_hook() {
    let security = make_test_security();
    security
        .approval_authority()
        .create_agent_rule(
            "",
            PermissionRuleInput {
                effect: PermissionRuleEffect::Deny,
                enabled: true,
                label: "Block forbidden tool".into(),
                matcher: ApprovalReuseMatcher::ToolCall {
                    tool_name: "forbidden_tool".into(),
                    input_params: None,
                },
            },
        )
        .await
        .expect("create deny rule");
    let security_hook = SecurityAgentHook::new(security, true);

    let composite = CompositeHook::new(vec![Arc::new(security_hook)]);

    let call = make_call("forbidden_tool", serde_json::json!({}));

    let decision = composite.before_tool_call(&call, 0).await.unwrap();
    assert!(
        decision.is_deny(),
        "SecurityAgentHook must deny tools in deny list, got: {decision:?}"
    );
}

#[tokio::test]
async fn default_hook_chain_allows_safe_tools() {
    let security = make_test_security();
    let security_hook = SecurityAgentHook::new(security, true);
    let composite = CompositeHook::new(vec![Arc::new(security_hook)]);

    let call = make_call("safe_tool", serde_json::json!({}));

    let decision = composite.before_tool_call(&call, 0).await.unwrap();
    assert!(
        decision.is_allow(),
        "SecurityAgentHook must allow tools not in deny list, got: {decision:?}"
    );
}

#[test]
fn guard_hook_type_does_not_exist() {
    assert!(
        !std::any::type_name::<CompositeHook>().contains("GuardHook"),
        "CompositeHook must not reference GuardHook"
    );
}

#[tokio::test]
async fn disabled_security_hook_passes_through_even_when_module_would_deny() {
    let security = rua_security::SecurityModule::for_test(SecurityVerdict::Deny {
        reason: "blocked by test policy".into(),
    });
    let security_hook = SecurityAgentHook::new(security, false);
    let call = make_call("bash", serde_json::json!({"command": "rm -rf /"}));

    let decision = security_hook.before_tool_call(&call, 0).await.unwrap();

    assert!(
        decision.is_allow(),
        "disabled SecurityAgentHook must bypass the guard module, got: {decision:?}"
    );
}

#[tokio::test]
async fn requires_approval_verdict_is_exposed_as_tool_decision_with_prompt_and_request_id() {
    let security = rua_security::SecurityModule::for_test(SecurityVerdict::RequiresApproval {
        request_id: "approval-123".into(),
        prompt: "Tool `bash` requires approval".into(),
    });
    let security_hook = SecurityAgentHook::new(security, true);
    let call = make_call("bash", serde_json::json!({"command": "touch file"}));

    let decision = security_hook.before_tool_call(&call, 0).await.unwrap();

    match decision {
        SecurityVerdict::RequiresApproval { request_id, prompt } => {
            assert_eq!(request_id, "approval-123");
            assert!(prompt.contains("bash"));
        }
        other => panic!("expected RequiresApproval decision, got {other:?}"),
    }
}

#[tokio::test]
async fn audit_only_verdict_is_not_collapsed_by_security_hook() {
    let finding = ToolGuardFinding {
        rule_id: "TEST_AUDIT".into(),
        category: "test".into(),
        severity: Severity::Info,
        title: "Audit finding".into(),
        description: "record only".into(),
        tool_name: "read".into(),
        guardian: "test_guardian".into(),
        message: "record only".into(),
        remediation: String::new(),
        matched_pattern: None,
        matched_value: None,
    };
    let security = rua_security::SecurityModule::for_test(SecurityVerdict::AuditOnly {
        findings: vec![finding],
    });
    let security_hook = SecurityAgentHook::new(security, true);
    let call = make_call("read", serde_json::json!({"path": "README.md"}));

    let decision = security_hook.before_tool_call(&call, 0).await.unwrap();

    match decision {
        SecurityVerdict::AuditOnly { findings } => {
            assert_eq!(findings.len(), 1);
            assert_eq!(findings[0].rule_id, "TEST_AUDIT");
        }
        other => panic!("expected AuditOnly decision, got {other:?}"),
    }
}

#[tokio::test]
async fn wait_for_approval_denies_when_session_is_missing() {
    let security = make_test_security();
    let security_hook = SecurityAgentHook::new(security, true).with_agent_id("agent-1");
    let call = make_call("bash", serde_json::json!({"command": "rm file"}));

    let result = security_hook
        .wait_for_approval("tc_1", "bash", "needs approval", &call)
        .await;

    match result {
        ApprovalResult::Deny { reason } => assert!(reason.contains("no session")),
        other => panic!("expected Deny without session, got {other:?}"),
    }
}

#[tokio::test]
async fn wait_for_approval_blocks_without_using_ask_user_store() {
    let security = make_test_security();
    let runtime_session_id = "runtime-session-1";
    let address = rua_core::ChatAddress::agent("agent-1", "chat-1");
    let security_hook = SecurityAgentHook::new(Arc::clone(&security), true)
        .with_agent_id("agent-1")
        .with_session_id(runtime_session_id)
        .with_chat_address(address.clone())
        .with_turn_id("turn-1");
    let call = make_call("bash", serde_json::json!({"command": "rm file"}));
    rua_tool::extended::ask_user::AskUserService::global()
        .clear_by_session(runtime_session_id)
        .await;

    let wait = security_hook.wait_for_approval("tc_1", "bash", "needs approval", &call);
    let decide = async {
        let request_id = wait_for_pending_request_id(&security, &address).await;
        assert!(
            rua_tool::extended::ask_user::AskUserService::global()
                .get_pending_by_session(runtime_session_id)
                .await
                .is_none(),
            "tool approval must not borrow the ask_user pending-question store"
        );
        security
            .approval_authority()
            .decide(
                &request_id,
                ApprovalDecision::RunOnce {
                    decided_by: "approved by test".into(),
                },
            )
            .await
            .expect("resolve authority approval");
        request_id
    };
    let (result, request_id) = tokio::join!(wait, decide);
    assert_eq!(result, ApprovalResult::Allow);
    let snapshot = security.approval_authority().snapshot_at(&address).await;
    assert_eq!(snapshot.resolved.len(), 1);
    assert_eq!(snapshot.resolved[0].id, request_id);
    assert!(
        rua_tool::extended::ask_user::AskUserService::global()
            .get_pending_by_session(runtime_session_id)
            .await
            .is_none(),
        "tool approval must not borrow the ask_user pending-question store"
    );
}

#[tokio::test]
async fn wait_for_approval_run_once_does_not_create_reuse_rule() {
    let security = make_test_security();
    let address = rua_core::ChatAddress::agent("agent-1", "chat-1");
    let security_hook = SecurityAgentHook::new(Arc::clone(&security), true)
        .with_agent_id("agent-1")
        .with_session_id("runtime-session-1")
        .with_chat_address(address.clone())
        .with_turn_id("turn-1");
    let call = make_call("bash", serde_json::json!({"command": "rm file"}));

    let first_wait = security_hook.wait_for_approval("tc_1", "bash", "needs approval", &call);
    let first_decide = async {
        let request_id = wait_for_pending_request_id(&security, &address).await;
        security
            .approval_authority()
            .decide(
                &request_id,
                ApprovalDecision::RunOnce {
                    decided_by: "approved by test".into(),
                },
            )
            .await
            .expect("resolve authority approval");
        request_id
    };
    let (first, request_id) = tokio::join!(first_wait, first_decide);
    assert_eq!(first, ApprovalResult::Allow);

    let second_wait = security_hook.wait_for_approval("tc_1", "bash", "needs approval", &call);
    let second_decide = async {
        let second_request_id = wait_for_pending_request_id(&security, &address).await;
        security
            .approval_authority()
            .decide(
                &second_request_id,
                ApprovalDecision::RunOnce {
                    decided_by: "approved by test".into(),
                },
            )
            .await
            .expect("resolve second authority approval");
        second_request_id
    };
    let (second, second_request_id) = tokio::join!(second_wait, second_decide);
    assert_eq!(second, ApprovalResult::Allow);
    assert_ne!(request_id, second_request_id);

    let snapshot = security.approval_authority().snapshot_at(&address).await;
    assert_eq!(snapshot.pending.len(), 0);
    assert_eq!(snapshot.resolved.len(), 2);
    assert_eq!(snapshot.resolved[0].status, ApprovalStatus::Approved);
}
