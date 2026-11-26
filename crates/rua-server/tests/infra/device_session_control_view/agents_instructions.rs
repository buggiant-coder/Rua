use super::*;
use rua_core::agents_instruction_discovery::{
    AgentsInstructionDiscoveryRequest, AgentsInstructionTargetKind, AGENTS_INSTRUCTIONS_MAX_BYTES,
};
use rua_server::services::device_command_bus::{
    deliver_agents_instructions_via_session, AgentsInstructionCommand,
};
use rua_server::services::device_read_check::{DeviceReadCheckRequest, DeviceReadConfirmation};
use serde_json::json;

async fn prepare() -> (RemoteInputConfirmationFixture, AgentsInstructionCommand) {
    let fixture = RemoteInputConfirmationFixture::new(Duration::ZERO, false).await;
    assert_eq!(fixture.submit().await["accepted"], true);
    let header = WorkspaceSessionStore::new(fixture.target_state.chat_workspace().as_path())
        .load_chat_header(&fixture.chat_id)
        .await
        .unwrap()
        .unwrap();
    WorkspaceSessionStore::new(fixture.source_state.chat_workspace().as_path())
        .save_chat_header(&header)
        .await
        .unwrap();
    let placement = rua_server::services::conversation_authority_placement::parse_conversation_authority_placement(&json!(header.extra)).unwrap();
    install_connectable_peer(&fixture.target, &fixture.source);
    let target_path = fixture.source_state.rua_root.join("instructions");
    std::fs::create_dir_all(&target_path).unwrap();
    let command = AgentsInstructionCommand {
        context: DeviceReadCheckRequest {
            source_device_id: fixture.target.device_id.clone(),
            target_device_id: fixture.source.device_id.clone(),
            agent_id: "default".into(),
            address: ChatAddress::new(fixture.chat_id.clone()),
            authority_epoch: placement.authority_epoch,
            turn_id: String::new(),
            tool_call_id: String::new(),
            tool_name: String::new(),
            confirmation: None,
            arguments: serde_json::Value::Null,
        },
        discovery: AgentsInstructionDiscoveryRequest {
            device_id: fixture.source.device_id.clone(),
            target_path: target_path.canonicalize().unwrap().to_str().unwrap().into(),
            target_kind: AgentsInstructionTargetKind::Directory,
            max_bytes: AGENTS_INSTRUCTIONS_MAX_BYTES,
            ..Default::default()
        },
    };
    (fixture, command)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_discovery_uses_target_content_reloads_and_never_creates_approval() {
    let (fixture, mut command) = prepare().await;
    let local_directory = fixture.target_state.rua_root.join("instructions");
    std::fs::create_dir_all(&local_directory).unwrap();
    std::fs::write(local_directory.join("AGENTS.md"), "execution-device decoy").unwrap();
    let remote_file = PathBuf::from(&command.discovery.target_path).join("AGENTS.md");
    for content in [
        "resource-device instructions",
        "changed resource instructions",
    ] {
        std::fs::write(&remote_file, content).unwrap();
        let result = deliver_agents_instructions_via_session(
            fixture.target.runtime.session_manager(),
            &command,
            Duration::from_secs(8),
        )
        .await
        .unwrap();
        assert_eq!(result.sources.len(), 1);
        assert_eq!(result.sources[0].content, content);
        assert_eq!(result.sources[0].device_id, fixture.source.device_id);
        assert_eq!(result.sources[0].path, remote_file.to_str().unwrap());
    }
    command.discovery.max_bytes = 8;
    let result = deliver_agents_instructions_via_session(
        fixture.target.runtime.session_manager(),
        &command,
        Duration::from_secs(8),
    )
    .await
    .unwrap();
    assert_eq!(result.sources[0].content, "changed");
    assert!(result.truncated);
    let mut policy = fixture
        .source_state
        .security
        .config_store()
        .load("default")
        .unwrap();
    policy.tool_guard.approval_mode = "ask_first".into();
    fixture
        .source_state
        .security
        .save_and_invalidate("default", &policy)
        .unwrap();
    let result = deliver_agents_instructions_via_session(
        fixture.target.runtime.session_manager(),
        &command,
        Duration::from_secs(8),
    )
    .await
    .unwrap();
    assert!(result.sources.is_empty());
    for state in [&fixture.source_state, &fixture.target_state] {
        let snapshot = state
            .security
            .approval_authority()
            .snapshot_at(&command.context.address)
            .await;
        assert!(snapshot.pending.is_empty() && snapshot.resolved.is_empty());
    }
    assert!(
        fixture.file_reads.requests.lock().unwrap().is_empty(),
        "discovery is not a file ToolCall"
    );
    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn discovery_rejects_unauthenticated_or_mismatched_device_routes_and_stale_context() {
    let (fixture, command) = prepare().await;
    std::fs::write(
        PathBuf::from(&command.discovery.target_path).join("AGENTS.md"),
        "target instructions",
    )
    .unwrap();
    let handler = &fixture.file_reads.inner;
    for case in 0..6 {
        let mut invalid = command.clone();
        match case {
            2 => invalid.context.source_device_id = "other-device".into(),
            3 => invalid.context.target_device_id = fixture.target.device_id.clone(),
            4 => invalid.discovery.device_id = fixture.target.device_id.clone(),
            5 => {
                invalid.context.confirmation = Some(DeviceReadConfirmation {
                    approval_id: "not-an-approval".into(),
                    dispatch_token: uuid::Uuid::new_v4(),
                })
            }
            _ => {}
        }
        let bytes =
            serde_json::to_vec(&json!({"command":"agents_instructions", "payload":invalid}))
                .unwrap();
        let reply = match case {
            0 => handler.handle_command(&bytes).await,
            1 => handler.handle_command_from("unrelated-peer", &bytes).await,
            _ => {
                handler
                    .handle_command_from(&fixture.target.device_id, &bytes)
                    .await
            }
        }
        .unwrap();
        let reply: DeviceCommandResponse = serde_json::from_slice(&reply).unwrap();
        assert!(
            matches!(&reply, DeviceCommandResponse::Rejected { reason, .. } if reason == "invalid_agents_instructions_route"),
            "case {case}: {reply:?}"
        );
    }
    // A Turn from a Chat whose selected Agent has since changed can no longer
    // claim the target's instructions.
    let stale = AgentsInstructionCommand {
        context: DeviceReadCheckRequest {
            agent_id: "another-agent".into(),
            ..command.context.clone()
        },
        ..command
    };
    let result = deliver_agents_instructions_via_session(
        fixture.target.runtime.session_manager(),
        &stale,
        Duration::from_secs(8),
    )
    .await;
    assert!(
        matches!(result, Err(DeviceCommandDeliveryError::Rejected { ref reason, .. }) if reason == "agents_instructions_unavailable"),
        "{result:?}"
    );
    fixture.shutdown().await;
}
