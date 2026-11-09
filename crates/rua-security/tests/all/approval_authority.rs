use rua_core::ChatAddress;
use rua_security::approval_authority::{
    ApprovalAuthority, ApprovalDecision, ApprovalModePreset, ApprovalOwner, ApprovalRequestInput,
    ApprovalReuseMatcher, ApprovalReuseScope, ApprovalStatus, ApprovalTrigger,
    PermissionRuleEffect, PermissionRuleInput, ToolCallSnapshot,
};
use serde_json::json;

fn request(tool_call_id: &str, tool_name: &str) -> ApprovalRequestInput {
    ApprovalRequestInput {
        agent_id: "agent-1".into(),
        owner: rua_security::ApprovalOwner::chat(ChatAddress::agent("agent-1", "chat-1")),
        turn_id: "turn-1".into(),
        tool_call_id: tool_call_id.into(),
        tool_name: tool_name.into(),
        trigger: ApprovalTrigger::approval_mode("Balanced requires approval"),
        tool_call: ToolCallSnapshot {
            display: format!("{tool_name} display"),
            args: json!({ "tool": tool_name }),
        },
    }
}

fn bash_request(tool_call_id: &str, command: &str) -> ApprovalRequestInput {
    ApprovalRequestInput {
        agent_id: "agent-1".into(),
        owner: rua_security::ApprovalOwner::chat(ChatAddress::agent("agent-1", "chat-1")),
        turn_id: "turn-1".into(),
        tool_call_id: tool_call_id.into(),
        tool_name: "bash".into(),
        trigger: ApprovalTrigger::approval_mode("Balanced requires approval"),
        tool_call: ToolCallSnapshot {
            display: format!("Bash {command}"),
            args: json!({ "command": command }),
        },
    }
}

#[tokio::test]
async fn snapshot_keeps_multiple_pending_requests_for_one_chat() {
    let authority = ApprovalAuthority::new();
    let mut events = authority.subscribe();

    let bash = authority.create_request(request("call-bash", "bash")).await;
    let read = authority.create_request(request("call-read", "read")).await;

    assert_ne!(bash.id, read.id);

    let snapshot = authority
        .snapshot_at(&rua_core::ChatAddress::agent("agent-1", "chat-1"))
        .await;
    assert_eq!(snapshot.pending.len(), 2);
    assert_eq!(snapshot.resolved.len(), 0);

    let pending_ids = snapshot
        .pending
        .iter()
        .map(|request| request.id.as_str())
        .collect::<std::collections::HashSet<_>>();
    assert!(pending_ids.contains(bash.id.as_str()));
    assert!(pending_ids.contains(read.id.as_str()));

    assert_eq!(events.recv().await.unwrap().event, "approval.created");
    assert_eq!(events.recv().await.unwrap().event, "approval.created");
}

#[test]
fn approval_authority_statuses_have_stable_wire_names() {
    let statuses = [
        ApprovalStatus::Pending,
        ApprovalStatus::Approved,
        ApprovalStatus::Rejected,
        ApprovalStatus::Denied,
        ApprovalStatus::Expired,
        ApprovalStatus::Consumed,
        ApprovalStatus::Superseded,
    ];

    assert_eq!(
        statuses.map(|status| serde_json::to_value(status).unwrap()),
        [
            json!("pending"),
            json!("approved"),
            json!("rejected"),
            json!("denied"),
            json!("expired"),
            json!("consumed"),
            json!("superseded"),
        ]
    );
}

#[tokio::test]
async fn snapshot_is_scoped_by_agent_and_chat() {
    let authority = ApprovalAuthority::new();

    authority.create_request(request("call-1", "bash")).await;

    let mut other_chat = request("call-2", "read");
    other_chat.owner = ApprovalOwner::chat(ChatAddress::agent("agent-1", "chat-2"));
    authority.create_request(other_chat).await;

    let mut other_agent = request("call-3", "edit");
    other_agent.agent_id = "agent-2".into();
    other_agent.owner = ApprovalOwner::chat(ChatAddress::agent("agent-2", "chat-1"));
    authority.create_request(other_agent).await;

    let snapshot = authority
        .snapshot_at(&rua_core::ChatAddress::agent("agent-1", "chat-1"))
        .await;
    assert_eq!(snapshot.pending.len(), 1);
    assert_eq!(snapshot.pending[0].tool_call_id, "call-1");
}

