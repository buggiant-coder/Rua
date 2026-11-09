use std::collections::BTreeMap;
use std::sync::Arc;

use rua_core::event::RuntimeEvent;
use rua_core::tool::ToolCall;
use rua_core::ChatAddress;
use rua_server::services::active_stream_broadcast::{
    ActiveStreamBroadcastFrame, ActiveStreamBroadcastHub,
};
use rua_server::services::console_chat::CONSOLE_CHANNEL_ID;
use rua_server::services::conversation_authority::{
    ConversationActiveTurnPhase, ConversationActiveTurnStart, ConversationAuthority,
    ConversationAuthorityPlacementLockReason, ConversationDriverKind, ConversationInputPayload,
    ConversationSendNowOutcome, ConversationSubmissionAcceptance, ConversationTurnHandoff,
    SUBMISSION_ID_KEY,
};
use rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch;
use rua_server::services::foreground_operation::{
    ForegroundOperationKind, ForegroundOperationRegistry, MobileResidencyState,
};
use rua_server::streaming::{ActiveStreamRegistry, Priority, StreamKey};
use rua_session::{ChatMetadata, ChatSessionHeader, WorkspaceSessionStore};

fn epoch(wall_ms: u64) -> ConversationAuthorityEpoch {
    ConversationAuthorityEpoch::from_parts(wall_ms, 0, [0; 8])
}

fn agent_address(chat_id: &str) -> ChatAddress {
    ChatAddress::agent("default", chat_id)
}

fn stream_key(chat_id: &str) -> StreamKey {
    StreamKey::new(CONSOLE_CHANNEL_ID, agent_address(chat_id), Priority::Normal)
}

#[allow(clippy::too_many_arguments)]
async fn complete_turn(
    authority: &ConversationAuthority,
    streams: &ActiveStreamRegistry,
    key: &StreamKey,
    address: &ChatAddress,
    turn_id: &str,
    status: &str,
    completed_at: Option<String>,
    drain_queue: bool,
) -> ConversationTurnHandoff {
    authority
        .complete_active_turn_and_drain_queue(
            streams,
            key,
            address,
            turn_id,
            status,
            completed_at,
            None,
            drain_queue,
            |_| async {},
        )
        .await
        .expect("complete active Turn")
}

#[tokio::test]
async fn equal_chat_ids_in_agent_and_project_scopes_have_independent_active_turns() {
    let authority = ConversationAuthority::new();
    let agent_address = rua_core::ChatAddress::agent("owner-a", "shared-chat");
    let project_address = rua_core::ChatAddress::project("owner-a", "shared-chat");

    let agent_turn = authority
        .start_active_turn(ConversationActiveTurnStart {
            address: agent_address.clone(),
            driver: ConversationDriverKind::Rua,
            authority_device_id: "device-a".to_string(),
            authority_epoch: epoch(1),
            driver_detail: None,
        })
        .await;
    let project_turn = authority
        .start_active_turn(ConversationActiveTurnStart {
            address: project_address.clone(),
            driver: ConversationDriverKind::Rua,
            authority_device_id: "device-b".to_string(),
            authority_epoch: epoch(1),
            driver_detail: None,
        })
        .await;

    assert_ne!(agent_turn.turn_id, project_turn.turn_id);
    assert_eq!(
        authority
            .active_turn(&agent_address)
            .await
            .map(|turn| turn.authority_device_id),
        Some("device-a".to_string())
    );
    assert_eq!(
        authority
            .active_turn(&project_address)
            .await
            .map(|turn| turn.authority_device_id),
        Some("device-b".to_string())
    );
}

#[tokio::test]
async fn stopped_turn_is_not_reactivated_by_late_runtime_events() {
    let authority = ConversationAuthority::new();
    let address = agent_address("stopping-late-events");
    let turn = authority
        .start_active_turn(ConversationActiveTurnStart {
            address: address.clone(),
            driver: ConversationDriverKind::Acp,
            authority_device_id: "device".into(),
            authority_epoch: epoch(1),
            driver_detail: None,
        })
        .await;
    authority
        .request_stop_matching(
            None,
            CONSOLE_CHANNEL_ID,
            &address,
            Some(&turn.turn_id),
            Some(epoch(1)),
        )
        .await
        .unwrap();
    for phase in [
        ConversationActiveTurnPhase::Streaming,
        ConversationActiveTurnPhase::WaitingApproval,
        ConversationActiveTurnPhase::RunningTool,
        ConversationActiveTurnPhase::Committing,
    ] {
        let observed = authority
            .transition_active_turn_phase(&address, Some(&turn.turn_id), phase)
            .await
            .unwrap();
        assert_eq!(observed.phase, ConversationActiveTurnPhase::Stopping);
    }
}

#[tokio::test]
async fn failed_first_submission_persistence_leaves_no_authority_or_stream_state() {
    let authority = ConversationAuthority::new();
    let streams = ActiveStreamRegistry::new();
    let address = ChatAddress::project("project-a", "chat-first-persist");
    let key = StreamKey::new(CONSOLE_CHANNEL_ID, address.clone(), Priority::Normal);
    let start = || ConversationActiveTurnStart {
        address: address.clone(),
        driver: ConversationDriverKind::Rua,
        authority_device_id: "resource-device".to_string(),
        authority_epoch: epoch(1),
        driver_detail: None,
    };

    let failed = authority
        .accept_submission(
            &streams,
            &key,
            start(),
            pending_message("first"),
            None,
            Some(Box::new(
                |_pending,
                 _prepared|
                 -> std::pin::Pin<
                    Box<dyn std::future::Future<Output = Result<(), String>> + Send>,
                > { Box::pin(async { Err("disk full".to_string()) }) },
            )),
        )
        .await;
    assert!(matches!(failed, Err(ref error) if error == "disk full"));
    assert!(authority.active_turn(&address).await.is_none());
    assert!(
        !streams
            .is_active_for_chat(CONSOLE_CHANNEL_ID, &address)
            .await
    );

    let mut pending = pending_message("first");
    pending.extra.insert(
        "turn_id".to_string(),
        serde_json::json!("turn-cli-project-first"),
    );
    let accepted = authority
        .accept_submission(
            &streams,
            &key,
            start(),
            pending,
            Some("device-submit".to_string()),
            Some(Box::new(
                |_pending,
                 _prepared|
                 -> std::pin::Pin<
                    Box<dyn std::future::Future<Output = Result<(), String>> + Send>,
                > { Box::pin(async { Ok(()) }) },
            )),
        )
        .await
        .unwrap();
    assert!(matches!(
        accepted,
        ConversationSubmissionAcceptance::Started { .. }
    ));
    let active_turn = authority.active_turn(&address).await.expect("active turn");
    assert_eq!(active_turn.turn_id, "turn-cli-project-first");
    assert_eq!(
        active_turn.submit_device_id.as_deref(),
        Some("device-submit")
    );
}

#[tokio::test]
async fn busy_submission_queues_without_running_persist_callback() {
    let authority = ConversationAuthority::new();
    let streams = ActiveStreamRegistry::new();
    let address = agent_address("chat-cli-busy");
    let key = stream_key("chat-cli-busy");
    let start = || ConversationActiveTurnStart {
        address: address.clone(),
        driver: ConversationDriverKind::Rua,
        authority_device_id: "device-local".to_string(),
        authority_epoch: epoch(1),
        driver_detail: None,
    };

    let first = authority
        .accept_submission(
            &streams,
            &key,
            start(),
            pending_message("first"),
            None,
            None,
        )
        .await
        .unwrap();
    assert!(matches!(
        first,
        ConversationSubmissionAcceptance::Started { .. }
    ));
    authority
        .pop_next_input(&address)
        .await
        .expect("active input");

    let queued = authority
        .accept_submission(
            &streams,
            &key,
            start(),
            pending_message("second"),
            None,
            Some(Box::new(
                |_,
                 _|
                 -> std::pin::Pin<
                    Box<dyn std::future::Future<Output = Result<(), String>> + Send>,
                > { panic!("busy submission must not run persistence callback") },
            )),
        )
        .await
        .unwrap();
    assert!(matches!(
        queued,
        ConversationSubmissionAcceptance::Queued { .. }
    ));

    let control = authority
        .control_view(&address, &serde_json::json!({}))
        .await;
    assert_eq!(control.queue.entries.len(), 1);
    assert_eq!(control.queue.entries[0].text, "second");
    assert!(control.active_turn.is_some());
}

#[tokio::test]
async fn project_chat_rejects_every_authority_movement_operation() {
    let authority = ConversationAuthority::new();
    let temp = tempfile::tempdir().unwrap();
    let address = ChatAddress::project("project-a", "chat-fixed-authority");
    let target =
        rua_server::services::conversation_authority_placement::ConversationAuthorityPlacement {
            device_id: "other-device".to_string(),
            authority_epoch: epoch(2),
            base_manifest_hash: format!("sha256:{}", "2".repeat(64)),
        };

    for result in [
        authority
            .switch_authority(None, temp.path(), &address, target.clone())
            .await
            .map(|_| ()),
        authority
            .apply_confirmed_authority_placement(temp.path(), &address, target.clone())
            .await
            .map(|_| ()),
        authority
            .apply_authority_supersession(temp.path(), &address, target)
            .await
            .map(|_| ()),
    ] {
        assert!(matches!(
            result,
            Err(rua_server::services::chat_metadata_store::ChatMetadataStoreError::Conflict(
                ref detail
            )) if detail.contains("fixed")
        ));
    }
}

