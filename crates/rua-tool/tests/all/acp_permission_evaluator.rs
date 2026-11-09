use std::sync::Arc;

use agent_client_protocol::schema::v1::{
    PermissionOption, PermissionOptionId, PermissionOptionKind, RequestPermissionRequest,
    ToolCallId, ToolCallUpdate, ToolCallUpdateFields,
};
use rua_acp::permissions::{PermissionDecision, PermissionEvaluator};
use rua_security::{
    ApprovalReuseMatcher, PermissionRuleEffect, PermissionRuleInput, SecurityConfigStore,
    SecurityModule,
};
use rua_tool::builtin::{AcpPermissionTaskContext, SecurityPermissionEvaluator};

fn make_request_with_command(command: &str) -> RequestPermissionRequest {
    let options = vec![PermissionOption::new(
        PermissionOptionId::new("allow_once"),
        "Allow once",
        PermissionOptionKind::AllowOnce,
    )];
    let tool_call = ToolCallUpdate::new(
        ToolCallId::new("tc-1"),
        ToolCallUpdateFields::new()
            .title("bash".to_string())
            .raw_input(serde_json::json!({"command": command})),
    );
    RequestPermissionRequest::new("session-1", tool_call, options)
}

#[tokio::test]
async fn safe_request_is_relayed_without_creating_rua_approval_state() {
    let tmp = tempfile::tempdir().unwrap();
    let security = Arc::new(SecurityModule::new(
        Arc::new(SecurityConfigStore::new(tmp.path().to_path_buf())),
        None,
    ));
    let evaluator = SecurityPermissionEvaluator::with_security(Arc::clone(&security));

    let decision = evaluator
        .evaluate(
            "agent-1",
            "chat-1",
            &make_request_with_command("git status --short"),
        )
        .await;

    assert_eq!(decision, PermissionDecision::Relay);
    assert!(security
        .approval_authority()
        .snapshot_at(&rua_core::ChatAddress::agent("agent-1", "chat-1"))
        .await
        .pending
        .is_empty());
}

#[tokio::test]
async fn user_overridable_safety_findings_remain_runner_owned() {
    let evaluator = SecurityPermissionEvaluator::new();

    for command in ["rm -rf /", "sudo cat /etc/shadow", "mkfs.ext4 /dev/sda1"] {
        let decision = evaluator
            .evaluate("agent-1", "chat-1", &make_request_with_command(command))
            .await;
        assert_eq!(
            decision,
            PermissionDecision::Relay,
            "{command} should remain an ACP runner permission choice unless an explicit host deny matches"
        );
    }
}

#[tokio::test]
async fn task_context_does_not_create_an_approval_authority_request() {
    let evaluator =
        SecurityPermissionEvaluator::new().with_task_context(AcpPermissionTaskContext {
            task_id: "task-123".to_string(),
            attempt_no: 2,
            runner: "codex".to_string(),
            origin_turn_id: Some("turn-owner".to_string()),
            origin_tool_call_id: Some("call-create".to_string()),
            working_dir: "/workspace/owner".to_string(),
        });

    let decision = evaluator
        .evaluate(
            "owner-agent",
            "owner-chat",
            &make_request_with_command("git status --short"),
        )
        .await;

    assert_eq!(decision, PermissionDecision::Relay);
}

#[tokio::test]
async fn explicit_owner_agent_deny_remains_a_host_veto() {
    let tmp = tempfile::tempdir().unwrap();
    let security = Arc::new(SecurityModule::new(
        Arc::new(SecurityConfigStore::new(tmp.path().to_path_buf())),
        None,
    ));
    security
        .approval_authority()
        .create_agent_rule(
            "agent-1",
            PermissionRuleInput {
                effect: PermissionRuleEffect::Deny,
                enabled: true,
                label: "Block external shell".to_string(),
                matcher: ApprovalReuseMatcher::ToolCall {
                    tool_name: "bash".to_string(),
                    input_params: None,
                },
            },
        )
        .await
        .unwrap();
    let evaluator = SecurityPermissionEvaluator::with_security(security);

    let decision = evaluator
        .evaluate(
            "agent-1",
            "chat-1",
            &make_request_with_command("git status --short"),
        )
        .await;

    assert!(matches!(decision, PermissionDecision::Denied { .. }));
}
