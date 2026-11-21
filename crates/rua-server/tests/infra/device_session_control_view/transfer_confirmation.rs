use super::*;
use rua_core::tool::{DeviceReadAccess, ToolContext};
use rua_server::services::conversation_authority::ConversationActiveTurnPhase;
use rua_server::services::device_read_check::ForegroundReadAccess;

#[test]
fn transfer_confirmation_wait_is_owned_by_foreground_chat_for_local_or_remote_endpoints() {
    let access = ForegroundReadAccess {
        authority: std::sync::Weak::new(),
        approvals: std::sync::Weak::new(),
        network: std::sync::Weak::new(),
    };
    let mut ctx = ToolContext {
        local_device_id: Some("desktop".into()),
        metadata: [
            (
                rua_agent::hooks::SECURITY_CHAT_ADDRESS_METADATA_KEY.into(),
                serde_json::json!(ChatAddress::new("chat")),
            ),
            ("turn_id".into(), serde_json::json!("turn")),
            (
                "chat_configuration_epoch".into(),
                serde_json::json!("epoch"),
            ),
        ]
        .into(),
        ..Default::default()
    };
    for (source, target) in [
        ("phone", "desktop"),
        ("desktop", "phone"),
        ("phone", "laptop"),
        ("desktop", "desktop"),
    ] {
        let args = serde_json::json!({"source_device_id":source,"source_paths":["/source/a.jpg"],"target_device_id":target,"target_dir":"/destination"});
        assert!(access.manages_operation_confirmation_wait("sync_send", &ctx, &args));
        ctx.metadata
            .insert("task_id".into(), serde_json::json!("task"));
        assert!(!access.manages_operation_confirmation_wait("sync_send", &ctx, &args));
        ctx.metadata.remove("task_id");
    }
    assert!(!access.manages_operation_confirmation_wait(
        "read",
        &ctx,
        &serde_json::json!({"device_id":"desktop"})
    ));
    assert!(access.manages_operation_confirmation_wait(
        "read",
        &ctx,
        &serde_json::json!({"device_id":"phone"})
    ));
    assert!(!access.manages_operation_confirmation_wait(
        "sync_send",
        &ctx,
        &serde_json::json!({"source_device_id":"phone"})
    ));
    ctx.metadata.remove("chat_configuration_epoch");
    assert!(!access.manages_operation_confirmation_wait(
        "sync_send",
        &ctx,
        &serde_json::json!({"source_device_id":"phone","target_device_id":"desktop"})
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_transfer_participant_claims_original_chat_decision_once_after_local_participant() {
    let fixture = RemoteInputConfirmationFixture::new(Duration::ZERO, false).await;
    let owner = &fixture.target_state;
    let release = Arc::new(Notify::new());
    let args = serde_json::json!({
        "source_device_id":fixture.source.device_id, "source_paths":["/photos/a.jpg"],
        "target_device_id":fixture.target.device_id, "target_dir":"/incoming", "future_option":{"keep":true}
    });
    let calls = [
        ToolCall::new("transfer-once", "sync_send", args.clone()),
        ToolCall::new("waiting-transfer", "sync_send", args.clone()),
    ];
    let (address, turn, _, _) = prepare_running_file_calls(&fixture, &calls, &release).await;
    let approvals = owner.security.approval_authority();
    let request = |call: &ToolCall| ApprovalRequestInput {
        agent_id: "default".into(),
        owner: rua_security::ApprovalOwner::chat(address.clone()),
        turn_id: turn.turn_id.clone(),
        tool_call_id: call.id.clone(),
        tool_name: call.name.clone(),
        trigger: ApprovalTrigger::approval_mode("Approve this transfer in the original Chat"),
        tool_call: ToolCallSnapshot {
            display: "sync_send".into(),
            args: call.arguments.clone(),
        },
    };
    let approved = approvals
        .create_target_file_confirmation(request(&calls[0]))
        .await
        .unwrap();
    approvals
        .decide_for_authenticated_user(
            "default",
            &approved.id,
            rua_security::ApprovalDecision::RunOnce {
                decided_by: "webui".into(),
            },
            "fixture-user",
        )
        .await
        .unwrap();

    let mut ctx = rua_security::ToolCallContext::new(
        "default",
        address.chat_id().as_str(),
        "sync_send",
        args.clone(),
    )
    .with_chat_address(address.clone());
    ctx.metadata
        .insert("turn_id".into(), serde_json::json!(turn.turn_id));
    ctx.metadata
        .insert("tool_call_id".into(), serde_json::json!(calls[0].id));
    // Establish the in-process participant's consumed decision; the remote
    // participant below must claim its real dispatch permit through the Device Session.
    let mut local_dispatch = Some(approved.id.clone());
    assert_eq!(
        approvals
            .consume_authenticated_transfer_confirmation_if(&ctx, &fixture.target.device_id, |id| {
                local_dispatch.take().as_deref() == Some(id)
            })
            .await
            .as_deref(),
        Some(approved.id.as_str())
    );
    assert_eq!(
        approvals.request(&approved.id).await.unwrap().status,
        rua_security::AuthorityStatus::Consumed
    );

    let pending = approvals
        .create_target_file_confirmation(request(&calls[1]))
        .await
        .unwrap();
    owner
        .conversation_authority
        .transition_active_turn_phase(
            &address,
            Some(&turn.turn_id),
            ConversationActiveTurnPhase::WaitingApproval,
        )
        .await
        .unwrap();
    let remote_permit = owner
        .conversation_authority
        .begin_read_dispatch(&approved.id, &fixture.source.device_id);
    let command = ConversationControlDeviceCommand {
        command_id: "claim-transfer".into(),
        agent_id: "default".into(),
        address: address.clone(),
        source_device_id: fixture.source.device_id.clone(),
        target_device_id: fixture.target.device_id.clone(),
        authority_epoch: turn.authority_epoch,
        action: ConversationControlDeviceAction::ClaimReadApproval {
            tool_name: "sync_send".into(),
            dispatch_token: remote_permit.token(),
            approval_id: approved.id.clone(),
            turn_id: turn.turn_id.clone(),
            tool_call_id: calls[0].id.clone(),
            arguments: args,
        },
    };
    let denied = |result| {
        assert!(
            matches!(result, Err(DeviceCommandDeliveryError::Rejected { ref reason, .. }) if reason == "read_approval_unavailable"),
            "{result:?}"
        )
    };
    let mut changed = command.clone();
    if let ConversationControlDeviceAction::ClaimReadApproval { arguments, .. } =
        &mut changed.action
    {
        arguments["conflict"] = serde_json::json!("rename");
    }
    denied(fixture.send_control(&changed).await);
    let mut stale = command.clone();
    if let ConversationControlDeviceAction::ClaimReadApproval { turn_id, .. } = &mut stale.action {
        *turn_id = "stale-turn".into();
    }
    denied(fixture.send_control(&stale).await);
    assert!(
        matches!(fixture.send_control(&command).await.unwrap(), ConversationControlDeviceResponse::ReadApprovalClaimed { approval_id } if approval_id == approved.id)
    );
    denied(fixture.send_control(&command).await);
    assert_eq!(approvals.list_pending_for_address(&address).await.len(), 1);
    let replica = fixture
        .source_state
        .security
        .approval_authority()
        .snapshot_at(&address)
        .await;
    assert!(replica.pending.is_empty() && replica.resolved.is_empty());
    approvals.supersede_request(&pending.id).await;
    release.notify_one();
    fixture.shutdown().await;
}