#[tokio::test]
async fn equal_chat_ids_in_agent_and_project_scopes_do_not_share_approvals() {
    let authority = ApprovalAuthority::new();
    authority
        .create_request(request("agent-call", "bash"))
        .await;

    let mut project_request = request("project-call", "bash");
    project_request.owner = ApprovalOwner::chat(ChatAddress::project("project-1", "chat-1"));
    authority.create_request(project_request).await;

    let agent = authority
        .snapshot_at(&ChatAddress::agent("agent-1", "chat-1"))
        .await;
    let project = authority
        .snapshot_at(&ChatAddress::project("project-1", "chat-1"))
        .await;

    assert_eq!(agent.pending.len(), 1);
    assert_eq!(agent.pending[0].tool_call_id, "agent-call");
    assert_eq!(project.pending.len(), 1);
    assert_eq!(project.pending[0].tool_call_id, "project-call");
}

#[tokio::test]
async fn runtime_session_approval_is_not_addressable_as_a_chat() {
    let authority = ApprovalAuthority::new();
    let owner = ApprovalOwner::runtime_session("agent-1", "acp-session-1");
    let created = authority
        .create_request(ApprovalRequestInput {
            agent_id: "agent-1".into(),
            owner: owner.clone(),
            turn_id: "turn-1".into(),
            tool_call_id: "call-1".into(),
            tool_name: "bash".into(),
            trigger: ApprovalTrigger::approval_mode("requires approval"),
            tool_call: ToolCallSnapshot {
                display: "bash git status".into(),
                args: json!({"command": "git status"}),
            },
        })
        .await;

    assert_eq!(created.owner, owner);
    assert!(authority
        .snapshot_at(&ChatAddress::agent("agent-1", "acp-session-1"))
        .await
        .pending
        .is_empty());
    assert_eq!(
        authority
            .snapshot_for_runtime_session("agent-1", "acp-session-1")
            .await
            .pending
            .len(),
        1
    );
    assert_eq!(
        authority
            .clear_by_address(&ChatAddress::agent("agent-1", "acp-session-1"))
            .await,
        0
    );
    assert_eq!(
        authority
            .clear_by_runtime_session("agent-1", "acp-session-1")
            .await,
        1
    );
}

#[tokio::test]
async fn runtime_session_reuse_does_not_cross_into_chat_or_another_session() {
    let authority = ApprovalAuthority::new();
    let request = authority
        .create_request(ApprovalRequestInput {
            agent_id: "agent-1".into(),
            owner: ApprovalOwner::runtime_session("agent-1", "acp-session-1"),
            turn_id: "turn-1".into(),
            tool_call_id: "call-1".into(),
            tool_name: "bash".into(),
            trigger: ApprovalTrigger::approval_mode("requires approval"),
            tool_call: ToolCallSnapshot {
                display: "bash git status".into(),
                args: json!({"command": "git status"}),
            },
        })
        .await;
    let option = request
        .reuse_options
        .iter()
        .find(|option| option.scope == ApprovalReuseScope::RuntimeSession)
        .expect("runtime session reuse option")
        .clone();

    authority
        .decide(
            &request.id,
            ApprovalDecision::AllowRuntimeSession {
                decided_by: "user".into(),
                matcher: option.matcher.clone(),
            },
        )
        .await
        .expect("allow runtime session");

    assert!(authority
        .find_reuse_rule_for_runtime_session(
            "agent-1",
            "acp-session-1",
            "bash",
            &json!({"command": "git status"}),
        )
        .await
        .is_some());
    assert!(authority
        .find_reuse_rule_for_runtime_session(
            "agent-1",
            "acp-session-2",
            "bash",
            &json!({"command": "git status"}),
        )
        .await
        .is_none());
    assert!(authority
        .find_reuse_rule_at(
            "agent-1",
            &ChatAddress::agent("agent-1", "acp-session-1"),
            "bash",
            &json!({"command": "git status"}),
        )
        .await
        .is_none());
}