#[tokio::test]
async fn authority_switch_uses_chat_policy_instead_of_inferring_from_scope() {
    let authority = ConversationAuthority::new();
    let temp = tempfile::tempdir().unwrap();
    let address = ChatAddress::project("project-a", "chat-selectable-authority");
    let mut header = ChatSessionHeader::new(ChatMetadata {
        chat_id: address.chat_id().to_string(),
        agent_id: "default".to_string(),
        title: "Selectable authority".to_string(),
        created_at: "2026-08-20T00:00:00Z".to_string(),
        last_activity_at: "2026-08-20T00:00:00Z".to_string(),
        pinned: false,
        archived: false,
        preview: None,
    });
    header.extra.insert(
        "chat_policy".to_string(),
        serde_json::to_value(rua_chat::ChatPolicy {
            agent_assignment: rua_chat::AgentAssignmentPolicy::Selectable,
            resource: rua_chat::ResourcePolicy::ProjectBinding,
            authority_placement: rua_chat::AuthorityPlacementPolicy::Selectable,
        })
        .unwrap(),
    );
    header.extra.insert(
        "selected_agent_id".to_string(),
        serde_json::json!("default"),
    );
    WorkspaceSessionStore::new(temp.path())
        .save_chat_header(&header)
        .await
        .unwrap();
    let target =
        rua_server::services::conversation_authority_placement::ConversationAuthorityPlacement {
            device_id: "other-device".to_string(),
            authority_epoch: epoch(2),
            base_manifest_hash: format!("sha256:{}", "2".repeat(64)),
        };

    authority
        .switch_authority(None, temp.path(), &address, target.clone())
        .await
        .expect("Selectable Authority Policy permits movement");

    let chat = rua_chat::ChatService::for_resolved_scope(temp.path(), address.clone())
        .load(&address)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        chat.authority_placement()
            .and_then(|placement| placement.get("device_id"))
            .and_then(serde_json::Value::as_str),
        Some(target.device_id.as_str())
    );
    let control = authority
        .control_view(
            &address,
            &serde_json::json!({
                "chat_policy": {
                    "agent_assignment": "selectable",
                    "resource": "project_binding",
                    "authority_placement": "selectable"
                },
                "authority_placement": target
            }),
        )
        .await;
    assert!(control.allowed_controls.switch_authority);
    assert!(control.placement.switchable);
}

#[tokio::test]
async fn execution_residency_follows_the_same_phase_contract_for_rua_and_acp() {
    for (index, driver) in [ConversationDriverKind::Rua, ConversationDriverKind::Acp]
        .into_iter()
        .enumerate()
    {
        let chat_id = format!("chat-{index}");
        let registry = Arc::new(ForegroundOperationRegistry::new(true));
        registry.enter_background(false);
        let authority = ConversationAuthority::with_foreground_operations(Arc::clone(&registry));
        let active_turn = authority
            .start_active_turn(ConversationActiveTurnStart {
                address: rua_core::ChatAddress::agent("default".to_string(), chat_id.clone()),
                driver,
                authority_device_id: "device-mobile".to_string(),
                authority_epoch: epoch(1),
                driver_detail: None,
            })
            .await;
        assert_eq!(registry.snapshot().active_count, 1);
        assert_eq!(
            registry.snapshot().operations[0].kind,
            ForegroundOperationKind::ConversationExecution
        );

        let streaming = authority
            .observe_runtime_event(
                &agent_address(&chat_id),
                Some(&active_turn.turn_id),
                &RuntimeEvent::TextDelta {
                    delta: "partial".to_string(),
                    message_id: None,
                },
            )
            .await
            .expect("streaming phase");
        assert_eq!(streaming.phase, ConversationActiveTurnPhase::Streaming);
        let running_tool = authority
            .observe_runtime_event(
                &agent_address(&chat_id),
                Some(&active_turn.turn_id),
                &RuntimeEvent::ToolCall {
                    call: ToolCall::new("call-1", "Shell", serde_json::json!({})),
                },
            )
            .await
            .expect("running tool phase");
        assert_eq!(running_tool.phase, ConversationActiveTurnPhase::RunningTool);
        assert_eq!(registry.snapshot().active_count, 1);

        authority
            .enqueue_input(&agent_address(&chat_id), pending_message("queued"), None)
            .await
            .expect("active turn queue");
        authority
            .observe_runtime_event(
                &agent_address(&chat_id),
                Some(&active_turn.turn_id),
                &RuntimeEvent::ToolApprovalPending {
                    call_id: "call-1".to_string(),
                    tool_name: "Shell".to_string(),
                    request_id: Some("approval-1".to_string()),
                    prompt: None,
                },
            )
            .await
            .expect("waiting approval");
        assert_eq!(
            registry.snapshot().active_count,
            0,
            "queued input must not retain execution residency"
        );
        assert_eq!(
            registry.snapshot().residency_state,
            MobileResidencyState::BackgroundGrace
        );

        registry.acquire(
            format!("viewer:{chat_id}"),
            format!("viewer:{chat_id}"),
            ForegroundOperationKind::ChatViewer,
        );
        authority
            .transition_active_turn_phase(
                &agent_address(&chat_id),
                Some(&active_turn.turn_id),
                ConversationActiveTurnPhase::RunningTool,
            )
            .await
            .expect("approval resume");
        assert_eq!(
            registry.snapshot().active_count,
            2,
            "viewer and execution leases must remain independent"
        );

        let committing = authority
            .observe_runtime_event(
                &agent_address(&chat_id),
                Some(&active_turn.turn_id),
                &RuntimeEvent::End {
                    finish_reason: Some("completed".to_string()),
                },
            )
            .await
            .expect("committing phase");
        assert_eq!(committing.phase, ConversationActiveTurnPhase::Committing);

        let stopping = authority
            .request_stop(None, CONSOLE_CHANNEL_ID, &agent_address(&chat_id))
            .await
            .expect("stop active turn");
        assert_eq!(stopping.phase, ConversationActiveTurnPhase::Stopping);
        assert_eq!(registry.snapshot().active_count, 2);
        authority.finish_active_turn(&agent_address(&chat_id)).await;
        assert_eq!(registry.snapshot().active_count, 1);
        authority.finish_active_turn(&agent_address(&chat_id)).await;
        assert_eq!(
            registry.snapshot().active_count,
            1,
            "terminal cleanup must release execution exactly once"
        );
        registry.release(&format!("viewer:{chat_id}"));
        assert_eq!(registry.snapshot().active_count, 0);
    }
}

#[tokio::test]
async fn control_view_exposes_active_turn_without_run_contract() {
    let authority = ConversationAuthority::new();
    let active_turn = authority
        .start_active_turn(ConversationActiveTurnStart {
            address: rua_core::ChatAddress::agent("default".to_string(), "chat-1".to_string()),
            driver: ConversationDriverKind::Rua,
            authority_device_id: "device-b".to_string(),
            authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(3, 0, [0; 8]),
            driver_detail: None,
        })
        .await;

    assert!(active_turn.turn_id.starts_with("turn-"));
    assert_eq!(active_turn.phase, ConversationActiveTurnPhase::Preparing);

    let control = authority
        .control_view(&agent_address("chat-1"), &serde_json::json!({}))
        .await;
    assert_eq!(
        control
            .active_turn
            .as_ref()
            .map(|turn| turn.turn_id.as_str()),
        Some(active_turn.turn_id.as_str())
    );

    let value = serde_json::to_value(control).expect("serialize control view");
    assert!(value.get("active_turn").is_some());
    assert!(value.get("run").is_none());
    assert!(!value.to_string().contains("run_id"));
}

#[tokio::test]
async fn idle_control_retains_authority_placement_after_terminal_handoff() {
    let authority = ConversationAuthority::new();
    let streams = ActiveStreamRegistry::new();
    let key = stream_key("chat-idle");
    let (_producer_id, _cancel_rx) = streams.register_active_keyed(&key).await;
    let authority_epoch = epoch(7);
    let active_turn = authority
        .start_active_turn(ConversationActiveTurnStart {
            address: rua_core::ChatAddress::agent("default".to_string(), "chat-idle".to_string()),
            driver: ConversationDriverKind::Rua,
            authority_device_id: "device-authority".to_string(),
            authority_epoch,
            driver_detail: None,
        })
        .await;

    complete_turn(
        &authority,
        &streams,
        &key,
        &agent_address("chat-idle"),
        &active_turn.turn_id,
        "completed",
        Some("2026-07-31T00:00:00Z".to_string()),
        true,
    )
    .await;

    let idle = authority
        .control_view(&agent_address("chat-idle"), &serde_json::json!({}))
        .await;
    assert!(idle.active_turn.is_none());
    assert_eq!(
        idle.placement.authority_device_id.as_deref(),
        Some("device-authority")
    );
    assert_eq!(idle.placement.authority_epoch, Some(authority_epoch));
}

#[tokio::test]
async fn control_revision_increases_with_each_published_authority_mutation() {
    let authority = ConversationAuthority::new();
    let idle = authority
        .control_view(&agent_address("chat-1"), &serde_json::json!({}))
        .await;
    assert_eq!(idle.revision, 0);

    authority
        .start_active_turn(ConversationActiveTurnStart {
            address: rua_core::ChatAddress::agent("default".to_string(), "chat-1".to_string()),
            driver: ConversationDriverKind::Rua,
            authority_device_id: "device-b".to_string(),
            authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(3, 0, [0; 8]),
            driver_detail: None,
        })
        .await;
    let started = authority
        .control_view(&agent_address("chat-1"), &serde_json::json!({}))
        .await;
    assert_eq!(started.revision, 1);

    authority
        .enqueue_input(&agent_address("chat-1"), pending_message("queued"), None)
        .await
        .expect("queue input");
    let queued = authority
        .control_view(&agent_address("chat-1"), &serde_json::json!({}))
        .await;
    assert!(queued.revision > started.revision);
}

