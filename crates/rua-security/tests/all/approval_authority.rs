use rua_core::ChatAddress;
use rua_security::approval_authority::{
    ApprovalAuthority, ApprovalAuthorityError, ApprovalDecision, ApprovalModePreset, ApprovalOwner,
    ApprovalRequest, ApprovalRequestInput, ApprovalReuseMatcher, ApprovalReuseScope,
    ApprovalStatus, ApprovalTrigger, AuthenticatedApprovalDecision, AuthenticatedUserDecision,
    PermissionRuleEffect, PermissionRuleInput, ToolCallSnapshot,
};
use serde_json::json;

fn request(tool_call_id: &str, tool_name: &str) -> ApprovalRequestInput {
    ApprovalRequestInput {
        agent_id: "agent-1".into(),
        owner: rua_security::ApprovalOwner::chat(ChatAddress::new("chat-1")),
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
        owner: rua_security::ApprovalOwner::chat(ChatAddress::new("chat-1")),
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

fn chat_file_read(
    args: serde_json::Value,
) -> (ApprovalRequestInput, rua_security::ToolCallContext) {
    let mut input = request("read-1", "read");
    input.tool_call.args = args.clone();
    let mut ctx = rua_security::ToolCallContext::new(&input.agent_id, "chat-1", "read", args)
        .with_chat_address(ChatAddress::new("chat-1"));
    ctx.metadata.insert("turn_id".into(), json!(input.turn_id));
    ctx.metadata
        .insert("tool_call_id".into(), json!(input.tool_call_id));
    (input, ctx)
}

async fn allow_read_from_card(
    authority: &ApprovalAuthority,
    selected: serde_json::Value,
    scope: ApprovalReuseScope,
) {
    let (request, _) = chat_file_read(selected);
    let pending = authority.create_request(request).await;
    let matcher = pending
        .reuse_options
        .iter()
        .find(|option| option.scope == scope)
        .expect("persistent option for this file")
        .matcher
        .clone();
    let decision = match scope {
        ApprovalReuseScope::Chat => ApprovalDecision::AllowChat {
            decided_by: "webui".into(),
            matcher,
        },
        ApprovalReuseScope::Agent => ApprovalDecision::AllowAgent {
            decided_by: "webui".into(),
            matcher,
        },
        ApprovalReuseScope::RuntimeSession => unreachable!(),
    };
    authority
        .decide_for_authenticated_user(&pending.agent_id, &pending.id, decision, "user")
        .await
        .unwrap();
}

fn manual_read_rule(
    effect: PermissionRuleEffect,
    pattern: &serde_json::Value,
) -> PermissionRuleInput {
    PermissionRuleInput {
        effect,
        enabled: true,
        label: "Fixture file pattern".into(),
        matcher: ApprovalReuseMatcher::ToolCall {
            tool_name: "read".into(),
            input_params: Some(pattern.to_string()),
        },
    }
}

fn open_file_rule_module(root: &std::path::Path) -> rua_security::SecurityModule {
    rua_security::SecurityModule::new(
        std::sync::Arc::new(rua_security::SecurityConfigStore::new(root.to_owned())),
        None,
    )
}

#[tokio::test]
async fn file_read_reuse_from_the_approval_card_keeps_the_selected_name_literal() {
    use rua_security::security_module::FileReadAdmission::{Allowed, RequiresUserConfirmation};
    let selected = json!({"device_id": "phone", "file_path": "/fixture/card-*.txt"});
    for scope in [ApprovalReuseScope::Chat, ApprovalReuseScope::Agent] {
        let tmp = tempfile::tempdir().unwrap();
        let module = open_file_rule_module(tmp.path());
        let mut config = module.config_store().load("agent-1").unwrap();
        config.tool_guard.approval_mode = "ask_first".into();
        module.save_and_invalidate("agent-1", &config).unwrap();
        allow_read_from_card(module.approval_authority(), selected.clone(), scope).await;
        let captured = module.config_store().load("agent-1").unwrap();
        for (args, expected) in [
            (selected.clone(), Allowed),
            (
                json!({"device_id": "tablet", "file_path": "/fixture/card-*.txt"}),
                RequiresUserConfirmation,
            ),
            (
                json!({"device_id": "phone", "file_path": "/fixture/card-1.txt"}),
                RequiresUserConfirmation,
            ),
        ] {
            let (_, ctx) = chat_file_read(args.clone());
            let path = args["file_path"].as_str().unwrap();
            let result = module
                .inspect_chat_file_read(&ctx, &captured, &[path])
                .await
                .unwrap();
            assert_eq!(result, expected, "{scope:?}: {args}");
        }
    }
}

#[tokio::test]
async fn file_read_checks_protected_resolved_and_backing_names_with_an_existing_allow() {
    use rua_security::security_module::{
        FileReadAdmission::{Allowed, RequiresUserConfirmation},
        FileReadPolicyError,
    };

    let tmp = tempfile::tempdir().unwrap();
    let module = open_file_rule_module(tmp.path());
    let requested = "/fixture/work/notes.txt";
    let resolved = "/fixture/shared/notes.txt";
    let backing = "/fixture/mobile/volume/notes.txt";
    let names = [requested, resolved, backing];
    let args = json!({"device_id": "phone", "file_path": requested});
    let (_, mut ctx) = chat_file_read(args.clone());
    ctx.local_device_id = Some("phone".into());
    let captured = module.config_store().load("agent-1").unwrap();
    let inspect = || module.inspect_chat_file_read(&ctx, &captured, &names);
    assert_eq!(inspect().await.unwrap(), Allowed);
    let mut config = captured.clone();
    config.tool_guard.approval_mode = "ask_first".into();
    module.save_and_invalidate("agent-1", &config).unwrap();
    assert_eq!(inspect().await.unwrap(), RequiresUserConfirmation);
    allow_read_from_card(module.approval_authority(), args, ApprovalReuseScope::Chat).await;
    assert_eq!(inspect().await.unwrap(), Allowed);

    for (protected_name, expected) in [
        (requested, Allowed),
        (resolved, RequiresUserConfirmation),
        (backing, RequiresUserConfirmation),
    ] {
        let mut protected = config.clone();
        protected
            .file_guard
            .sensitive_files
            .push(protected_name.into());
        module.save_and_invalidate("agent-1", &protected).unwrap();
        assert_eq!(inspect().await.unwrap(), expected, "{protected_name}");
    }
    let backing_args = json!({"device_id": "phone", "file_path": backing});
    let authority = module.approval_authority();
    authority
        .create_agent_rule(
            "agent-1",
            manual_read_rule(PermissionRuleEffect::Allow, &backing_args),
        )
        .await
        .unwrap();
    assert_eq!(inspect().await.unwrap(), Allowed);
    authority
        .create_agent_rule(
            "agent-1",
            manual_read_rule(PermissionRuleEffect::Deny, &backing_args),
        )
        .await
        .unwrap();
    assert!(matches!(
        inspect().await,
        Err(FileReadPolicyError::Denied(_))
    ));
    assert!(authority
        .snapshot_at(&ChatAddress::new("chat-1"))
        .await
        .pending
        .is_empty());
}

#[tokio::test]
async fn file_read_reuse_from_a_manual_pattern_matches_the_named_device_and_file_pattern() {
    let authority = ApprovalAuthority::new();
    let address = ChatAddress::new("chat-1");
    let pattern = json!({"device_id": "phone", "file_path": "/fixture/card-*.txt"});
    let rule = authority
        .create_agent_rule(
            "agent-1",
            manual_read_rule(PermissionRuleEffect::Allow, &pattern),
        )
        .await
        .unwrap();
    for (device, path, expected) in [
        ("phone", "/fixture/card-1.txt", Some(&rule.id)),
        ("phone", "/fixture/card-2.txt", Some(&rule.id)),
        ("tablet", "/fixture/card-1.txt", None),
        ("phone", "/fixture/note.txt", None),
    ] {
        let args = json!({"device_id": device, "file_path": path});
        let reused = authority
            .find_reuse_rule_at("agent-1", &address, "read", &args)
            .await;
        assert_eq!(reused.as_ref().map(|r| &r.id), expected, "{args}");
    }
}

#[tokio::test]
async fn file_read_reuse_from_the_card_preserves_manual_deny_precedence_after_restart() {
    use rua_security::SecurityVerdict;
    let tmp = tempfile::tempdir().unwrap();
    let address = ChatAddress::new("chat-1");
    let selected = json!({"device_id": "phone", "file_path": "/fixture/card-*.txt"});
    let sibling = json!({"device_id": "phone", "file_path": "/fixture/card-1.txt"});
    let blocked = {
        let module = open_file_rule_module(tmp.path());
        let authority = module.approval_authority();
        let rule = authority
            .create_agent_rule(
                "agent-1",
                manual_read_rule(PermissionRuleEffect::Deny, &selected),
            )
            .await
            .unwrap();
        allow_read_from_card(authority, selected.clone(), ApprovalReuseScope::Agent).await;
        rule
    };

    let reopened = open_file_rule_module(tmp.path());
    let authority = reopened.approval_authority();
    assert_eq!(authority.list_agent_rules("agent-1").await.len(), 2);
    for args in [&selected, &sibling] {
        let (_, ctx) = chat_file_read(args.clone());
        let verdict = reopened.enforce(&ctx, &[]).await;
        assert!(
            matches!(&verdict, SecurityVerdict::Deny { reason } if reason.contains(&blocked.label)),
            "{args}: {verdict:?}"
        );
    }
    assert!(authority
        .find_reuse_rule_at("agent-1", &address, "read", &selected)
        .await
        .is_some());
    assert!(authority
        .find_reuse_rule_at("agent-1", &address, "read", &sibling)
        .await
        .is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn authenticated_read_claim_matches_the_entire_invocation_and_is_consumed_once() {
    use std::sync::Arc;
    use tokio::sync::Barrier;
    const FILE: &str = "/selected/note.txt";
    let authority = Arc::new(ApprovalAuthority::new());
    let (request, ctx) = chat_file_read(json!({"device_id":"phone","file_path":FILE,"limit":3}));
    let pending = authority.create_request(request).await;
    let approved = authority
        .decide_for_authenticated_user(
            &pending.agent_id,
            &pending.id,
            ApprovalDecision::RunOnce {
                decided_by: "console".into(),
            },
            "alice",
        )
        .await
        .unwrap();
    let mut updates = authority.subscribe();

    for mismatch in [
        "agent", "chat", "owner", "turn", "call", "tool", "limit", "device", "path",
    ] {
        let mut other = ctx.clone();
        match mismatch {
            "agent" => other.agent_id.push('2'),
            "chat" => other.approval_owner = ApprovalOwner::chat(ChatAddress::new("chat-2")),
            "owner" => other.approval_owner = ApprovalOwner::runtime_session("agent-1", "chat-1"),
            "turn" => {
                other.metadata.insert("turn_id".into(), json!("turn-2"));
            }
            "call" => {
                other
                    .metadata
                    .insert("tool_call_id".into(), json!("read-2"));
            }
            "tool" => other.tool_name = "edit".into(),
            "limit" => other.tool_input["limit"] = json!(4),
            "device" => other.tool_input["device_id"] = json!("desktop"),
            "path" => other.tool_input["file_path"] = json!("/other/note.txt"),
            _ => unreachable!(),
        }
        assert_eq!(
            authority
                .consume_authenticated_read_once(
                    &other,
                    other.tool_input["device_id"].as_str().unwrap(),
                    other.tool_input["file_path"].as_str().unwrap()
                )
                .await,
            None,
            "{mismatch}"
        );
    }
    assert_eq!(
        authority
            .consume_authenticated_read_once(&ctx, "tablet", FILE)
            .await,
        None
    );
    assert_eq!(
        authority
            .consume_authenticated_read_once(&ctx, "phone", "/another/file")
            .await,
        None
    );
    assert_eq!(authority.request(&pending.id).await, Some(approved.clone()));

    let start = Arc::new(Barrier::new(2));
    let claim = || {
        let authority = Arc::clone(&authority);
        let start = Arc::clone(&start);
        let ctx = ctx.clone();
        tokio::spawn(async move {
            start.wait().await;
            authority
                .consume_authenticated_read_once(&ctx, "phone", FILE)
                .await
        })
    };
    let (one, two) = tokio::join!(claim(), claim());
    assert_eq!(
        [one.unwrap(), two.unwrap()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>(),
        std::slice::from_ref(&pending.id)
    );
    let consumed = authority.request(&pending.id).await.unwrap();
    assert_eq!(consumed.status, ApprovalStatus::Consumed);
    assert_eq!(
        consumed.authenticated_user_decision,
        approved.authenticated_user_decision
    );
    assert_eq!(updates.recv().await.unwrap().request, consumed);
    assert!(matches!(
        updates.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
}

#[tokio::test]
async fn authenticated_read_claim_excludes_directory_paging_media_and_nonlocal_paths() {
    for (path, extras) in [
        ("/folder", json!({"after":"a.txt"})),
        ("media/704d76eb-dee7-47d7-bef3-915891f200db", json!({})),
        ("relative.txt", json!({})),
        (r"\\server\share\note.txt", json!({})),
    ] {
        let authority = ApprovalAuthority::new();
        let mut args = extras;
        args["device_id"] = json!("phone");
        args["file_path"] = json!(path);
        let (request, ctx) = chat_file_read(args);
        let pending = authority.create_request(request).await;
        let approved = authority
            .decide_for_authenticated_user(
                &ctx.agent_id,
                &pending.id,
                ApprovalDecision::RunOnce {
                    decided_by: "console".into(),
                },
                "alice",
            )
            .await
            .unwrap();
        assert_eq!(
            authority
                .consume_authenticated_read_once(&ctx, "phone", path)
                .await,
            None
        );
        assert_eq!(authority.request(&pending.id).await.unwrap(), approved);
    }
}

#[tokio::test]
async fn authenticated_read_claim_does_not_convert_hook_or_reusable_approval_to_run_once() {
    let path = "/selected/note.txt";
    for reuse in [false, true] {
        let authority = ApprovalAuthority::new();
        let (request, ctx) = chat_file_read(json!({"device_id":"phone","file_path":path}));
        let pending = authority.create_request(request).await;
        let approved = if reuse {
            authority
                .decide_for_authenticated_user(
                    &pending.agent_id,
                    &pending.id,
                    ApprovalDecision::AllowChat {
                        decided_by: "console".into(),
                        matcher: pending.reuse_options[0].matcher.clone(),
                    },
                    "alice",
                )
                .await
        } else {
            authority
                .decide(
                    &pending.id,
                    ApprovalDecision::RunOnce {
                        decided_by: "product_hook:auto".into(),
                    },
                )
                .await
        }
        .unwrap();
        assert_eq!(
            authority
                .consume_authenticated_read_once(&ctx, "phone", path)
                .await,
            None
        );
        assert_eq!(authority.request(&pending.id).await.unwrap(), approved);
    }
}

#[tokio::test]
async fn authenticated_read_claim_requires_one_unambiguous_approval_request() {
    let authority = ApprovalAuthority::new();
    let path = "/selected/note.txt";
    let (input, ctx) = chat_file_read(json!({"device_id":"phone","file_path":path}));
    for _ in 0..2 {
        let pending = authority.create_request(input.clone()).await;
        authority
            .decide_for_authenticated_user(
                &ctx.agent_id,
                &pending.id,
                ApprovalDecision::RunOnce {
                    decided_by: "console".into(),
                },
                "alice",
            )
            .await
            .unwrap();
    }
    assert_eq!(
        authority
            .consume_authenticated_read_once(&ctx, "phone", path)
            .await,
        None
    );
    let snapshot = authority.snapshot_for_owner(&ctx.approval_owner).await;
    assert_eq!(snapshot.resolved.len(), 2);
    assert!(snapshot
        .resolved
        .iter()
        .all(|r| r.status == ApprovalStatus::Approved));
}

#[tokio::test]
async fn approval_labels_and_permission_hooks_do_not_record_an_authenticated_user() {
    let authority = ApprovalAuthority::new();
    for (label, agent_scoped) in [("alice", true), ("product_hook:permission-1", false)] {
        let mut input = serde_json::to_value(request(label, "read")).unwrap();
        input["authenticated_user_decision"] = json!({"user_id":"alice","decision":"run_once"});
        let pending = authority
            .create_request(serde_json::from_value(input).unwrap())
            .await;
        assert!(pending.authenticated_user_decision.is_none());
        let decision = ApprovalDecision::RunOnce {
            decided_by: label.into(),
        };
        let approved = if agent_scoped {
            authority
                .decide_for_agent(&pending.agent_id, &pending.id, decision)
                .await
        } else {
            authority.decide(&pending.id, decision).await
        }
        .unwrap();
        assert_eq!(approved.decided_by.as_deref(), Some(label));
        assert!(approved.authenticated_user_decision.is_none());
        let legacy = serde_json::to_value(&approved).unwrap();
        assert!(legacy.get("authenticated_user_decision").is_none());
        assert_eq!(
            serde_json::from_value::<ApprovalRequest>(legacy).unwrap(),
            approved
        );
        let repeated = authority
            .decide_for_authenticated_user(
                &pending.agent_id,
                &pending.id,
                ApprovalDecision::RunOnce {
                    decided_by: "webui".into(),
                },
                "alice",
            )
            .await;
        assert!(matches!(
            repeated,
            Err(ApprovalAuthorityError::InvalidState { .. })
        ));
        assert_eq!(authority.request(&pending.id).await.unwrap(), approved);
    }
}

#[tokio::test]
async fn authenticated_run_once_records_the_verified_user_on_the_exact_pending_request() {
    let authority = ApprovalAuthority::new();
    let pending = authority.create_request(request("call-1", "read")).await;
    let unassigned = authority
        .create_request(ApprovalRequestInput {
            agent_id: String::new(),
            ..request("unassigned", "read")
        })
        .await;
    let run_once = ApprovalDecision::RunOnce {
        decided_by: "client_label".into(),
    };
    for (agent, id) in [
        ("", unassigned.id.as_str()),
        ("other-agent", pending.id.as_str()),
        (&pending.agent_id, "absent"),
    ] {
        assert!(matches!(
            authority
                .decide_for_authenticated_user(agent, id, run_once.clone(), "alice")
                .await,
            Err(ApprovalAuthorityError::NotFound(_))
        ));
    }
    assert!(authority
        .decide_for_authenticated_user(&pending.agent_id, &pending.id, run_once.clone(), "  ")
        .await
        .is_err());
    assert_eq!(authority.request(&pending.id).await.unwrap(), pending);

    let mut updates = authority.subscribe();
    let approved = authority
        .decide_for_authenticated_user(&pending.agent_id, &pending.id, run_once, "alice")
        .await
        .unwrap();
    assert_eq!(approved.status, ApprovalStatus::Approved);
    assert_eq!(
        approved.authenticated_user_decision,
        Some(AuthenticatedUserDecision {
            user_id: "alice".into(),
            decision: AuthenticatedApprovalDecision::RunOnce,
            extra: Default::default(),
        })
    );
    assert_eq!(updates.recv().await.unwrap().request, approved);
    assert_eq!(
        authority.wait_for_decision(&pending.id).await.unwrap(),
        approved
    );
    assert_eq!(
        serde_json::to_value(&approved).unwrap()["authenticated_user_decision"],
        json!({"user_id":"alice","decision":"run_once"})
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_authenticated_decisions_publish_only_the_winning_identity_and_action() {
    use std::sync::Arc;
    use tokio::sync::Barrier;

    let authority = Arc::new(ApprovalAuthority::new());
    let pending = authority.create_request(request("call-1", "read")).await;
    let mut updates = authority.subscribe();
    let barrier = Arc::new(Barrier::new(2));
    let decide = |user, decision| {
        let authority = Arc::clone(&authority);
        let barrier = Arc::clone(&barrier);
        let pending = pending.clone();
        tokio::spawn(async move {
            barrier.wait().await;
            authority
                .decide_for_authenticated_user(&pending.agent_id, &pending.id, decision, user)
                .await
        })
    };
    let (one, two) = tokio::join!(
        decide(
            "alice",
            ApprovalDecision::RunOnce {
                decided_by: "webui".into()
            }
        ),
        decide(
            "bob",
            ApprovalDecision::Reject {
                decided_by: "webui".into()
            }
        )
    );
    let (winner, user, kind, status) = match (one.unwrap(), two.unwrap()) {
        (Ok(winner), Err(ApprovalAuthorityError::InvalidState { .. })) => (
            winner,
            "alice",
            AuthenticatedApprovalDecision::RunOnce,
            ApprovalStatus::Approved,
        ),
        (Err(ApprovalAuthorityError::InvalidState { .. }), Ok(winner)) => (
            winner,
            "bob",
            AuthenticatedApprovalDecision::Reject,
            ApprovalStatus::Rejected,
        ),
        outcomes => panic!("expected exactly one successful decision: {outcomes:?}"),
    };
    assert_eq!(winner.status, status);
    assert_eq!(
        winner.authenticated_user_decision,
        Some(AuthenticatedUserDecision {
            user_id: user.into(),
            decision: kind,
            extra: Default::default(),
        })
    );
    assert_eq!(authority.request(&pending.id).await.unwrap(), winner);
    assert_eq!(updates.recv().await.unwrap().request, winner);
    assert!(matches!(
        updates.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
}

#[tokio::test]
async fn switching_one_chat_serializes_pending_approval_creation_only_for_that_chat() {
    let authority = ApprovalAuthority::new();
    let address = ChatAddress::new("chat-1");
    let transition = authority.hold_chat_registrations(&address).await;
    let pending_task = authority.create_request(request("task:old:attempt:1", "edit"));
    tokio::pin!(pending_task);
    tokio::select! {
        biased;
        _ = &mut pending_task => panic!("approval registration bypassed this Chat's switch"),
        () = async {} => {}
    }
    for independent_owner in [
        ApprovalOwner::chat(ChatAddress::new("other")),
        ApprovalOwner::runtime_session("agent-1", "chat-1"),
    ] {
        let request = authority
            .create_request(ApprovalRequestInput {
                owner: independent_owner.clone(),
                ..request("independent", "edit")
            })
            .await;
        assert_eq!(request.owner, independent_owner);
    }
    assert!(authority
        .list_pending_for_address(&address)
        .await
        .is_empty());
    drop(transition);
    let pending = pending_task.await;
    let _next_switch = authority.hold_chat_registrations(&address).await;
    assert_eq!(
        authority.list_pending_for_address(&address).await,
        vec![pending.clone()]
    );
    assert_eq!(pending.agent_id, "agent-1");
    assert_eq!(pending.owner, ApprovalOwner::chat(address));
}

#[tokio::test]
async fn snapshot_keeps_multiple_pending_requests_for_one_chat() {
    let authority = ApprovalAuthority::new();
    let mut events = authority.subscribe();

    let bash = authority.create_request(request("call-bash", "bash")).await;
    let read = authority.create_request(request("call-read", "read")).await;

    assert_ne!(bash.id, read.id);

    let snapshot = authority
        .snapshot_at(&rua_core::ChatAddress::new("chat-1"))
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
async fn snapshot_includes_the_same_chat_across_agents() {
    let authority = ApprovalAuthority::new();

    authority.create_request(request("call-1", "bash")).await;

    let mut other_chat = request("call-2", "read");
    other_chat.owner = ApprovalOwner::chat(ChatAddress::new("chat-2"));
    authority.create_request(other_chat).await;

    let mut other_agent = request("call-3", "edit");
    other_agent.agent_id = "agent-2".into();
    other_agent.owner = ApprovalOwner::chat(ChatAddress::new("chat-1"));
    authority.create_request(other_agent).await;

    let snapshot = authority
        .snapshot_at(&rua_core::ChatAddress::new("chat-1"))
        .await;
    let mut ids = snapshot
        .pending
        .iter()
        .map(|r| r.tool_call_id.as_str())
        .collect::<Vec<_>>();
    ids.sort();
    assert_eq!(ids, ["call-1", "call-3"]);
}

#[tokio::test]
async fn distinct_chats_do_not_share_approvals() {
    let authority = ApprovalAuthority::new();
    authority
        .create_request(request("agent-call", "bash"))
        .await;

    let mut project_request = request("project-call", "bash");
    project_request.owner = ApprovalOwner::chat(ChatAddress::new("chat-2"));
    authority.create_request(project_request).await;

    let agent = authority.snapshot_at(&ChatAddress::new("chat-1")).await;
    let project = authority.snapshot_at(&ChatAddress::new("chat-2")).await;

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
        .snapshot_at(&ChatAddress::new("acp-session-1"))
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
            .clear_by_address(&ChatAddress::new("acp-session-1"))
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
            &ChatAddress::new("acp-session-1"),
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
        .snapshot_at(&rua_core::ChatAddress::new("chat-1"))
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
        .snapshot_at(&rua_core::ChatAddress::new("chat-1"))
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
            .clear_by_address(&rua_core::ChatAddress::new("chat-1"))
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
        .snapshot_at(&rua_core::ChatAddress::new("chat-1"))
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
            &rua_core::ChatAddress::new("chat-1"),
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

    let approved = authority
        .decide_for_authenticated_user(
            &request.agent_id,
            &request.id,
            ApprovalDecision::AllowChat {
                decided_by: "user".into(),
                matcher: ApprovalReuseMatcher::ToolCall {
                    tool_name: "bash".into(),
                    input_params: Some("git status *".into()),
                },
            },
            "alice",
        )
        .await
        .expect("allow chat");
    assert_eq!(
        approved.authenticated_user_decision.unwrap().decision,
        AuthenticatedApprovalDecision::AllowChat
    );

    let same_chat_rule = authority
        .find_reuse_rule_at(
            "agent-1",
            &rua_core::ChatAddress::new("chat-1"),
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
            &rua_core::ChatAddress::new("chat-2"),
            "bash",
            &json!({ "command": "git status" }),
        )
        .await;
    assert!(other_chat_rule.is_none());
}

#[tokio::test]
async fn project_chat_reuse_follows_typed_address_after_agent_switch() {
    let authority = ApprovalAuthority::new();
    let address = ChatAddress::new("chat-1");
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
async fn different_chat_cannot_reuse_chat_rule() {
    let authority = ApprovalAuthority::new();
    let project_address = ChatAddress::new("shared-chat");
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
            &ChatAddress::new("another-chat"),
            "bash",
            &json!({"command": "git status"}),
        )
        .await
        .is_none());
}

#[tokio::test]
async fn deleting_project_chat_rules_preserves_agent_scoped_rules() {
    let authority = ApprovalAuthority::new();
    let address = ChatAddress::new("chat-1");
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
async fn deleting_member_chat_rules_preserves_other_chats() {
    let authority = ApprovalAuthority::new();
    let project_one = ChatAddress::new("chat-1");
    let project_two = ChatAddress::new("chat-2");
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
        .delete_chat_rules(&project_one)
        .await
        .expect("delete Project rules");

    assert!(authority.list_chat_rules(&project_one).await.is_empty());
    assert_eq!(authority.list_chat_rules(&project_two).await.len(), 1);
}

#[tokio::test]
async fn archive_style_pending_clear_preserves_project_chat_rules() {
    let authority = ApprovalAuthority::new();
    let address = ChatAddress::new("chat-1");
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

#[tokio::test]
async fn confirmed_preapproved_read_satisfies_ask_first_and_preserves_path_guards() {
    use rua_security::security_module::{
        FileReadAdmission::{Allowed, RequiresUserConfirmation},
        FileReadPolicyError,
    };

    let dir = tempfile::tempdir().unwrap();
    let security = open_file_rule_module(dir.path());
    let requested = "/fixture/public/note.txt";
    let backing = "/fixture/volume/note.txt";
    let names = [requested, backing];
    let args = json!({"device_id":"phone","file_path":requested});
    let (_, mut ctx) = chat_file_read(args.clone());
    ctx.local_device_id = Some("phone".into());
    ctx = ctx.with_chat_address(ChatAddress::new("second-chat"));
    let mut captured = security.config_store().load(&ctx.agent_id).unwrap();
    captured.tool_guard.approval_mode = "ask_first".into();
    security
        .save_and_invalidate(&ctx.agent_id, &captured)
        .unwrap();
    allow_read_from_card(
        security.approval_authority(),
        args,
        ApprovalReuseScope::Agent,
    )
    .await;

    assert_eq!(
        security
            .inspect_chat_file_read(&ctx, &captured, &names)
            .await
            .unwrap(),
        RequiresUserConfirmation
    );
    assert_eq!(
        security
            .inspect_preapproved_chat_file_read(&ctx, &captured, &names, false)
            .await
            .unwrap(),
        RequiresUserConfirmation
    );
    assert_eq!(
        security
            .inspect_preapproved_chat_file_read(&ctx, &captured, &names, true)
            .await
            .unwrap(),
        Allowed
    );

    let mut protected = captured.clone();
    protected.file_guard.sensitive_files.push(backing.into());
    assert_eq!(
        security
            .inspect_preapproved_chat_file_read(&ctx, &protected, &names, true)
            .await
            .unwrap(),
        RequiresUserConfirmation
    );
    let mut current = security.config_store().load(&ctx.agent_id).unwrap();
    current.file_guard.sensitive_files.push(backing.into());
    security
        .save_and_invalidate(&ctx.agent_id, &current)
        .unwrap();
    assert_eq!(
        security
            .inspect_preapproved_chat_file_read(&ctx, &captured, &names, true)
            .await
            .unwrap(),
        RequiresUserConfirmation
    );

    security
        .approval_authority()
        .create_agent_rule(
            &ctx.agent_id,
            manual_read_rule(
                PermissionRuleEffect::Deny,
                &json!({"device_id":"phone","file_path":backing}),
            ),
        )
        .await
        .unwrap();
    assert!(matches!(
        security
            .inspect_preapproved_chat_file_read(&ctx, &captured, &names, true)
            .await,
        Err(FileReadPolicyError::Denied(_))
    ));
}

#[tokio::test]
async fn an_invocation_confirmation_still_requires_the_exact_local_file_approval() {
    use rua_security::security_module::FileReadPolicyError;
    let dir = tempfile::tempdir().unwrap();
    let security = open_file_rule_module(dir.path());
    let path = "/fixture/note.txt";
    let args = json!({"device_id":"phone","file_path":path,"limit":1});
    let (_, mut ctx) = chat_file_read(args.clone());
    ctx.local_device_id = Some("phone".into());
    let config = security.config_store().load(&ctx.agent_id).unwrap();
    assert!(matches!(
        security
            .inspect_preapproved_chat_file_read(&ctx, &config, &[path], true)
            .await,
        Err(FileReadPolicyError::Denied(_))
    ));
    allow_read_from_card(
        security.approval_authority(),
        args,
        ApprovalReuseScope::Agent,
    )
    .await;
    ctx.tool_input["limit"] = 2.into();
    assert!(matches!(
        security
            .inspect_preapproved_chat_file_read(&ctx, &config, &[path], true)
            .await,
        Err(FileReadPolicyError::Denied(_))
    ));
}