#[tokio::test]
async fn decisions_resolve_one_request_without_touching_the_other() {
    let authority = ApprovalAuthority::new();
    let mut events = authority.subscribe();

    let bash = authority.create_request(request("call-bash", "bash")).await;
    let read = authority.create_request(request("call-read", "read")).await;
    let _ = events.recv().await.unwrap();
    let _ = events.recv().await.unwrap();

    let approved = authority
        .decide(
            &bash.id,
            ApprovalDecision::RunOnce {
                decided_by: "user".into(),
            },
        )
        .await
        .expect("approve bash");
    assert_eq!(approved.status, ApprovalStatus::Approved);

    let snapshot = authority
        .snapshot_at(&rua_core::ChatAddress::agent("agent-1", "chat-1"))
        .await;
    assert_eq!(snapshot.pending.len(), 1);
    assert_eq!(snapshot.pending[0].id, read.id);
    assert_eq!(snapshot.resolved.len(), 1);
    assert_eq!(snapshot.resolved[0].id, bash.id);

    let rejected = authority
        .decide(
            &read.id,
            ApprovalDecision::Reject {
                decided_by: "user".into(),
            },
        )
        .await
        .expect("reject read");
    assert_eq!(rejected.status, ApprovalStatus::Rejected);

    let snapshot = authority
        .snapshot_at(&rua_core::ChatAddress::agent("agent-1", "chat-1"))
        .await;
    assert_eq!(snapshot.pending.len(), 0);
    assert_eq!(snapshot.resolved.len(), 2);

    assert_eq!(events.recv().await.unwrap().event, "approval.updated");
    assert_eq!(events.recv().await.unwrap().event, "approval.updated");
}

#[tokio::test]
async fn deciding_a_resolved_request_returns_invalid_state() {
    let authority = ApprovalAuthority::new();
    let bash = authority.create_request(request("call-bash", "bash")).await;

    authority
        .decide(
            &bash.id,
            ApprovalDecision::RunOnce {
                decided_by: "user".into(),
            },
        )
        .await
        .expect("first decision succeeds");

    let err = authority
        .decide(
            &bash.id,
            ApprovalDecision::Reject {
                decided_by: "user".into(),
            },
        )
        .await
        .expect_err("second decision must fail");

    assert_eq!(
        err.to_string(),
        format!("approval request {} is Approved, cannot decide", bash.id)
    );
}

#[tokio::test]
async fn cleared_chat_approval_cannot_be_approved_later() {
    let authority = ApprovalAuthority::new();
    let bash = authority.create_request(request("call-bash", "bash")).await;

    assert_eq!(
        authority
            .clear_by_address(&rua_core::ChatAddress::agent("agent-1", "chat-1"))
            .await,
        1
    );

    let decided = authority
        .wait_for_decision(&bash.id)
        .await
        .expect("superseded request should wake approval waiter");
    assert_eq!(decided.status, ApprovalStatus::Superseded);

    let err = authority
        .decide(
            &bash.id,
            ApprovalDecision::RunOnce {
                decided_by: "late user".into(),
            },
        )
        .await
        .expect_err("superseded approval must not become executable");

    assert_eq!(
        err.to_string(),
        format!("approval request {} is Superseded, cannot decide", bash.id)
    );
    let snapshot = authority
        .snapshot_at(&rua_core::ChatAddress::agent("agent-1", "chat-1"))
        .await;
    assert!(snapshot.pending.is_empty());
    assert_eq!(snapshot.resolved[0].status, ApprovalStatus::Superseded);
}

#[test]
fn balanced_mode_requires_approval_for_bash() {
    assert!(ApprovalModePreset::Balanced.requires_approval_for_tool("bash"));
    assert!(!ApprovalModePreset::Balanced.requires_approval_for_tool("read"));
}