#[tokio::test]
async fn terminal_candidate_keeps_the_same_active_turn_in_committing_phase() {
    let authority = ConversationAuthority::new();
    let active_turn = authority
        .start_active_turn(ConversationActiveTurnStart {
            address: rua_core::ChatAddress::agent("default".to_string(), "chat-1".to_string()),
            driver: ConversationDriverKind::Rua,
            authority_device_id: "device-b".to_string(),
            authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(3, 0, [0; 8]),
            driver_detail: None,
        })
        .await;

    let fact = authority
        .commit_turn_terminal(
            &agent_address("chat-1"),
            "completed",
            Some("2026-07-21T00:00:00Z".to_string()),
            None,
        )
        .await
        .expect("terminal fact");
    assert_eq!(fact.turn_id, active_turn.turn_id);

    let control = authority
        .control_view(&agent_address("chat-1"), &serde_json::json!({}))
        .await;
    let committing = control.active_turn.expect("committing active turn");
    assert_eq!(committing.turn_id, fact.turn_id);
    assert_eq!(committing.phase, ConversationActiveTurnPhase::Committing);
}

#[tokio::test]
async fn completing_active_turn_reaches_idle_before_draining_the_next_input() {
    let authority = ConversationAuthority::new();
    let streams = ActiveStreamRegistry::new();
    let key = stream_key("chat-1");
    let (_producer_id, _cancel_rx) = streams.register_active_keyed(&key).await;
    let completed_turn = authority
        .start_active_turn(ConversationActiveTurnStart {
            address: rua_core::ChatAddress::agent("default".to_string(), "chat-1".to_string()),
            driver: ConversationDriverKind::Rua,
            authority_device_id: "device-b".to_string(),
            authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(3, 0, [0; 8]),
            driver_detail: None,
        })
        .await;
    let queued = authority
        .enqueue_input(&agent_address("chat-1"), pending_message("next"), None)
        .await
        .expect("queue next input");
    let mut control_events = authority
        .subscribe_control_events(&agent_address("chat-1"))
        .await;

    let handoff = complete_turn(
        &authority,
        &streams,
        &key,
        &agent_address("chat-1"),
        &completed_turn.turn_id,
        "completed",
        Some("2026-07-21T00:00:00Z".to_string()),
        true,
    )
    .await;

    assert_eq!(handoff.terminal_fact.turn_id, completed_turn.turn_id);
    let next = handoff.next_input.expect("next input claimed");
    assert_eq!(next.snapshot.input_id, queued.input_id);
    let next_turn_id = next.pending.extra["turn_id"]
        .as_str()
        .expect("claimed input turn id");
    assert_ne!(next_turn_id, completed_turn.turn_id);

    let idle: serde_json::Value = serde_json::from_str(
        &control_events
            .recv()
            .await
            .expect("idle Control View must be published"),
    )
    .expect("idle Control View payload");
    assert_eq!(idle["reason"], "turn_completed");
    assert!(idle["control"]["active_turn"].is_null());

    let next_started: serde_json::Value = serde_json::from_str(
        &control_events
            .recv()
            .await
            .expect("next Turn start must be published"),
    )
    .expect("next Turn Control View payload");
    assert_eq!(next_started["reason"], "active_turn_started");
    assert_eq!(
        next_started["control"]["active_turn"]["turn_id"],
        next_turn_id
    );

    let active_turn = authority
        .active_turn(&agent_address("chat-1"))
        .await
        .expect("next turn active before completion returns");
    assert_eq!(active_turn.turn_id, next_turn_id);
    assert!(authority
        .control_view(&agent_address("chat-1"), &serde_json::json!({}))
        .await
        .queue
        .entries
        .is_empty());
}

#[tokio::test]
async fn terminal_and_control_transitions_share_one_activity_order() {
    let authority = ConversationAuthority::new();
    let hub = Arc::new(ActiveStreamBroadcastHub::new());
    authority.set_active_stream_broadcast_hub(Arc::clone(&hub));
    let streams = ActiveStreamRegistry::new();
    let address = agent_address("chat-activity-order");
    let key = stream_key("chat-activity-order");
    let (_producer_id, _cancel_rx) = streams.register_active_keyed(&key).await;
    let mut activity = hub
        .subscribe_chat(&address, &BTreeMap::new())
        .await
        .rx
        .expect("persistent Chat activity");

    let completed_turn = authority
        .start_active_turn(ConversationActiveTurnStart {
            address: address.clone(),
            driver: ConversationDriverKind::Rua,
            authority_device_id: "device-b".to_string(),
            authority_epoch: epoch(3),
            driver_detail: None,
        })
        .await;
    authority
        .enqueue_input(&address, pending_message("next"), None)
        .await
        .expect("queue next input");

    let terminal_hub = Arc::clone(&hub);
    let terminal_address = address.clone();
    authority
        .complete_active_turn_and_drain_queue(
            &streams,
            &key,
            &address,
            &completed_turn.turn_id,
            "completed",
            Some("2026-08-05T00:00:00Z".to_string()),
            None,
            true,
            move |fact| async move {
                terminal_hub
                    .publish_conversation_event(
                        "turn-stream",
                        &terminal_address,
                        fact.authority_epoch,
                        serde_json::json!({
                            "object": "response",
                            "status": "completed",
                            "output": [],
                        })
                        .to_string(),
                    )
                    .await;
                terminal_hub
                    .publish_conversation_event(
                        "turn-stream",
                        &terminal_address,
                        fact.authority_epoch,
                        serde_json::json!({
                            "type": "turn_terminal",
                            "turn_id": fact.turn_id,
                            "status": fact.status,
                        })
                        .to_string(),
                    )
                    .await;
            },
        )
        .await
        .expect("complete active Turn");

    let mut ordered = Vec::new();
    while ordered.len() < 6 {
        let frame = activity.recv().await.expect("activity frame");
        if let ActiveStreamBroadcastFrame::Event { payload, .. } = frame {
            let value: serde_json::Value =
                serde_json::from_str(&payload).expect("activity payload");
            ordered.push((
                value["activity_sequence"]
                    .as_u64()
                    .expect("activity sequence"),
                value["type"]
                    .as_str()
                    .or_else(|| value["object"].as_str())
                    .expect("activity type")
                    .to_string(),
                value["reason"].as_str().map(str::to_string),
            ));
        }
    }

    assert_eq!(
        ordered,
        vec![
            (
                1,
                "conversation_control".to_string(),
                Some("active_turn_started".to_string()),
            ),
            (
                2,
                "conversation_control".to_string(),
                Some("queue_accepted".to_string()),
            ),
            (3, "response".to_string(), None,),
            (4, "turn_terminal".to_string(), None),
            (
                5,
                "conversation_control".to_string(),
                Some("turn_completed".to_string()),
            ),
            (
                6,
                "conversation_control".to_string(),
                Some("active_turn_started".to_string()),
            ),
        ]
    );
}

#[tokio::test]
async fn completing_final_turn_keeps_local_stream_until_terminal_is_published() {
    let authority = ConversationAuthority::new();
    let streams = ActiveStreamRegistry::new();
    let key = stream_key("chat-1");
    let (producer_id, _cancel_rx) = streams.register_active_keyed(&key).await;
    let (_replay, mut subscriber) = streams
        .subscribe_keyed(&key)
        .await
        .expect("local viewer subscription");
    let completed_turn = authority
        .start_active_turn(ConversationActiveTurnStart {
            address: rua_core::ChatAddress::agent("default".to_string(), "chat-1".to_string()),
            driver: ConversationDriverKind::Rua,
            authority_device_id: "device-local".to_string(),
            authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
            driver_detail: None,
        })
        .await;

    let handoff = complete_turn(
        &authority,
        &streams,
        &key,
        &agent_address("chat-1"),
        &completed_turn.turn_id,
        "completed",
        Some("2026-07-21T00:00:00Z".to_string()),
        true,
    )
    .await;

    assert!(handoff.next_input.is_none());
    assert!(authority
        .active_turn(&agent_address("chat-1"))
        .await
        .is_none());

    let terminal_payload = r#"{"type":"turn_terminal","status":"completed"}"#;
    streams
        .push_event_keyed(&key, terminal_payload.to_string())
        .await;
    let received = tokio::time::timeout(std::time::Duration::from_millis(100), subscriber.recv())
        .await
        .expect("local viewer should receive terminal before stream cleanup")
        .expect("local viewer channel should remain open");
    assert_eq!(received, terminal_payload);

    assert!(streams.mark_completed(&key, producer_id).await);
    assert!(streams.subscribe_keyed(&key).await.is_none());
}

