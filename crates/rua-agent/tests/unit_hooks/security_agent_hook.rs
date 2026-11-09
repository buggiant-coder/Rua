use std::sync::Arc;

use agent_types::ToolCall;
use react_agent::hooks::{AgentHook, ApprovalResult};
use react_agent::{PermissionHookDecision, ToolDecision};
use rua_agent::hooks::SecurityAgentHook;
use rua_config::types::security::SecurityConfig;
use rua_security::approval_authority::{
    ApprovalDecision, ApprovalReuseMatcher, ApprovalTriggerSource, PermissionRuleEffect,
    PermissionRuleInput,
};
use tempfile::TempDir;

fn make_call(name: &str) -> ToolCall {
    ToolCall {
        id: "tc_1".into(),
        name: name.into(),
        arguments: serde_json::json!({"command": "ls"}),
        raw_arguments: None,
        provider_call_id: None,
        metadata: Default::default(),
    }
}

fn make_call_with_args(name: &str, arguments: serde_json::Value) -> ToolCall {
    ToolCall {
        id: "tc_1".into(),
        name: name.into(),
        arguments,
        raw_arguments: None,
        provider_call_id: None,
        metadata: Default::default(),
    }
}

fn make_test_security() -> (Arc<rua_security::SecurityModule>, TempDir) {
    let tmp = TempDir::new().expect("security tempdir");
    let config_store = Arc::new(rua_security::SecurityConfigStore::new(
        tmp.path().to_path_buf(),
    ));
    (
        Arc::new(rua_security::SecurityModule::new(config_store, None)),
        tmp,
    )
}

#[tokio::test]
async fn permission_hook_decisions_are_persisted_as_run_once_approval_facts() {
    for (decision, expected_status) in [
        (
            PermissionHookDecision::Allow { reason: None },
            rua_security::AuthorityStatus::Approved,
        ),
        (
            PermissionHookDecision::Deny {
                reason: "blocked".to_string(),
            },
            rua_security::AuthorityStatus::Rejected,
        ),
    ] {
        let (security, _tmp) = make_test_security();
        let address = rua_core::ChatAddress::agent("agent-1", "chat-1");
        let hook = SecurityAgentHook::new(Arc::clone(&security), true)
            .with_agent_id("agent-1")
            .with_session_id("chat-1")
            .with_chat_address(address.clone())
            .with_turn_id("turn-1");
        let result = hook
            .record_permission_hook_decision(
                "digest-a",
                "approval required",
                &make_call("sensitive_tool"),
                &decision,
            )
            .await;

        assert_eq!(
            matches!(result, ApprovalResult::Allow),
            matches!(decision, PermissionHookDecision::Allow { .. })
        );
        let snapshot = security.approval_authority().snapshot_at(&address).await;
        assert!(snapshot.pending.is_empty());
        assert_eq!(snapshot.resolved.len(), 1);
        assert_eq!(snapshot.resolved[0].status, expected_status);
        assert!(security
            .approval_authority()
            .list_chat_rules(&address)
            .await
            .is_empty());
    }
}

#[tokio::test]
async fn security_hook_denies_explicitly_denied_tool_before_approval() {
    let (security, _tmp) = make_test_security();
    security
        .approval_authority()
        .create_agent_rule(
            "",
            PermissionRuleInput {
                effect: PermissionRuleEffect::Deny,
                enabled: true,
                label: "Block dangerous tool".into(),
                matcher: ApprovalReuseMatcher::ToolCall {
                    tool_name: "dangerous_tool".into(),
                    input_params: None,
                },
            },
        )
        .await
        .expect("create deny rule");
    let hook = SecurityAgentHook::new(security, true);

    let result = hook
        .before_tool_call(&make_call("dangerous_tool"), 0)
        .await
        .unwrap();

    assert!(
        matches!(result, ToolDecision::Deny { .. }),
        "auto-denied tool must be denied"
    );
}

#[tokio::test]
async fn security_hook_hands_off_ignores_approval_required_list() {
    let tmp = TempDir::new().expect("security tempdir");
    let config_store = Arc::new(rua_security::SecurityConfigStore::new(
        tmp.path().to_path_buf(),
    ));
    let security = Arc::new(rua_security::SecurityModule::new(config_store, None));
    let hook = SecurityAgentHook::new(Arc::clone(&security), true)
        .with_session_id("session-1")
        .with_agent_id("agent-1")
        .with_turn_id("turn-1")
        .with_approval_required(vec!["guarded_tool".into()]);
    let call = make_call("guarded_tool");

    let first = hook.before_tool_call(&call, 0).await.unwrap();
    let second = hook.before_tool_call(&call, 1).await.unwrap();

    assert!(
        matches!(first, ToolDecision::Allow),
        "hands_off should ignore approval_required for ordinary calls"
    );
    assert!(matches!(second, ToolDecision::Allow));

    let snapshot = security
        .approval_authority()
        .snapshot_at(&rua_core::ChatAddress::agent("agent-1", "session-1"))
        .await;
    assert!(snapshot.pending.is_empty());
}

#[tokio::test]
async fn security_hook_ask_first_asks_for_tool_calls() {
    let (security, _tmp) = make_test_security();
    let mut config = SecurityConfig::default();
    config.tool_guard.approval_mode = "ask_first".into();
    security.config_store().save("agent-1", &config).unwrap();

    let hook = SecurityAgentHook::new(security, true).with_agent_id("agent-1");

    let result = hook
        .before_tool_call(&make_call("sensitive_tool"), 0)
        .await
        .unwrap();

    assert!(matches!(result, ToolDecision::RequiresApproval { .. }));
}