#[tokio::test]
async fn bash_request_suggests_exact_and_generalized_reuse_matchers() {
    let authority = ApprovalAuthority::new();

    let request = authority
        .create_request(bash_request(
            "call-du",
            "du -sh /Users/example/projects/rua 2>/dev/null",
        ))
        .await;

    assert!(
        request.reuse_options.iter().any(|option| {
            option.scope == ApprovalReuseScope::Chat
                && option.label
                    == "Allow bash(du -sh /Users/example/projects/rua 2>/dev/null) in this chat"
                && option.matcher
                    == ApprovalReuseMatcher::ToolCall {
                        tool_name: "bash".into(),
                        input_params: Some("du -sh /Users/example/projects/rua 2>/dev/null".into()),
                    }
        }),
        "request should include exact chat-scope option: {:?}",
        request.reuse_options
    );
    assert!(
        request.reuse_options.iter().any(|option| {
            option.scope == ApprovalReuseScope::Agent
                && option.label == "Always allow bash(du -sh *) for this agent"
                && option.matcher
                    == ApprovalReuseMatcher::ToolCall {
                        tool_name: "bash".into(),
                        input_params: Some("du -sh *".into()),
                    }
        }),
        "request should include generalized agent-scope option: {:?}",
        request.reuse_options
    );
}

#[tokio::test]
async fn bash_pattern_reuse_matches_same_command_family_without_git_status_hardcode() {
    let authority = ApprovalAuthority::new();
    let request = authority
        .create_request(bash_request(
            "call-du",
            "du -sh /Users/example/projects/rua 2>/dev/null",
        ))
        .await;
    let generalized = request
        .reuse_options
        .iter()
        .find(|option| {
            option.scope == ApprovalReuseScope::Chat
                && option.matcher
                    == ApprovalReuseMatcher::ToolCall {
                        tool_name: "bash".into(),
                        input_params: Some("du -sh *".into()),
                    }
        })
        .expect("generalized du -sh chat option");

    authority
        .decide(
            &request.id,
            ApprovalDecision::AllowChat {
                decided_by: "user".into(),
                matcher: generalized.matcher.clone(),
            },
        )
        .await
        .expect("allow chat");

    let matched = authority
        .find_reuse_rule_at(
            "agent-1",
            &rua_core::ChatAddress::agent("agent-1", "chat-1"),
            "bash",
            &json!({
                "command": "du -sh /Users/example/projects/rua/*/ 2>/dev/null | sort -rh | head -30"
            }),
        )
        .await
        .expect("du -sh wildcard rule should match later du -sh command");
    assert_eq!(
        matched.matcher,
        ApprovalReuseMatcher::ToolCall {
            tool_name: "bash".into(),
            input_params: Some("du -sh *".into())
        }
    );
}

#[tokio::test]
async fn allow_chat_creates_reuse_rule_that_matches_only_same_chat() {
    let authority = ApprovalAuthority::new();
    let request = authority
        .create_request(bash_request("call-git-status", "git status --short"))
        .await;

    authority
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

    let same_chat_rule = authority
        .find_reuse_rule_at(
            "agent-1",
            &rua_core::ChatAddress::agent("agent-1", "chat-1"),
            "bash",
            &json!({ "command": "git status" }),
        )
        .await
        .expect("same chat should match");
    assert_eq!(same_chat_rule.scope, ApprovalReuseScope::Chat);
    assert_eq!(
        same_chat_rule.label,
        "Allow bash(git status *) in this chat"
    );

    let other_chat_rule = authority
        .find_reuse_rule_at(
            "agent-1",
            &rua_core::ChatAddress::agent("agent-1", "chat-2"),
            "bash",
            &json!({ "command": "git status" }),
        )
        .await;
    assert!(other_chat_rule.is_none());
}

#[tokio::test]
async fn project_chat_reuse_follows_typed_address_after_agent_switch() {
    let authority = ApprovalAuthority::new();
    let address = ChatAddress::project("project-1", "chat-1");
    let mut input = bash_request("call-git-status", "git status --short");
    input.owner = ApprovalOwner::chat(address.clone());
    let request = authority.create_request(input).await;
    let matcher = ApprovalReuseMatcher::ToolCall {
        tool_name: "bash".into(),
        input_params: Some("git status *".into()),
    };

    authority
        .decide(
            &request.id,
            ApprovalDecision::AllowChat {
                decided_by: "user".into(),
                matcher: matcher.clone(),
            },
        )
        .await
        .expect("allow Project Chat");

    let switched_agent_rule = authority
        .find_reuse_rule_at(
            "agent-2",
            &address,
            "bash",
            &json!({"command": "git status"}),
        )
        .await
        .expect("Project Chat rule should survive Agent switch");
    assert_eq!(switched_agent_rule.scope, ApprovalReuseScope::Chat);
    assert_eq!(
        switched_agent_rule.owner,
        Some(ApprovalOwner::chat(address))
    );
}