#[tokio::test]
async fn regeneration_reuses_the_target_turn_for_authority_handoff() {
    let authority = ConversationAuthority::new();
    let streams = ActiveStreamRegistry::new();
    let key = stream_key("chat-1");
    let mut pending = pending_message("regenerate");
    pending.extra.insert(
        "regenerate_turn_id".to_string(),
        serde_json::json!("turn-existing"),
    );

    let (producer_id, _cancel_rx) = match authority
        .accept_submission(
            &streams,
            &key,
            ConversationActiveTurnStart {
                address: rua_core::ChatAddress::agent("default".to_string(), "chat-1".to_string()),
                driver: ConversationDriverKind::Rua,
                authority_device_id: "device-local".to_string(),
                authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
                driver_detail: None,
            },
            pending,
            None,
            None,
        )
        .await
        .unwrap()
    {
        ConversationSubmissionAcceptance::Started {
            producer_id,
            cancel_rx,
            ..
        } => (producer_id, cancel_rx),
        ConversationSubmissionAcceptance::Queued { .. } => {
            panic!("idle regeneration should start immediately")
        }
    };

    let active_turn = authority
        .active_turn(&agent_address("chat-1"))
        .await
        .expect("regeneration active turn");
    assert_eq!(active_turn.turn_id, "turn-existing");

    let claimed = authority
        .pop_next_input(&agent_address("chat-1"))
        .await
        .expect("regeneration input");
    assert_eq!(
        claimed.pending.extra["turn_id"].as_str(),
        Some("turn-existing")
    );

    let handoff = complete_turn(
        &authority,
        &streams,
        &key,
        &agent_address("chat-1"),
        "turn-existing",
        "completed",
        Some("2026-07-22T00:00:00Z".to_string()),
        true,
    )
    .await;
    assert_eq!(handoff.terminal_fact.turn_id, "turn-existing");
    assert!(handoff.next_input.is_none());
    assert!(authority
        .active_turn(&agent_address("chat-1"))
        .await
        .is_none());
    assert!(streams.mark_completed(&key, producer_id).await);
}

#[tokio::test]
async fn queued_regeneration_claims_its_target_turn_during_handoff() {
    let authority = ConversationAuthority::new();
    let streams = ActiveStreamRegistry::new();
    let key = stream_key("chat-1");
    let (producer_id, _cancel_rx) = streams.register_active_keyed(&key).await;
    let current_turn = authority
        .start_active_turn(ConversationActiveTurnStart {
            address: rua_core::ChatAddress::agent("default".to_string(), "chat-1".to_string()),
            driver: ConversationDriverKind::Rua,
            authority_device_id: "device-local".to_string(),
            authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
            driver_detail: None,
        })
        .await;
    let mut pending = pending_message("regenerate next");
    pending.extra.insert(
        "regenerate_turn_id".to_string(),
        serde_json::json!("turn-existing"),
    );

    let acceptance = authority
        .accept_submission(
            &streams,
            &key,
            ConversationActiveTurnStart {
                address: rua_core::ChatAddress::agent("default".to_string(), "chat-1".to_string()),
                driver: ConversationDriverKind::Rua,
                authority_device_id: "device-local".to_string(),
                authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
                driver_detail: None,
            },
            pending,
            None,
            None,
        )
        .await
        .unwrap();
    assert!(matches!(
        acceptance,
        ConversationSubmissionAcceptance::Queued { .. }
    ));

    let handoff = complete_turn(
        &authority,
        &streams,
        &key,
        &agent_address("chat-1"),
        &current_turn.turn_id,
        "completed",
        Some("2026-07-22T00:00:00Z".to_string()),
        true,
    )
    .await;
    let next = handoff.next_input.expect("queued regeneration input");
    assert_eq!(
        next.pending.extra["turn_id"].as_str(),
        Some("turn-existing")
    );
    assert_eq!(
        authority
            .active_turn(&agent_address("chat-1"))
            .await
            .expect("regeneration active turn")
            .turn_id,
        "turn-existing"
    );

    complete_turn(
        &authority,
        &streams,
        &key,
        &agent_address("chat-1"),
        "turn-existing",
        "completed",
        Some("2026-07-22T00:00:01Z".to_string()),
        true,
    )
    .await;
    assert!(streams.mark_completed(&key, producer_id).await);
}

#[tokio::test]
async fn stop_preserves_and_pauses_queue_until_explicit_resume() {
    let authority = ConversationAuthority::new();
    let streams = ActiveStreamRegistry::new();
    let key = stream_key("chat-1");
    let (producer_id, cancel_rx) = streams.register_active_keyed(&key).await;
    let active_turn = authority
        .start_active_turn(ConversationActiveTurnStart {
            address: rua_core::ChatAddress::agent("default".to_string(), "chat-1".to_string()),
            driver: ConversationDriverKind::Rua,
            authority_device_id: "device-b".to_string(),
            authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(3, 0, [0; 8]),
            driver_detail: None,
        })
        .await;
    let queued = authority
        .enqueue_input(&agent_address("chat-1"), pending_message("keep me"), None)
        .await
        .expect("queue input");

    authority
        .request_stop(Some(&streams), CONSOLE_CHANNEL_ID, &agent_address("chat-1"))
        .await
        .expect("request stop");
    assert_eq!(*cancel_rx.borrow(), rua_core::runtime::RunControl::Stop);
    let handoff = complete_turn(
        &authority,
        &streams,
        &key,
        &agent_address("chat-1"),
        &active_turn.turn_id,
        "interrupted",
        Some("2026-07-21T00:00:00Z".to_string()),
        false,
    )
    .await;
    assert!(handoff.next_input.is_none());

    let control = authority
        .control_view(&agent_address("chat-1"), &serde_json::json!({}))
        .await;
    assert!(control.active_turn.is_none());
    assert!(control.queue.paused);
    assert_eq!(control.queue.entries.len(), 1);
    assert_eq!(control.queue.entries[0].input_id, queued.input_id);
    assert!(streams.mark_completed(&key, producer_id).await);

    let accepted = authority
        .accept_submission(
            &streams,
            &key,
            ConversationActiveTurnStart {
                address: rua_core::ChatAddress::agent("default".to_string(), "chat-1".to_string()),
                driver: ConversationDriverKind::Rua,
                authority_device_id: "device-b".to_string(),
                authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(3, 0, [0; 8]),
                driver_detail: None,
            },
            pending_message("also keep me"),
            None,
            None,
        )
        .await
        .unwrap();
    assert!(matches!(
        accepted,
        ConversationSubmissionAcceptance::Queued { .. }
    ));
    assert!(
        !streams
            .is_active_for_chat(CONSOLE_CHANNEL_ID, &agent_address("chat-1"))
            .await
    );
    assert_eq!(
        authority
            .control_view(&agent_address("chat-1"), &serde_json::json!({}))
            .await
            .queue
            .entries
            .len(),
        2
    );
}

#[tokio::test]
async fn stop_with_empty_queue_allows_later_submission_to_be_claimed() {
    let authority = ConversationAuthority::new();
    let streams = ActiveStreamRegistry::new();
    let key = stream_key("chat-1");
    let (producer_id, cancel_rx) = streams.register_active_keyed(&key).await;
    let active_turn = authority
        .start_active_turn(ConversationActiveTurnStart {
            address: rua_core::ChatAddress::agent("default".to_string(), "chat-1".to_string()),
            driver: ConversationDriverKind::Rua,
            authority_device_id: "device-b".to_string(),
            authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(3, 0, [0; 8]),
            driver_detail: None,
        })
        .await;

    authority
        .request_stop(Some(&streams), CONSOLE_CHANNEL_ID, &agent_address("chat-1"))
        .await
        .expect("request stop");
    assert_eq!(*cancel_rx.borrow(), rua_core::runtime::RunControl::Stop);
    assert!(
        !authority
            .control_view(&agent_address("chat-1"), &serde_json::json!({}))
            .await
            .queue
            .paused,
        "an empty queue at the stop boundary must not pause later submissions"
    );

    let accepted = authority
        .accept_submission(
            &streams,
            &key,
            ConversationActiveTurnStart {
                address: rua_core::ChatAddress::agent("default".to_string(), "chat-1".to_string()),
                driver: ConversationDriverKind::Rua,
                authority_device_id: "device-b".to_string(),
                authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(3, 0, [0; 8]),
                driver_detail: None,
            },
            pending_message("send after stop"),
            None,
            None,
        )
        .await
        .unwrap();
    assert!(matches!(
        accepted,
        ConversationSubmissionAcceptance::Queued { .. }
    ));

    let handoff = complete_turn(
        &authority,
        &streams,
        &key,
        &agent_address("chat-1"),
        &active_turn.turn_id,
        "interrupted",
        Some("2026-07-21T00:00:00Z".to_string()),
        true,
    )
    .await;
    let next = handoff
        .next_input
        .expect("later submission should auto-resume");
    assert_eq!(next.pending.message, "send after stop");
    assert!(streams.mark_completed(&key, producer_id).await);
}

#[tokio::test]
async fn stop_does_not_treat_unclaimed_started_input_as_a_queued_followup() {
    let authority = ConversationAuthority::new();
    let streams = ActiveStreamRegistry::new();
    let key = stream_key("chat-1");
    let acceptance = authority
        .accept_submission(
            &streams,
            &key,
            ConversationActiveTurnStart {
                address: rua_core::ChatAddress::agent("default".to_string(), "chat-1".to_string()),
                driver: ConversationDriverKind::Rua,
                authority_device_id: "device-b".to_string(),
                authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(3, 0, [0; 8]),
                driver_detail: None,
            },
            pending_message("current turn"),
            None,
            None,
        )
        .await
        .unwrap();
    let ConversationSubmissionAcceptance::Started {
        producer_id,
        cancel_rx,
        ..
    } = acceptance
    else {
        panic!("first input should start the turn");
    };

    authority
        .request_stop(Some(&streams), CONSOLE_CHANNEL_ID, &agent_address("chat-1"))
        .await
        .expect("request stop");
    assert_eq!(*cancel_rx.borrow(), rua_core::runtime::RunControl::Stop);
    assert!(
        !authority
            .control_view(&agent_address("chat-1"), &serde_json::json!({}))
            .await
            .queue
            .paused,
        "the current turn's unclaimed first input is not a queued follow-up"
    );

    authority.clear_queue(&agent_address("chat-1")).await;
    assert!(streams.mark_completed(&key, producer_id).await);
}

