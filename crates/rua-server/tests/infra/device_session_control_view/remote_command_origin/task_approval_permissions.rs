use super::*;
use rua_security::approval_authority::{
    ApprovalReuseScope, ApprovalStatus, AuthenticatedApprovalDecision,
};
use rua_server::services::device_command_bus::ConversationApprovalDecision;
use rua_tool::extended::task_tools::TaskCreateTool;

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_task_chat_permission_commits_at_authority_before_target_release() {
    let fixture = RemoteInputConfirmationFixture::new(Duration::ZERO, false).await;
    // The production default is hands_off. Capture ask_first before creating the
    // Chat so this scenario exercises a real pending Task approval.
    for state in [&fixture.source_state, &fixture.target_state] {
        let mut config = state.security.config_store().load("default").unwrap();
        config.tool_guard.approval_mode = "ask_first".into();
        state
            .security
            .config_store()
            .save("default", &config)
            .unwrap();
    }
    let cwd = fixture
        .source_state
        .rua_root
        .canonicalize()
        .unwrap()
        .join("task-chat-permission");
    std::fs::create_dir(&cwd).unwrap();
    let call = ToolCall::new(
        "task-chat-permission",
        "task_create",
        json!({
            "runner":{"type":"tool","tool_name":"bash","arguments":{
                "device_id":fixture.source.device_id,"cwd":cwd,"command":"printf once >> starts"
            }}
        }),
    );
    let f = Origin::new(fixture, std::slice::from_ref(&call)).await;
    let target_approvals = f.fixture.source_state.security.approval_authority();
    for rule in target_approvals.list_agent_rules("default").await {
        target_approvals
            .delete_agent_rule("default", &rule.id)
            .await
            .unwrap();
    }
    let result = TaskCreateTool
        .call(call.arguments.clone(), f.context(&call))
        .await
        .unwrap();
    let result: serde_json::Value = serde_json::from_str(&result.text_content()).unwrap();
    let id = result["task_id"].as_str().unwrap();
    let owner = rua_task::TaskOwner::new(&f.fixture.chat_id);
    let card = tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            let snapshot = target_approvals.snapshot_at(&owner.chat_address).await;
            if let Some(card) = snapshot
                .pending
                .into_iter()
                .find(|card| card.tool_call_id == format!("task:{id}:attempt:1:tool"))
            {
                if f.fixture
                    .source_state
                    .task_runtime
                    .get(&owner, id)
                    .await
                    .unwrap()
                    .state
                    == rua_task::TaskState::WaitingApproval
                {
                    break card;
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    let card = match card {
        Ok(card) => card,
        Err(error) => panic!(
            "target Task requires its original approval: {error}; record={:?}; approvals={:?}",
            f.fixture.source_state.task_runtime.get(&owner, id).await,
            target_approvals.snapshot_at(&owner.chat_address).await
        ),
    };
    let matcher = card
        .reuse_options
        .iter()
        .find(|option| option.scope == ApprovalReuseScope::Chat)
        .unwrap()
        .matcher
        .clone();
    let decision = ConversationApprovalDecision::AllowChat {
        decided_by: "webui".into(),
        matcher,
    };
    // The user is connected to B, while A remains the Chat Authority. The
    // service must route B -> A -> the same pending Task card on B.
    let entry = seeded_chat_task_service(&f.fixture.source_state);
    let canonical = f.fixture.target_state.security.approval_authority();
    let canonical_workspace = f.fixture.target_state.chat_workspace().as_path().to_owned();
    let canonical_store = WorkspaceSessionStore::new(&canonical_workspace);
    let rules_path = canonical_store
        .chat_dir(&f.fixture.chat_id)
        .join("permission_rules.json");
    let header = canonical_store
        .load_chat_header(&f.fixture.chat_id)
        .await
        .unwrap()
        .unwrap();
    let epoch = rua_server::services::conversation_authority_placement::parse_conversation_authority_placement(
        &serde_json::Value::Object(header.extra.clone()),
    ).unwrap().authority_epoch;
    // A paired Device cannot claim that a different Device committed this rule.
    for (sender, claimed_epoch) in [
        (f.fixture.source.device_id.clone(), epoch),
        (f.fixture.target.device_id.clone(), rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::next_for_device(&f.fixture.target.device_id, Some(epoch))),
    ] {
        let error = entry.handle(rua_server::services::chat_task::ChatTaskCommand {
            source_device_id: sender,
            target_device_id: f.fixture.source.device_id.clone(),
            owner: owner.clone(),
            action: rua_server::services::chat_task::ChatTaskAction::ApprovalApplyAtExecution {
                task_id: id.into(), attempt_no: 1,
                authority_device_id: f.fixture.target.device_id.clone(), authority_epoch: claimed_epoch,
                original: Box::new(card.clone()), decided: Box::new(card.clone()),
            },
            extra: Default::default(),
        }).await.unwrap_err();
        assert!(error.contains("current Chat Authority"), "{error}");
    }
    // A late Task card must not create a reusable Chat permission while the
    // canonical writer is frozen, even when the decision entered through B.
    let frozen = rua_session::chat_handoff::ChatHandoffRecord {
        handoff_id: "approval-handoff-fence".into(),
        chat_id: f.fixture.chat_id.clone(),
        old_placement: header.extra["authority_placement"].clone(),
        new_placement: json!({
            "device_id": f.fixture.source.device_id,
            "authority_epoch": rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::next_for_device(&f.fixture.source.device_id, Some(epoch)),
            "base_manifest_hash": header.extra["authority_placement"]["base_manifest_hash"],
        }),
        configuration: Some(header.extra["hook_configuration"].clone()),
        candidate_configuration: None,
        requirements: json!({}),
        phase: rua_session::chat_handoff::ChatHandoffPhase::Prepared,
        extra: Default::default(),
    };
    canonical_store
        .prepare_chat_handoff(&frozen, None)
        .await
        .unwrap();
    let error = entry
        .decide_task_approval(&owner, id, 1, &card.id, decision.clone(), "logged-in-user")
        .await
        .unwrap_err();
    assert!(error.contains("handoff"), "{error}");
    assert!(canonical
        .list_chat_rules(&owner.chat_address)
        .await
        .is_empty());
    assert_eq!(
        target_approvals.request(&card.id).await.unwrap().status,
        ApprovalStatus::Pending
    );
    assert!(!cwd.join("starts").exists());
    canonical_store
        .abort_chat_handoff(&f.fixture.chat_id, &frozen.handoff_id)
        .await
        .unwrap();
    std::fs::create_dir(&rules_path).unwrap();
    let store_error = std::fs::read_to_string(&rules_path)
        .unwrap_err()
        .to_string();
    let error = entry
        .decide_task_approval(&owner, id, 1, &card.id, decision.clone(), "logged-in-user")
        .await
        .unwrap_err();
    assert!(error.contains(&store_error), "{error}");
    assert_eq!(
        target_approvals.request(&card.id).await.unwrap().status,
        ApprovalStatus::Pending
    );
    assert!(
        canonical
            .snapshot_at(&owner.chat_address)
            .await
            .pending
            .is_empty(),
        "failed persistence must not leave a second pending card"
    );
    assert!(!cwd.join("starts").exists());
    std::fs::remove_dir(&rules_path).unwrap();
    assert!(entry
        .decide_task_approval(&owner, id, 2, &card.id, decision.clone(), "logged-in-user")
        .await
        .is_err());
    assert!(canonical
        .list_chat_rules(&owner.chat_address)
        .await
        .is_empty());
    let accepted = entry
        .decide_task_approval(&owner, id, 1, &card.id, decision.clone(), "logged-in-user")
        .await
        .unwrap();
    assert_eq!(accepted.id, card.id);
    assert_eq!(
        accepted
            .authenticated_user_decision
            .as_ref()
            .unwrap()
            .decision,
        AuthenticatedApprovalDecision::AllowChat
    );
    let rules = canonical.list_chat_rules(&owner.chat_address).await;
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].source_approval_id, card.id);
    assert!(
        target_approvals
            .list_chat_rules(&owner.chat_address)
            .await
            .is_empty(),
        "target release does not write a second Chat rule"
    );
    let completed = f
        .fixture
        .source_state
        .task_runtime
        .await_terminal(&owner, id, Duration::from_secs(8))
        .await
        .unwrap();
    assert_eq!(completed.state, rua_task::TaskState::Completed);
    assert_eq!(std::fs::read_to_string(cwd.join("starts")).unwrap(), "once");
    assert_eq!(
        entry
            .decide_task_approval(&owner, id, 1, &card.id, decision.clone(), "logged-in-user")
            .await
            .unwrap(),
        accepted
    );
    assert_eq!(
        canonical.list_chat_rules(&owner.chat_address).await,
        rules,
        "a repeated decision keeps the original rule"
    );

    // A third replica receives the same permission via the existing settings
    // adapter. Ordinary revocation then wins over a retry of the old decision.
    let replica = TempDir::new().unwrap();
    let replica_store = WorkspaceSessionStore::new(replica.path().join("chats"));
    replica_store.save_chat_header(&header).await.unwrap();
    let replica_security = rua_security::SecurityModule::new(
        Arc::new(rua_security::SecurityConfigStore::new(
            replica.path().into(),
        )),
        None,
    );
    let snapshots = || {
        rua_sync::evergreen_chat_adapter::EvergreenChatSyncAdapter::snapshot_changes_for_chat(
            &canonical_workspace,
            &owner.chat_address,
            &f.fixture.target.device_id,
            epoch.as_hlc(),
        )
        .unwrap()
        .into_iter()
        .filter(|change| change.record_id.ends_with("/permission_rules"))
        .collect::<Vec<_>>()
    };
    let mut registry = rua_sync::sync_registry::SyncRegistry::new();
    registry
        .register(Arc::new(
            rua_sync::evergreen_chat_adapter::EvergreenChatSyncAdapter::new(
                replica.path().into(),
                "3333333333333333",
            ),
        ))
        .unwrap();
    let merge = rua_sync::merge_store::MergeStore::open(&replica.path().join("sync")).unwrap();
    let changes = snapshots();
    assert_eq!(changes.len(), 1);
    registry.apply_change(&changes[0], &merge).await.unwrap();
    let copied = replica_security
        .approval_authority()
        .list_chat_rules(&owner.chat_address)
        .await;
    assert_eq!(copied.len(), 1);
    assert_eq!(copied[0].id, rules[0].id);
    assert_eq!(copied[0].matcher, rules[0].matcher);
    canonical
        .delete_chat_rules(&owner.chat_address)
        .await
        .unwrap();
    registry
        .apply_change(&snapshots()[0], &merge)
        .await
        .unwrap();
    assert!(replica_security
        .approval_authority()
        .list_chat_rules(&owner.chat_address)
        .await
        .is_empty());
    assert!(entry
        .decide_task_approval(&owner, id, 1, &card.id, decision, "logged-in-user")
        .await
        .is_err());
    assert!(
        canonical
            .list_chat_rules(&owner.chat_address)
            .await
            .is_empty(),
        "retry of an old decision cannot restore a revoked rule"
    );
    assert_eq!(std::fs::read_to_string(cwd.join("starts")).unwrap(), "once");
    f.shutdown().await;
}