#[tokio::test]
async fn equal_chat_uuid_in_other_scope_cannot_reuse_project_chat_rule() {
    let authority = ApprovalAuthority::new();
    let project_address = ChatAddress::project("project-1", "shared-chat");
    let mut input = bash_request("call-git-status", "git status --short");
    input.owner = ApprovalOwner::chat(project_address.clone());
    let request = authority.create_request(input).await;
    let matcher = ApprovalReuseMatcher::ToolCall {
        tool_name: "bash".into(),
        input_params: Some("git status *".into()),
    };
    authority
        .decide(
            &request.id,
            ApprovalDecision::AllowChat {
                decided_by: "user".into(),
                matcher,
            },
        )
        .await
        .expect("allow Project Chat");

    assert!(authority
        .find_reuse_rule_at(
            "agent-1",
            &ChatAddress::agent("agent-1", "shared-chat"),
            "bash",
            &json!({"command": "git status"}),
        )
        .await
        .is_none());
}

#[tokio::test]
async fn deleting_project_chat_rules_preserves_agent_scoped_rules() {
    let authority = ApprovalAuthority::new();
    let address = ChatAddress::project("project-1", "chat-1");
    let mut input = bash_request("call-git-status", "git status --short");
    input.owner = ApprovalOwner::chat(address.clone());
    let request = authority.create_request(input).await;
    let matcher = ApprovalReuseMatcher::ToolCall {
        tool_name: "bash".into(),
        input_params: Some("git status *".into()),
    };
    authority
        .decide(
            &request.id,
            ApprovalDecision::AllowChat {
                decided_by: "user".into(),
                matcher: matcher.clone(),
            },
        )
        .await
        .expect("allow Project Chat");
    let agent_rule = authority
        .create_agent_rule(
            "agent-1",
            PermissionRuleInput {
                effect: PermissionRuleEffect::Allow,
                enabled: true,
                label: "Allow git status for Agent".into(),
                matcher,
            },
        )
        .await
        .expect("create Agent rule");

    authority
        .delete_chat_rules(&address)
        .await
        .expect("delete Project Chat rules");

    assert!(authority.list_chat_rules(&address).await.is_empty());
    assert!(authority
        .list_agent_rules("agent-1")
        .await
        .iter()
        .any(|rule| rule.id == agent_rule.id));
}

#[tokio::test]
async fn deleting_project_rules_removes_only_that_projects_chat_rules() {
    let authority = ApprovalAuthority::new();
    let project_one = ChatAddress::project("project-1", "chat-1");
    let project_two = ChatAddress::project("project-2", "chat-1");
    for (address, call_id) in [(&project_one, "call-1"), (&project_two, "call-2")] {
        let mut input = bash_request(call_id, "git status --short");
        input.owner = ApprovalOwner::chat(address.clone());
        let request = authority.create_request(input).await;
        authority
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
    }

    authority
        .delete_project_rules(&rua_core::ProjectId::new("project-1"))
        .await
        .expect("delete Project rules");

    assert!(authority.list_chat_rules(&project_one).await.is_empty());
    assert_eq!(authority.list_chat_rules(&project_two).await.len(), 1);
}

#[tokio::test]
async fn archive_style_pending_clear_preserves_project_chat_rules() {
    let authority = ApprovalAuthority::new();
    let address = ChatAddress::project("project-1", "chat-1");
    let mut reusable = bash_request("call-reusable", "git status --short");
    reusable.owner = ApprovalOwner::chat(address.clone());
    let reusable = authority.create_request(reusable).await;
    authority
        .decide(
            &reusable.id,
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
    let mut pending = bash_request("call-pending", "git diff --stat");
    pending.owner = ApprovalOwner::chat(address.clone());
    authority.create_request(pending).await;

    assert_eq!(authority.clear_by_address(&address).await, 1);

    assert_eq!(authority.list_chat_rules(&address).await.len(), 1);
    assert!(authority
        .find_reuse_rule_at(
            "agent-2",
            &address,
            "bash",
            &json!({"command": "git status"}),
        )
        .await
        .is_some());
}