#[tokio::test]
async fn rua_queued_submit_requests_internal_continue_at_safe_boundary() {
    let authority = ConversationAuthority::new();
    let streams = ActiveStreamRegistry::new();
    let key = stream_key("chat-1");
    let (_producer_id, cancel_rx) = streams.register_active_keyed(&key).await;
    authority
        .start_active_turn(ConversationActiveTurnStart {
            address: rua_core::ChatAddress::agent("default".to_string(), "chat-1".to_string()),
            driver: ConversationDriverKind::Rua,
            authority_device_id: "device-b".to_string(),
            authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(3, 0, [0; 8]),
            driver_detail: None,
        })
        .await;

    let accepted = authority
        .accept_submission(
            &streams,
            &key,
            ConversationActiveTurnStart {
                address: rua_core::ChatAddress::agent("default".to_string(), "chat-1".to_string()),
                driver: ConversationDriverKind::Rua,
                authority_device_id: "device-b".to_string(),
                authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(3, 0, [0; 8]),
                driver_detail: None,
            },
            pending_message("insert at next boundary"),
            None,
            None,
        )
        .await
        .unwrap();

    assert!(matches!(
        accepted,
        ConversationSubmissionAcceptance::Queued { .. }
    ));
    assert_eq!(
        *cancel_rx.borrow(),
        rua_core::runtime::RunControl::ContinueAtBoundary
    );
}

#[tokio::test]
async fn send_now_starts_selected_paused_input_once_when_chat_is_idle() {
    let authority = ConversationAuthority::new();
    let streams = ActiveStreamRegistry::new();
    let key = stream_key("chat-1");
    let (producer_id, _cancel_rx) = streams.register_active_keyed(&key).await;
    let active_turn = authority
        .start_active_turn(ConversationActiveTurnStart {
            address: rua_core::ChatAddress::agent("default".to_string(), "chat-1".to_string()),
            driver: ConversationDriverKind::Rua,
            authority_device_id: "device-b".to_string(),
            authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(3, 0, [0; 8]),
            driver_detail: None,
        })
        .await;
    let queued = authority
        .enqueue_input(
            &agent_address("chat-1"),
            pending_message("keep queued"),
            None,
        )
        .await
        .expect("first queued input");
    let selected = authority
        .enqueue_input(
            &agent_address("chat-1"),
            pending_message("send me now"),
            None,
        )
        .await
        .expect("selected queued input");
    authority
        .request_stop(Some(&streams), CONSOLE_CHANNEL_ID, &agent_address("chat-1"))
        .await
        .expect("request stop");
    let handoff = complete_turn(
        &authority,
        &streams,
        &key,
        &agent_address("chat-1"),
        &active_turn.turn_id,
        "interrupted",
        Some("2026-07-21T00:00:00Z".to_string()),
        false,
    )
    .await;
    assert!(handoff.next_input.is_none());
    assert!(streams.mark_completed(&key, producer_id).await);

    let started = authority
        .request_send_now(&streams, CONSOLE_CHANNEL_ID, &agent_address("chat-1"),
            &selected.input_id,
            epoch(3),
            "send-now-idle",
            ConversationActiveTurnStart {
                address: rua_core::ChatAddress::agent("default".to_string(), "chat-1".to_string()),
                driver: ConversationDriverKind::Rua,
                authority_device_id: "device-b".to_string(),
                authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(3, 0, [0; 8]),
                driver_detail: None,
            },
        )
        .await
        .expect("send now");
    let ConversationSendNowOutcome::Started { input, .. } = started else {
        panic!("expected selected input to start");
    };
    assert_eq!(input.input_id, selected.input_id);
    assert!(authority
        .active_turn(&agent_address("chat-1"))
        .await
        .is_some());
    let control = authority
        .control_view(&agent_address("chat-1"), &serde_json::json!({}))
        .await;
    assert!(!control.queue.paused);
    assert_eq!(control.queue.entries.len(), 2);
    assert_eq!(control.queue.entries[0].input_id, selected.input_id);
    assert_eq!(control.queue.entries[1].input_id, queued.input_id);

    let retried = authority
        .request_send_now(&streams, CONSOLE_CHANNEL_ID, &agent_address("chat-1"),
            &selected.input_id,
            epoch(3),
            "send-now-idle",
            ConversationActiveTurnStart {
                address: rua_core::ChatAddress::agent("default".to_string(), "chat-1".to_string()),
                driver: ConversationDriverKind::Rua,
                authority_device_id: "device-b".to_string(),
                authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(3, 0, [0; 8]),
                driver_detail: None,
            },
        )
        .await
        .expect("idempotent retry");
    assert!(matches!(
        retried,
        ConversationSendNowOutcome::AlreadyApplied { .. }
    ));
}

#[tokio::test]
async fn control_view_projects_active_turn_metadata_and_allowed_controls() {
    let authority = ConversationAuthority::new();
    authority
        .start_active_turn(ConversationActiveTurnStart {
            address: rua_core::ChatAddress::agent("default".to_string(), "chat-1".to_string()),
            driver: ConversationDriverKind::Rua,
            authority_device_id: "device-b".to_string(),
            authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(3, 0, [0; 8]),
            driver_detail: None,
        })
        .await;

    let control = authority
        .control_view(
            &agent_address("chat-1"),
            &serde_json::json!({
                "authority_placement": {
                    "device_id": "device-b",
                    "authority_epoch": "0000000000000003000000000000000000000000",
                    "base_manifest_hash": format!("sha256:{}", "1".repeat(64))
                },
                "acp_binding": {
                    "kind": "acp_passthrough",
                    "runner": "codex"
                }
            }),
        )
        .await;

    assert_eq!(control.authority_device_id.as_deref(), Some("device-b"));
    assert_eq!(
        control.placement.authority_device_id.as_deref(),
        Some("device-b")
    );
    assert_eq!(control.placement.authority_epoch, Some(epoch(3)));
    assert!(!control.placement.switchable);
    assert_eq!(
        control.placement.lock_reason,
        Some(ConversationAuthorityPlacementLockReason::AcpPassthrough)
    );
    assert_eq!(
        control.active_turn.as_ref().map(|snapshot| snapshot.driver),
        Some(ConversationDriverKind::Rua)
    );
    assert!(control.queue.entries.is_empty());
    assert!(control.acp.active);
    assert_eq!(control.acp.binding.as_ref().unwrap()["runner"], "codex");
    assert!(control.allowed_controls.stop);
    assert!(control.allowed_controls.queue);
    assert!(control.allowed_controls.interject);
    assert!(!control.allowed_controls.switch_authority);
    assert!(!control.allowed_controls.enter_acp);
    assert!(control.allowed_controls.exit_acp);
}

#[tokio::test]
async fn control_view_projects_idle_explicit_placement() {
    let authority = ConversationAuthority::new();

    let missing = authority
        .control_view(&agent_address("chat-1"), &serde_json::json!({}))
        .await;
    assert!(missing.placement.authority_device_id.is_none());
    assert!(missing.placement.authority_epoch.is_none());

    let explicit = authority
        .control_view(
            &agent_address("chat-1"),
            &serde_json::json!({
                "authority_placement": {
                    "device_id": "device-b",
                    "authority_epoch": epoch(2),
                    "base_manifest_hash": format!("sha256:{}", "2".repeat(64))
                }
            }),
        )
        .await;
    assert_eq!(
        explicit.placement.authority_device_id.as_deref(),
        Some("device-b")
    );
    assert_eq!(explicit.placement.authority_epoch, Some(epoch(2)));
    assert!(explicit.placement.switchable);
    assert!(explicit.placement.lock_reason.is_none());
}

#[tokio::test]
async fn control_view_projects_runtime_queue_entries_from_authority() {
    let authority = ConversationAuthority::new();
    authority
        .start_active_turn(ConversationActiveTurnStart {
            address: rua_core::ChatAddress::agent("default".to_string(), "chat-1".to_string()),
            driver: ConversationDriverKind::Rua,
            authority_device_id: "device-b".to_string(),
            authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
            driver_detail: None,
        })
        .await;

    let queued = authority
        .enqueue_input(
            &agent_address("chat-1"),
            pending_message("queued from authority"),
            Some("device-a".to_string()),
        )
        .await
        .expect("active authority accepts queued input");
    assert!(queued.input_id.starts_with("input-"));
    assert_eq!(queued.text, "queued from authority");

    let control = authority
        .control_view(&agent_address("chat-1"), &serde_json::json!({}))
        .await;

    assert_eq!(control.active_turn.as_ref().unwrap().queued_count, 1);
    assert_eq!(control.queue.entries.len(), 1);
    assert_eq!(control.queue.entries[0].input_id, queued.input_id);
    assert_eq!(control.queue.entries[0].text, "queued from authority");
    assert_eq!(
        control.queue.entries[0].submit_device_id.as_deref(),
        Some("device-a")
    );
    assert!(control.queue.entries[0].editable);
    assert!(control.queue.entries[0].deletable);
}

