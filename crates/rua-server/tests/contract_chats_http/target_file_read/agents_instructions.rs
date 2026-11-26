use super::*;
use rua_core::agents_instruction_discovery::{
    AgentsInstructionDiscoveryRequest, AgentsInstructionTargetKind, AGENTS_INSTRUCTIONS_MAX_BYTES,
};
use tokio_util::sync::CancellationToken;

fn discovery(f: &Fixture, path: &Path) -> AgentsInstructionDiscoveryRequest {
    AgentsInstructionDiscoveryRequest {
        device_id: f.device.clone(),
        target_path: path.to_str().unwrap().into(),
        target_kind: AgentsInstructionTargetKind::Directory,
        max_bytes: AGENTS_INSTRUCTIONS_MAX_BYTES,
        ..Default::default()
    }
}

async fn assert_no_approvals(f: &Fixture) {
    let snapshot = f
        .app
        .state
        .security
        .approval_authority()
        .snapshot_at(&f.address)
        .await;
    assert!(snapshot.pending.is_empty());
    assert!(snapshot.resolved.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn current_permissions_load_target_instructions_without_a_tool_invocation_or_cache() {
    let f = Fixture::materialize(build_agent().await).await;
    let dir = f.app.state.rua_root.join("instructions");
    std::fs::create_dir(&dir).unwrap();
    let file = dir.join("AGENTS.md");
    std::fs::write(&file, "first target instructions").unwrap();
    let request = discovery(&f, &dir);
    // Discovery carries Chat context, without fabricating a ToolCall or its arguments.
    let context = TargetChatRead {
        tool_name: "",
        tool_call_id: "",
        args: &Value::Null,
        ..f.request(&Value::Null)
    };
    for content in ["first target instructions", "updated target instructions"] {
        std::fs::write(&file, content).unwrap();
        let result = f
            .context
            .discover_agents_instructions(&context, &request, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(result.sources.len(), 1);
        assert_eq!(result.sources[0].content, content);
        assert_eq!(result.sources[0].device_id, f.device);
        assert_eq!(
            result.sources[0].path,
            file.canonicalize().unwrap().to_str().unwrap()
        );
        assert_eq!(
            result.sources[0].scope_directory,
            dir.canonicalize().unwrap().to_str().unwrap()
        );
    }
    assert_no_approvals(&f).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn denied_override_falls_back_but_confirmation_and_disabled_read_skip_without_approval() {
    let f = Fixture::materialize(build_agent().await).await;
    let dir = f.app.state.rua_root.join("instructions");
    std::fs::create_dir(&dir).unwrap();
    let override_path = dir.join("AGENTS.override.md");
    std::fs::write(&override_path, "must not be disclosed").unwrap();
    let override_path = override_path.canonicalize().unwrap();
    std::fs::write(dir.join("AGENTS.md"), "allowed fallback").unwrap();
    let security = &f.app.state.security;
    security
        .approval_authority()
        .create_agent_rule(
            AGENT,
            PermissionRuleInput {
                label: "Denied override".into(),
                enabled: true,
                effect: PermissionRuleEffect::Deny,
                matcher: ApprovalReuseMatcher::ToolCall {
                    tool_name: "read".into(),
                    input_params: Some(
                        json!({"device_id": f.device, "file_path": override_path}).to_string(),
                    ),
                },
            },
        )
        .await
        .unwrap();
    let request = discovery(&f, &dir);
    let result = f
        .context
        .discover_agents_instructions(&f.request(&Value::Null), &request, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(result.sources.len(), 1);
    assert_eq!(result.sources[0].content, "allowed fallback");
    let ordinary = security.config_store().load(AGENT).unwrap();
    let mut policy = ordinary.clone();
    policy.tool_guard.approval_mode = "ask_first".into();
    security.save_and_invalidate(AGENT, &policy).unwrap();
    let result = f
        .context
        .discover_agents_instructions(&f.request(&Value::Null), &request, CancellationToken::new())
        .await
        .unwrap();
    assert!(result.sources.is_empty());
    security.save_and_invalidate(AGENT, &ordinary).unwrap();
    select_read(&f.app, false).await;
    let result = f
        .context
        .discover_agents_instructions(&f.request(&Value::Null), &request, CancellationToken::new())
        .await
        .unwrap();
    assert!(result.sources.is_empty());
    assert_no_approvals(&f).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wrong_device_stale_chat_context_and_cancellation_never_return_instructions() {
    let f = Fixture::materialize(build_agent().await).await;
    let dir = f.app.state.rua_root.join("instructions");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("AGENTS.md"), "private target instructions").unwrap();
    let request = discovery(&f, &dir);
    let wrong_device = AgentsInstructionDiscoveryRequest {
        device_id: "other-device".into(),
        ..request.clone()
    };
    assert!(f
        .context
        .discover_agents_instructions(
            &f.request(&Value::Null),
            &wrong_device,
            CancellationToken::new()
        )
        .await
        .is_err());
    for context in [
        TargetChatRead {
            authority_device_id: "other-device",
            ..f.request(&Value::Null)
        },
        TargetChatRead {
            agent_id: "other-agent",
            ..f.request(&Value::Null)
        },
    ] {
        assert!(f
            .context
            .discover_agents_instructions(&context, &request, CancellationToken::new())
            .await
            .is_err());
    }
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(matches!(
        f.context
            .discover_agents_instructions(&f.request(&Value::Null), &request, cancel)
            .await,
        Err(rua_core::RuaError::Cancelled)
    ));
    assert_no_approvals(&f).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn instruction_content_stays_within_the_requested_budget() {
    let f = Fixture::materialize(build_agent().await).await;
    let dir = f.app.state.rua_root.join("instructions");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("AGENTS.md"), "路".repeat(100)).unwrap();
    let mut request = discovery(&f, &dir);
    request.max_bytes = 16;
    let result = f
        .context
        .discover_agents_instructions(&f.request(&Value::Null), &request, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(result.sources.len(), 1);
    assert_eq!(result.sources[0].content, "路".repeat(5));
    assert!(result.truncated && result.sources[0].truncated);
    assert_no_approvals(&f).await;
}