#[tokio::test]
async fn security_hook_uses_typed_project_chat_address_for_approval_ownership() {
    let (security, _tmp) = make_test_security();
    let mut config = SecurityConfig::default();
    config.tool_guard.approval_mode = "ask_first".into();
    security.config_store().save("agent-1", &config).unwrap();
    let address = rua_core::ChatAddress::project("project-1", "chat-1");

    let hook = SecurityAgentHook::new(Arc::clone(&security), true)
        .with_agent_id("agent-1")
        .with_session_id("chat-1")
        .with_chat_address(address.clone());

    let result = hook
        .before_tool_call(&make_call("sensitive_tool"), 0)
        .await
        .unwrap();

    assert!(matches!(result, ToolDecision::RequiresApproval { .. }));
    let project_snapshot = security.approval_authority().snapshot_at(&address).await;
    assert_eq!(project_snapshot.pending.len(), 1);
    assert_eq!(project_snapshot.pending[0].agent_id, "agent-1");
    assert!(security
        .approval_authority()
        .snapshot_at(&rua_core::ChatAddress::agent("agent-1", "chat-1"))
        .await
        .pending
        .is_empty());
}

#[tokio::test]
async fn security_hook_approval_bypass_allows_approval_prompts_for_cron() {
    let (security, _tmp) = make_test_security();
    let mut config = SecurityConfig::default();
    config.tool_guard.approval_mode = "ask_first".into();
    security.config_store().save("agent-1", &config).unwrap();

    let hook = SecurityAgentHook::new(security, true)
        .with_agent_id("agent-1")
        .with_approval_bypass(true);

    let result = hook
        .before_tool_call(&make_call("sensitive_tool"), 0)
        .await
        .unwrap();

    assert!(matches!(result, ToolDecision::Allow));
}

#[tokio::test]
async fn legacy_balanced_mode_allows_ordinary_bash_without_authority_request() {
    let (security, _tmp) = make_test_security();
    let mut config = SecurityConfig::default();
    config.tool_guard.approval_mode = "balanced".into();
    security.config_store().save("agent-1", &config).unwrap();

    let hook = SecurityAgentHook::new(Arc::clone(&security), true)
        .with_agent_id("agent-1")
        .with_session_id("chat-1")
        .with_turn_id("turn-1");
    let call = make_call("bash");

    let decision = hook.before_tool_call(&call, 0).await.unwrap();
    assert!(
        matches!(decision, ToolDecision::Allow),
        "legacy balanced mode should behave as hands_off for ordinary Bash, got {decision:?}"
    );

    let snapshot = security
        .approval_authority()
        .snapshot_at(&rua_core::ChatAddress::agent("agent-1", "chat-1"))
        .await;
    assert!(snapshot.pending.is_empty());
}

#[tokio::test]
async fn wait_for_approval_reuses_structured_request_created_by_security_module() {
    let (security, _tmp) = make_test_security();
    let address = rua_core::ChatAddress::agent("agent-1", "chat-1");
    let hook = SecurityAgentHook::new(Arc::clone(&security), true)
        .with_agent_id("agent-1")
        .with_session_id("runtime-session-1")
        .with_chat_address(address.clone())
        .with_turn_id("turn-1");
    let call = make_call_with_args("read", serde_json::json!({"path": "project/.env"}));

    let decision = hook.before_tool_call(&call, 0).await.unwrap();
    let request_id = match decision {
        ToolDecision::RequiresApproval { request_id, .. } => request_id,
        other => panic!("expected File Guard approval, got {other:?}"),
    };

    let snapshot = security.approval_authority().snapshot_at(&address).await;
    assert_eq!(snapshot.pending.len(), 1);
    assert_eq!(snapshot.pending[0].id, request_id);
    assert_eq!(snapshot.pending[0].turn_id, "turn-1");
    assert_eq!(
        snapshot.pending[0].trigger.source,
        ApprovalTriggerSource::FileGuard
    );

    let authority = Arc::clone(security.approval_authority());
    let wait = hook.wait_for_approval("tc_1", "read", "File Guard requires approval", &call);
    let decide = async {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        authority
            .decide(
                &request_id,
                ApprovalDecision::Reject {
                    decided_by: "user".into(),
                },
            )
            .await
            .expect("reject request");
    };
    let (result, _) = tokio::join!(wait, decide);
    assert_eq!(
        result,
        react_agent::hooks::ApprovalResult::Deny {
            reason: "rejected by the user".into()
        }
    );
}

#[tokio::test]
async fn security_hook_serializes_concurrent_guard_decisions() {
    let (security, _tmp) = make_test_security();
    let hook = Arc::new(
        SecurityAgentHook::new(security, true)
            .with_session_id("sess-1")
            .with_agent_id("agent-1"),
    );
    let hook1 = hook.clone();
    let hook2 = hook.clone();
    let call_a = make_call_with_args("read", serde_json::json!({"path": "project/.env"}));
    let call_b = make_call_with_args("read", serde_json::json!({"path": "project/src/lib.rs"}));

    let (first, second) = tokio::join!(
        hook1.before_tool_call(&call_a, 0),
        hook2.before_tool_call(&call_b, 0),
    );

    assert!(matches!(
        first.unwrap(),
        ToolDecision::RequiresApproval { .. }
    ));
    assert!(matches!(second.unwrap(), ToolDecision::Allow));
}