#[tokio::test]
async fn control_view_excludes_task_watch_notifications_from_user_input_queue() {
    let authority = ConversationAuthority::new();
    authority
        .start_active_turn(ConversationActiveTurnStart {
            address: agent_address("chat-1"),
            driver: ConversationDriverKind::Rua,
            authority_device_id: "device-b".to_string(),
            authority_epoch: epoch(1),
            driver_detail: None,
        })
        .await;

    let mut notification = pending_message("");
    notification.extra.insert(
        "runtime_observation".to_string(),
        serde_json::json!({"runtime_note_id": "notification-1"}),
    );
    authority
        .enqueue_input(&agent_address("chat-1"), notification, None)
        .await
        .expect("active authority accepts task watch notification");

    assert_eq!(
        authority
            .queue_snapshot(&agent_address("chat-1"))
            .await
            .len(),
        1
    );

    let control = authority
        .control_view(&agent_address("chat-1"), &serde_json::json!({}))
        .await;

    assert!(control.queue.entries.is_empty());
    assert_eq!(control.active_turn.as_ref().unwrap().queued_count, 0);
}

#[tokio::test]
async fn editing_queued_input_preserves_identity_and_position() {
    let authority = ConversationAuthority::new();
    authority
        .start_active_turn(ConversationActiveTurnStart {
            address: rua_core::ChatAddress::agent("default".to_string(), "chat-1".to_string()),
            driver: ConversationDriverKind::Rua,
            authority_device_id: "device-local".to_string(),
            authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
            driver_detail: None,
        })
        .await;
    let first = authority
        .enqueue_input(&agent_address("chat-1"), pending_message("first"), None)
        .await
        .expect("first");
    let second = authority
        .enqueue_input(&agent_address("chat-1"), pending_message("second"), None)
        .await
        .expect("second");

    let edited = authority
        .edit_queued_input(&agent_address("chat-1"), &second.input_id, "second edited")
        .await
        .expect("edit");
    assert_eq!(edited.input_id, second.input_id);
    assert_eq!(edited.text, "second edited");

    let control = authority
        .control_view(&agent_address("chat-1"), &serde_json::json!({}))
        .await;
    assert_eq!(control.queue.entries[0].input_id, first.input_id);
    assert_eq!(control.queue.entries[0].text, "first");
    assert_eq!(control.queue.entries[1].input_id, second.input_id);
    assert_eq!(control.queue.entries[1].text, "second edited");
}

#[tokio::test]
async fn identical_text_submissions_remain_distinct_authority_inputs() {
    let authority = ConversationAuthority::new();
    authority
        .start_active_turn(ConversationActiveTurnStart {
            address: rua_core::ChatAddress::agent("default".to_string(), "chat-1".to_string()),
            driver: ConversationDriverKind::Rua,
            authority_device_id: "device-local".to_string(),
            authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
            driver_detail: None,
        })
        .await;

    let mut first_pending = pending_message("repeat");
    first_pending.extra.insert(
        "submission_id".to_string(),
        serde_json::json!("submission-repeat-1"),
    );
    let mut second_pending = pending_message("repeat");
    second_pending.extra.insert(
        "submission_id".to_string(),
        serde_json::json!("submission-repeat-2"),
    );
    authority
        .enqueue_input(&agent_address("chat-1"), first_pending, None)
        .await
        .expect("first identical input");
    authority
        .enqueue_input(&agent_address("chat-1"), second_pending, None)
        .await
        .expect("second identical input");

    let control = authority
        .control_view(&agent_address("chat-1"), &serde_json::json!({}))
        .await;
    assert_eq!(control.queue.entries.len(), 2);
    assert_eq!(control.queue.entries[0].text, "repeat");
    assert_eq!(control.queue.entries[1].text, "repeat");
    assert_eq!(
        control.queue.entries[0].submission_id,
        "submission-repeat-1"
    );
    assert_eq!(
        control.queue.entries[1].submission_id,
        "submission-repeat-2"
    );
}

#[tokio::test]
async fn accepted_submission_id_stays_idempotent_after_queue_claim() {
    let authority = ConversationAuthority::new();
    let streams = ActiveStreamRegistry::new();
    let key = stream_key("chat-1");
    let (producer_id, _cancel_rx) = streams.register_active_keyed(&key).await;
    authority
        .start_active_turn(ConversationActiveTurnStart {
            address: rua_core::ChatAddress::agent("default".to_string(), "chat-1".to_string()),
            driver: ConversationDriverKind::Rua,
            authority_device_id: "device-local".to_string(),
            authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
            driver_detail: None,
        })
        .await;

    let mut pending = pending_message("retry");
    pending.extra.insert(
        "submission_id".to_string(),
        serde_json::json!("submission-retry"),
    );
    let first = authority
        .accept_submission(
            &streams,
            &key,
            ConversationActiveTurnStart {
                address: rua_core::ChatAddress::agent("default".to_string(), "chat-1".to_string()),
                driver: ConversationDriverKind::Rua,
                authority_device_id: "device-local".to_string(),
                authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
                driver_detail: None,
            },
            pending.clone(),
            Some("device-submit".to_string()),
            None,
        )
        .await
        .unwrap();
    assert!(matches!(
        first,
        ConversationSubmissionAcceptance::Queued { .. }
    ));
    let claimed = authority
        .claim_next_or_finish(&streams, &key, producer_id, &agent_address("chat-1"))
        .await
        .expect("worker claims accepted input");
    assert_eq!(claimed.snapshot.submission_id, "submission-retry");
    assert_eq!(
        claimed.snapshot.submit_device_id.as_deref(),
        Some("device-submit")
    );
    assert_eq!(
        authority
            .active_turn(&agent_address("chat-1"))
            .await
            .and_then(|turn| turn.submit_device_id),
        Some("device-submit".to_string())
    );

    let duplicate = authority
        .accept_submission(
            &streams,
            &key,
            ConversationActiveTurnStart {
                address: rua_core::ChatAddress::agent("default".to_string(), "chat-1".to_string()),
                driver: ConversationDriverKind::Rua,
                authority_device_id: "device-local".to_string(),
                authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
                driver_detail: None,
            },
            pending,
            Some("device-submit".to_string()),
            None,
        )
        .await
        .unwrap();
    assert!(matches!(
        duplicate,
        ConversationSubmissionAcceptance::Queued { .. }
    ));
    assert!(
        authority
            .control_view(&agent_address("chat-1"), &serde_json::json!({}))
            .await
            .queue
            .entries
            .is_empty(),
        "transport redelivery must not reinsert an already claimed submission"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_duplicate_submission_id_inserts_one_authority_input() {
    let authority = Arc::new(ConversationAuthority::new());
    authority
        .start_active_turn(ConversationActiveTurnStart {
            address: rua_core::ChatAddress::agent("default".to_string(), "chat-1".to_string()),
            driver: ConversationDriverKind::Rua,
            authority_device_id: "device-local".to_string(),
            authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
            driver_detail: None,
        })
        .await;

    let barrier = Arc::new(tokio::sync::Barrier::new(33));
    let mut submissions = Vec::new();
    for _ in 0..32 {
        let authority = Arc::clone(&authority);
        let barrier = Arc::clone(&barrier);
        submissions.push(tokio::spawn(async move {
            let mut pending = pending_message("retry");
            pending.extra.insert(
                "submission_id".to_string(),
                serde_json::json!("submission-concurrent-retry"),
            );
            barrier.wait().await;
            authority
                .enqueue_input(&agent_address("chat-1"), pending, None)
                .await
                .expect("concurrent submission accepted")
        }));
    }
    barrier.wait().await;

    let mut input_ids = Vec::new();
    for submission in submissions {
        input_ids.push(
            submission
                .await
                .expect("concurrent submission task")
                .input_id,
        );
    }
    input_ids.sort();
    input_ids.dedup();
    assert_eq!(input_ids.len(), 1);

    let control = authority
        .control_view(&agent_address("chat-1"), &serde_json::json!({}))
        .await;
    assert_eq!(control.queue.entries.len(), 1);
    assert_eq!(
        control.queue.entries[0].submission_id,
        "submission-concurrent-retry"
    );
}

#[tokio::test]
async fn delete_and_pop_update_runtime_queue_without_durable_replay() {
    let authority = ConversationAuthority::new();
    authority
        .start_active_turn(ConversationActiveTurnStart {
            address: rua_core::ChatAddress::agent("default".to_string(), "chat-1".to_string()),
            driver: ConversationDriverKind::Rua,
            authority_device_id: "device-local".to_string(),
            authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
            driver_detail: None,
        })
        .await;
    let first = authority
        .enqueue_input(&agent_address("chat-1"), pending_message("first"), None)
        .await
        .expect("first");
    let second = authority
        .enqueue_input(&agent_address("chat-1"), pending_message("second"), None)
        .await
        .expect("second");

    let deleted = authority
        .delete_queued_input(&agent_address("chat-1"), &first.input_id)
        .await
        .expect("delete first");
    assert_eq!(deleted.input_id, first.input_id);
    let popped = authority
        .pop_next_input(&agent_address("chat-1"))
        .await
        .expect("pop second");
    assert_eq!(popped.snapshot.input_id, second.input_id);
    assert_eq!(popped.pending.message, "second");

    let control = authority
        .control_view(&agent_address("chat-1"), &serde_json::json!({}))
        .await;
    assert!(control.queue.entries.is_empty());
    assert_eq!(control.active_turn.as_ref().unwrap().queued_count, 0);

    let mut replacement_payload = pending_message("replacement");
    replacement_payload.extra.insert(
        SUBMISSION_ID_KEY.to_string(),
        serde_json::json!(first.submission_id),
    );
    let replacement = authority
        .enqueue_input(&agent_address("chat-1"), replacement_payload, None)
        .await
        .expect("replacement input");
    assert_ne!(replacement.input_id, first.input_id);
    authority.finish_active_turn(&agent_address("chat-1")).await;
    let streams = ActiveStreamRegistry::new();
    let replacement_started = authority
        .request_send_now(
            &streams,
            CONSOLE_CHANNEL_ID,
            &agent_address("chat-1"),
            &replacement.input_id,
            epoch(2),
            "send-replacement-now",
            ConversationActiveTurnStart {
                address: agent_address("chat-1"),
                driver: ConversationDriverKind::Rua,
                authority_device_id: "device-local".to_string(),
                authority_epoch: epoch(2),
                driver_detail: None,
            },
        )
        .await
        .expect("send replacement now");
    let ConversationSendNowOutcome::Started { input, .. } = replacement_started else {
        panic!("expected replacement input to start");
    };
    assert_eq!(input.input_id, replacement.input_id);

    let restarted = ConversationAuthority::new();
    let restarted_control = restarted
        .control_view(&agent_address("chat-1"), &serde_json::json!({}))
        .await;
    assert!(restarted_control.queue.entries.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_submit_and_worker_finish_never_strands_input() {
    for _ in 0..64 {
        let authority = Arc::new(ConversationAuthority::new());
        let streams = Arc::new(ActiveStreamRegistry::new());
        let key = stream_key("chat-1");
        let (producer_id, _cancel_rx) = streams.register_active_keyed(&key).await;
        authority
            .start_active_turn(ConversationActiveTurnStart {
                address: rua_core::ChatAddress::agent("default".to_string(), "chat-1".to_string()),
                driver: ConversationDriverKind::Rua,
                authority_device_id: "device-local".to_string(),
                authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
                driver_detail: None,
            })
            .await;

        let barrier = Arc::new(tokio::sync::Barrier::new(3));
        let finish = {
            let authority = Arc::clone(&authority);
            let streams = Arc::clone(&streams);
            let key = key.clone();
            let barrier = Arc::clone(&barrier);
            tokio::spawn(async move {
                barrier.wait().await;
                authority
                    .claim_next_or_finish(
                        streams.as_ref(),
                        &key,
                        producer_id,
                        &agent_address("chat-1"),
                    )
                    .await
            })
        };
        let submit = {
            let authority = Arc::clone(&authority);
            let streams = Arc::clone(&streams);
            let key = key.clone();
            let barrier = Arc::clone(&barrier);
            tokio::spawn(async move {
                barrier.wait().await;
                authority
                    .accept_submission(
                        streams.as_ref(),
                        &key,
                        ConversationActiveTurnStart {
                            address: rua_core::ChatAddress::agent("default".to_string(), "chat-1".to_string()),
                            driver: ConversationDriverKind::Rua,
                            authority_device_id: "device-local".to_string(),
                            authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
                            driver_detail: None,
                        },
                        pending_message("next"),
                        None,
                        None,
                    )
                    .await
                    .unwrap()
            })
        };
        barrier.wait().await;

        let handoff = finish.await.expect("finish transition");
        let acceptance = submit.await.expect("submit transition");
        match acceptance {
            ConversationSubmissionAcceptance::Started { input, .. } => {
                assert!(handoff.is_none());
                assert_eq!(input.text, "next");
                assert_eq!(
                    authority
                        .control_view(&agent_address("chat-1"), &serde_json::json!({}))
                        .await
                        .queue
                        .entries
                        .len(),
                    1
                );
            }
            ConversationSubmissionAcceptance::Queued { input } => {
                let handoff = handoff.expect("old worker must claim queued input");
                assert_eq!(handoff.snapshot.input_id, input.input_id);
                assert_eq!(handoff.pending.message, "next");
            }
        }

        assert!(
            authority
                .active_turn(&agent_address("chat-1"))
                .await
                .is_some(),
            "accepted input must retain or start a worker"
        );
        assert!(
            streams
                .is_active_for_chat(CONSOLE_CHANNEL_ID, &agent_address("chat-1"))
                .await,
            "accepted input must retain or register a stream producer"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_terminal_handoff_and_submit_never_strands_input() {
    for _ in 0..64 {
        let authority = Arc::new(ConversationAuthority::new());
        let streams = Arc::new(ActiveStreamRegistry::new());
        let key = stream_key("chat-1");
        let (producer_id, _cancel_rx) = streams.register_active_keyed(&key).await;
        let active_turn = authority
            .start_active_turn(ConversationActiveTurnStart {
                address: rua_core::ChatAddress::agent("default".to_string(), "chat-1".to_string()),
                driver: ConversationDriverKind::Rua,
                authority_device_id: "device-local".to_string(),
                authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
                driver_detail: None,
            })
            .await;

        let barrier = Arc::new(tokio::sync::Barrier::new(3));
        let complete = {
            let authority = Arc::clone(&authority);
            let streams = Arc::clone(&streams);
            let key = key.clone();
            let barrier = Arc::clone(&barrier);
            let turn_id = active_turn.turn_id.clone();
            tokio::spawn(async move {
                barrier.wait().await;
                let claimed = authority
                    .complete_active_turn_and_drain_queue(
                        streams.as_ref(),
                        &key,
                        &agent_address("chat-1"),
                        &turn_id,
                        "completed",
                        Some("2026-07-21T00:00:00Z".to_string()),
                        None,
                        true,
                        |_| async {},
                    )
                    .await
                    .expect("terminal completion")
                    .next_input;
                if claimed.is_none() {
                    streams.mark_completed(&key, producer_id).await;
                }
                claimed
            })
        };
        let submit = {
            let authority = Arc::clone(&authority);
            let streams = Arc::clone(&streams);
            let key = key.clone();
            let barrier = Arc::clone(&barrier);
            tokio::spawn(async move {
                barrier.wait().await;
                authority
                    .accept_submission(
                        streams.as_ref(),
                        &key,
                        ConversationActiveTurnStart {
                            address: rua_core::ChatAddress::agent("default".to_string(), "chat-1".to_string()),
                            driver: ConversationDriverKind::Rua,
                            authority_device_id: "device-local".to_string(),
                            authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
                            driver_detail: None,
                        },
                        pending_message("next"),
                        None,
                        None,
                    )
                    .await
                    .unwrap()
            })
        };
        barrier.wait().await;

        let claimed = complete.await.expect("completion task");
        let acceptance = submit.await.expect("submit task");
        match acceptance {
            ConversationSubmissionAcceptance::Started { input, .. } => {
                assert!(claimed.is_none());
                assert_eq!(input.text, "next");
            }
            ConversationSubmissionAcceptance::Queued { input } => {
                let claimed = claimed.expect("queued input claimed");
                assert_eq!(claimed.snapshot.input_id, input.input_id);
            }
        }

        assert!(
            authority
                .active_turn(&agent_address("chat-1"))
                .await
                .is_some(),
            "accepted input must own an Active Turn after terminal handoff"
        );
        assert!(
            streams
                .is_active_for_chat(CONSOLE_CHANNEL_ID, &agent_address("chat-1"))
                .await,
            "accepted input must retain or register a producer"
        );
    }
}

#[test]
fn control_metadata_writes_go_through_conversation_authority() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let allowed = [
        "services/conversation_authority.rs",
        "services/chat_metadata_store.rs",
    ];
    let forbidden = [
        "update_authority_placement(",
        "persist_acp_binding(",
        "clear_acp_binding(",
        "update_authority_placement as",
        "persist_acp_binding as",
        "clear_acp_binding as",
    ];
    let mut violations = Vec::new();
    collect_metadata_write_violations(&root, &root, &allowed, &forbidden, &mut violations);

    assert!(
        violations.is_empty(),
        "control-plane metadata writes must go through ConversationAuthority:\n{}",
        violations.join("\n")
    );
}

#[test]
fn active_stream_registry_is_not_used_as_chat_business_queue() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let allowed: [&str; 0] = [];
    let forbidden = [
        "enqueue_pending_if_active(",
        "enqueue_and_attach(",
        "enqueue_without_subscriber(",
        "pop_pending(",
        "PendingMessage",
        "original_chat_id",
        "StreamKey::legacy(",
        "pub async fn register(&self, key: &str)",
        "pub async fn push_event(&self, key: &str",
        "pub async fn subscribe(\n        &self,\n        key: &str",
        "pub async fn cancel(&self, key: &str)",
        "pub async fn remove(&self, key: &str)",
        "pub async fn is_active(&self, key: &str)",
        "pub async fn active_keys_for_agent(",
        "pub async fn accept_remote_input(",
        "external_cancel: Option<tokio::sync::watch::Receiver<rua_core::RunControl>>",
        "chat_service.chat_stream(&agent_id, runtime, req, None)",
    ];
    let mut violations = Vec::new();
    collect_source_violations(&root, &root, &allowed, &forbidden, &mut violations);

    assert!(
        violations.is_empty(),
        "ActiveStreamRegistry active pending queue must not be used as Chat business queue:\n{}",
        violations.join("\n")
    );
}

fn collect_metadata_write_violations(
    root: &std::path::Path,
    dir: &std::path::Path,
    allowed: &[&str],
    forbidden: &[&str],
    violations: &mut Vec<String>,
) {
    for entry in std::fs::read_dir(dir).expect("read source dir") {
        let entry = entry.expect("source entry");
        let path = entry.path();
        if path.is_dir() {
            collect_metadata_write_violations(root, &path, allowed, forbidden, violations);
            continue;
        }
        if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
            continue;
        }
        let relative = path
            .strip_prefix(root)
            .expect("relative source path")
            .to_string_lossy()
            .replace('\\', "/");
        if allowed.iter().any(|allowed| *allowed == relative) {
            continue;
        }
        let source = std::fs::read_to_string(&path).expect("read source");
        for pattern in forbidden {
            if source.contains(pattern) {
                violations.push(format!("{relative}: contains `{pattern}`"));
            }
        }
    }
}

fn collect_source_violations(
    root: &std::path::Path,
    dir: &std::path::Path,
    allowed: &[&str],
    forbidden: &[&str],
    violations: &mut Vec<String>,
) {
    for entry in std::fs::read_dir(dir).expect("read source dir") {
        let entry = entry.expect("source entry");
        let path = entry.path();
        if path.is_dir() {
            collect_source_violations(root, &path, allowed, forbidden, violations);
            continue;
        }
        if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
            continue;
        }
        let relative = path
            .strip_prefix(root)
            .expect("relative source path")
            .to_string_lossy()
            .replace('\\', "/");
        if allowed.iter().any(|allowed| *allowed == relative) {
            continue;
        }
        let source = std::fs::read_to_string(&path).expect("read source");
        for pattern in forbidden {
            if source.contains(pattern) {
                violations.push(format!("{relative}: contains `{pattern}`"));
            }
        }
    }
}

fn pending_message(message: &str) -> ConversationInputPayload {
    ConversationInputPayload {
        message: message.to_string(),
        model_override: None,
        input: None,
        messages: None,
        extra: serde_json::Map::new(),
        attachments: Vec::new(),
        enqueued_at: std::time::Instant::now(),
    }
}

struct BlockingCompletionNotification {
    started: tokio::sync::Notify,
    release: tokio::sync::Notify,
    completed: std::sync::atomic::AtomicBool,
}

#[async_trait::async_trait]
impl rua_server::services::user_notification::LocalUserNotificationAdapter
    for BlockingCompletionNotification
{
    async fn present(
        &self,
        _request: rua_server::services::user_notification::UserNotificationRequest,
    ) -> bool {
        self.started.notify_one();
        self.release.notified().await;
        self.completed
            .store(true, std::sync::atomic::Ordering::SeqCst);
        true
    }
}

#[async_trait::async_trait]
impl rua_server::services::user_notification::RemoteUserNotificationTransport
    for BlockingCompletionNotification
{
    async fn send(
        &self,
        _request: rua_server::services::user_notification::UserNotificationRequest,
    ) -> bool {
        panic!("fixture has only the local notification target");
    }
}

#[tokio::test]
async fn slow_completion_notification_does_not_block_next_submission_or_queued_turn() {
    use rua_server::services::agent_manager::AgentManager;
    use rua_server::services::chat_completion_notification::ChatCompletionNotifier;
    use rua_server::services::user_notification::UserNotificationService;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    for has_queued_input in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let notification = Arc::new(BlockingCompletionNotification {
            started: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
            completed: AtomicBool::new(false),
        });
        let notifications = Arc::new(UserNotificationService::new(
            "device-local",
            notification.clone(),
            notification.clone(),
        ));
        let authority = Arc::new(ConversationAuthority::new());
        authority.set_chat_completion_notifier(Arc::new(ChatCompletionNotifier::new(
            "device-local".to_string(),
            temp.path().to_path_buf(),
            temp.path().join("sync"),
            notifications,
            Arc::new(AgentManager::new(temp.path().to_path_buf())),
            Arc::new(std::sync::OnceLock::new()),
        )));
        let streams = Arc::new(ActiveStreamRegistry::new());
        let key = stream_key("notification-handoff");
        let start = ConversationActiveTurnStart {
            address: key.address.clone(),
            driver: ConversationDriverKind::Rua,
            authority_device_id: "device-local".to_string(),
            authority_epoch: epoch(1),
            driver_detail: None,
        };
        let (producer_id, _cancel_rx) = streams.register_active_keyed(&key).await;
        let first = authority.start_active_turn(start.clone()).await;
        if has_queued_input {
            authority
                .enqueue_input(&key.address, pending_message("queued follow-up"), None)
                .await
                .unwrap();
        }
        let mut completion = {
            let authority = Arc::clone(&authority);
            let streams = Arc::clone(&streams);
            let key = key.clone();
            tokio::spawn(async move {
                let handoff = complete_turn(
                    &authority,
                    &streams,
                    &key,
                    &key.address,
                    &first.turn_id,
                    "completed",
                    None,
                    true,
                )
                .await;
                if handoff.next_input.is_none() {
                    assert!(streams.mark_completed(&key, producer_id).await);
                }
                handoff
            })
        };
        tokio::time::timeout(Duration::from_secs(1), notification.started.notified())
            .await
            .expect("completion notification started");
        let handoff = tokio::time::timeout(Duration::from_secs(1), &mut completion).await;
        if handoff.is_err() {
            notification.release.notify_one();
            completion.await.unwrap();
            panic!("completion notification blocked the producer handoff");
        }
        let handoff = handoff.unwrap().unwrap();
        assert!(!notification.completed.load(Ordering::SeqCst));
        if has_queued_input {
            let next = handoff.next_input.expect("queued Turn is ready to execute");
            assert_eq!(next.pending.message, "queued follow-up");
            assert_ne!(next.pending.extra["turn_id"], handoff.terminal_fact.turn_id);
        } else {
            assert!(handoff.next_input.is_none());
            let accepted = tokio::time::timeout(
                Duration::from_secs(1),
                authority.accept_submission(
                    &streams,
                    &key,
                    start,
                    pending_message("new follow-up"),
                    None,
                    None,
                ),
            )
            .await
            .expect("new submission must not wait for completion notification")
            .unwrap();
            assert!(matches!(
                accepted,
                ConversationSubmissionAcceptance::Started { .. }
            ));
        }
        if has_queued_input {
            drop(authority);
            tokio::time::timeout(Duration::from_secs(1), async {
                while Arc::strong_count(&notification) > 1 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("dropping the authority cancels its pending notifications");
            assert!(!notification.completed.load(Ordering::SeqCst));
        } else {
            notification.release.notify_one();
            tokio::time::timeout(Duration::from_secs(1), async {
                while !notification.completed.load(Ordering::SeqCst) {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("notification still completes after the handoff");
        }
    }
}

#[tokio::test]
async fn stop_allows_background_results_without_resuming_user_queue() {
    for during_stop in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let state = rua_server::AppState::from_tempdir(
            temp.path().to_path_buf(),
            rua_server::ServerProfile::Test,
        );
        let authority = &state.conversation_authority;
        let streams = &state.streams;
        let address = agent_address("background-after-stop");
        let key = stream_key("background-after-stop");
        let start = || ConversationActiveTurnStart {
            address: address.clone(),
            driver: ConversationDriverKind::Rua,
            authority_device_id: "device".into(),
            authority_epoch: epoch(1),
            driver_detail: None,
        };
        let (producer_id, _cancel_rx) = streams.register_active_keyed(&key).await;
        let active = authority.start_active_turn(start()).await;
        let user = authority
            .enqueue_input(&address, pending_message("keep paused"), None)
            .await
            .unwrap();
        authority
            .request_stop(Some(streams), CONSOLE_CHANNEL_ID, &address)
            .await
            .unwrap();
        assert!(
            !authority.background_work_view(&address).await.suppressed,
            "User Stop must not suppress Task or collaboration results"
        );
        if !during_stop {
            complete_turn(
                authority,
                streams,
                &key,
                &address,
                &active.turn_id,
                "interrupted",
                None,
                false,
            )
            .await;
            assert!(streams.mark_completed(&key, producer_id).await);
        }
        let mut result = pending_message("background result");
        result
            .extra
            .insert("background_result".into(), serde_json::json!(true));
        result
            .extra
            .insert("runtime_observation".into(), serde_json::json!({}));
        let accepted = authority
            .accept_submission(streams, &key, start(), result, None, None)
            .await
            .expect("background result is accepted after Stop");
        let background_turn = if during_stop {
            assert!(matches!(
                accepted,
                ConversationSubmissionAcceptance::Queued { .. }
            ));
            let handoff = complete_turn(
                authority,
                streams,
                &key,
                &address,
                &active.turn_id,
                "interrupted",
                None,
                false,
            )
            .await;
            assert_eq!(
                handoff.next_input.unwrap().pending.message,
                "background result"
            );
            authority.active_turn(&address).await.unwrap()
        } else {
            assert!(matches!(
                accepted,
                ConversationSubmissionAcceptance::Started { .. }
            ));
            let first = authority.pop_next_input(&address).await.unwrap();
            assert_eq!(
                first.pending.message, "background result",
                "the background producer must not claim paused user input"
            );
            authority.active_turn(&address).await.unwrap()
        };
        let handoff = complete_turn(
            authority,
            streams,
            &key,
            &address,
            &background_turn.turn_id,
            "completed",
            None,
            true,
        )
        .await;
        assert!(
            handoff.next_input.is_none(),
            "background completion must preserve user pause"
        );
        let control = authority
            .control_view(&address, &serde_json::json!({}))
            .await;
        assert!(control.queue.paused);
        assert_eq!(control.queue.entries.len(), 1);
        assert_eq!(control.queue.entries[0].input_id, user.input_id);
    }
}
