use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use agent_client_protocol::schema::v1::{
    PermissionOption, PermissionOptionId, PermissionOptionKind, RequestPermissionOutcome,
    RequestPermissionRequest, ToolCall as AcpToolCall, ToolCallId, ToolCallStatus, ToolCallUpdate,
    ToolCallUpdateFields,
};
use rua_acp::permissions::{
    select_unambiguous_allow_once, selected_response, AcpPermissionRelay,
    AcpPermissionRequestContext, AcpPermissionTaskContext, PermissionRelayResolution,
};
use rua_acp::service::{AcpService, TurnMode};
use rua_acp::tool_projection::AcpToolProjection;
use rua_config::types::acp::{AcpAgentConfig, AcpConfig, ToolParseMode};
use rua_core::event::RuntimeEvent;
use rua_core::id::SessionId;

fn options() -> Vec<PermissionOption> {
    vec![
        PermissionOption::new(
            PermissionOptionId::new("allow_once"),
            "Allow once",
            PermissionOptionKind::AllowOnce,
        ),
        PermissionOption::new(
            PermissionOptionId::new("allow_session"),
            "Allow for session",
            PermissionOptionKind::AllowAlways,
        ),
        PermissionOption::new(
            PermissionOptionId::new("allow_always"),
            "Always allow",
            PermissionOptionKind::AllowAlways,
        ),
        PermissionOption::new(
            PermissionOptionId::new("decline"),
            "Decline",
            PermissionOptionKind::RejectOnce,
        ),
    ]
}

fn sparse_request() -> RequestPermissionRequest {
    RequestPermissionRequest::new(
        "session-1",
        ToolCallUpdate::new(ToolCallId::new("tool-1"), ToolCallUpdateFields::default()),
        options(),
    )
}

fn context() -> AcpPermissionRequestContext {
    AcpPermissionRequestContext {
        agent_id: "agent-1".to_string(),
        chat_id: "chat-1".to_string(),
        chat_address: None,
        runner: "codex".to_string(),
        task: None,
    }
}

#[tokio::test]
async fn task_permission_request_preserves_task_attempt_and_origin_provenance() {
    let relay = AcpPermissionRelay::new();
    let task = AcpPermissionTaskContext {
        task_id: "task-123".to_string(),
        attempt_no: 2,
        origin_turn_id: Some("turn-owner".to_string()),
        origin_tool_call_id: Some("call-create".to_string()),
    };
    let (pending, _resolution) = relay
        .create_request(
            AcpPermissionRequestContext {
                agent_id: "agent-1".to_string(),
                chat_id: "chat-1".to_string(),
                chat_address: None,
                runner: "codex".to_string(),
                task: Some(task.clone()),
            },
            &sparse_request(),
            None,
        )
        .await;

    assert_eq!(pending.task, Some(task));
    assert_eq!(pending.agent_id, "agent-1");
    assert_eq!(pending.chat_id, "chat-1");
    assert_eq!(pending.runner, "codex");
}

fn make_config(env: HashMap<String, String>) -> Arc<AcpConfig> {
    let mut fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    fixture.push("tests/fixtures/mock_acp_agent.py");
    let mut agents = HashMap::new();
    agents.insert(
        "mock_agent".to_string(),
        AcpAgentConfig {
            enabled: true,
            command: if cfg!(windows) {
                "python".to_string()
            } else {
                "python3".to_string()
            },
            args: vec![fixture.to_string_lossy().to_string()],
            env,
            trusted: false,
            tool_parse_mode: ToolParseMode::default(),
            stdio_buffer_limit_bytes: None,
            extra: serde_json::Map::new(),
        },
    );
    Arc::new(AcpConfig {
        agents,
        extra: serde_json::Map::new(),
    })
}

#[tokio::test]
async fn relay_preserves_options_and_returns_exact_reject_selection() {
    let relay = AcpPermissionRelay::new();
    let (pending, resolution) = relay
        .create_request(context(), &sparse_request(), None)
        .await;

    assert_eq!(
        pending
            .options
            .iter()
            .map(|option| (option.option_id.as_str(), option.name.as_str(), option.kind))
            .collect::<Vec<_>>(),
        vec![
            ("allow_once", "Allow once", PermissionOptionKind::AllowOnce),
            (
                "allow_session",
                "Allow for session",
                PermissionOptionKind::AllowAlways
            ),
            (
                "allow_always",
                "Always allow",
                PermissionOptionKind::AllowAlways
            ),
            ("decline", "Decline", PermissionOptionKind::RejectOnce),
        ]
    );

    relay
        .decide("agent-1", "chat-1", &pending.id, "decline")
        .await
        .expect("exact reject option should be selectable");
    let selected = resolution.await.expect("relay resolution");
    assert_eq!(
        selected,
        PermissionRelayResolution::Selected(PermissionOptionId::new("decline"))
    );

    let response = selected_response(PermissionOptionId::new("decline"));
    match response.outcome {
        RequestPermissionOutcome::Selected(selected) => {
            assert_eq!(selected.option_id, PermissionOptionId::new("decline"));
        }
        other => panic!("runner reject is still a selected option, got {other:?}"),
    }
}

#[tokio::test]
async fn unknown_option_does_not_consume_pending_request() {
    let relay = AcpPermissionRelay::new();
    let (pending, _resolution) = relay
        .create_request(context(), &sparse_request(), None)
        .await;

    let error = relay
        .decide("agent-1", "chat-1", &pending.id, "not-an-option")
        .await
        .expect_err("unknown option must be rejected");
    assert!(error.to_string().contains("not-an-option"));
    assert_eq!(relay.snapshot("agent-1", "chat-1").await.pending.len(), 1);
}

#[tokio::test]
async fn cancellation_resolves_waiter_without_selecting_an_option() {
    let relay = AcpPermissionRelay::new();
    let (pending, resolution) = relay
        .create_request(context(), &sparse_request(), None)
        .await;

    relay.cancel_request(&pending.id).await;
    assert_eq!(
        resolution.await.expect("relay resolution"),
        PermissionRelayResolution::Cancelled
    );
    assert!(relay.snapshot("agent-1", "chat-1").await.pending.is_empty());
}

#[tokio::test]
async fn cancelling_one_session_scope_leaves_other_chat_work_in_place() {
    let relay = AcpPermissionRelay::new();
    let (first, first_resolution) = relay
        .create_request_in_scope(
            "session-scope-a".to_string(),
            context(),
            &sparse_request(),
            None,
        )
        .await;
    let (second, _second_resolution) = relay
        .create_request_in_scope(
            "session-scope-b".to_string(),
            context(),
            &sparse_request(),
            None,
        )
        .await;

    relay.cancel_scope("session-scope-a").await;

    assert_eq!(
        first_resolution.await.expect("first relay resolution"),
        PermissionRelayResolution::Cancelled
    );
    let snapshot = relay.snapshot("agent-1", "chat-1").await;
    assert_eq!(
        snapshot
            .pending
            .iter()
            .map(|request| request.id.as_str())
            .collect::<Vec<_>>(),
        vec![second.id.as_str()]
    );
    assert_ne!(first.id, second.id);
}

#[tokio::test]
async fn same_chat_uuid_in_agent_and_project_scopes_never_shares_permission_requests() {
    let relay = AcpPermissionRelay::new();
    let chat_id = "shared-chat-id";
    let agent_address = rua_core::ChatAddress::agent("default", chat_id);
    let project_address = rua_core::ChatAddress::project("project", chat_id);
    let mut agent_context = context();
    agent_context.chat_id = chat_id.into();
    agent_context.chat_address = Some(agent_address.clone());
    let mut project_context = context();
    project_context.chat_id = chat_id.into();
    project_context.chat_address = Some(project_address.clone());
    let (agent_request, _agent_resolution) = relay
        .create_request(agent_context, &sparse_request(), None)
        .await;
    let (project_request, _project_resolution) = relay
        .create_request(project_context, &sparse_request(), None)
        .await;

    assert_eq!(
        relay.snapshot_for_address(&agent_address).await.pending,
        vec![agent_request.clone()]
    );
    assert_eq!(
        relay.snapshot_for_address(&project_address).await.pending,
        vec![project_request.clone()]
    );
    assert_eq!(
        relay
            .decide_for_address(
                &project_address,
                &agent_request.id,
                &agent_request.options[0].option_id,
            )
            .await,
        Err(rua_acp::permissions::AcpPermissionRelayError::WrongScope)
    );
}

#[tokio::test]
async fn sparse_request_is_enriched_from_tool_projection() {
    let mut projection = AcpToolProjection::default();
    projection.apply_tool_call(
        AcpToolCall::new("tool-1", "mcp.knowledge.search")
            .status(ToolCallStatus::InProgress)
            .raw_input(serde_json::json!({
                "server": "knowledge",
                "tool": "search",
                "arguments": {"query": "ACP"}
            })),
    );
    let enriched = projection
        .enrich_tool_call_update(&sparse_request().tool_call)
        .expect("known tool call");
    let relay = AcpPermissionRelay::new();
    let (pending, _resolution) = relay
        .create_request(context(), &sparse_request(), Some(&enriched))
        .await;

    assert_eq!(pending.tool_name, "mcp.knowledge.search");
    assert_eq!(
        pending.tool_input,
        serde_json::json!({
            "server": "knowledge",
            "tool": "search",
            "arguments": {"query": "ACP"}
        })
    );
}

#[test]
fn trusted_selection_requires_exactly_one_allow_once_option() {
    let request = sparse_request();
    assert_eq!(
        select_unambiguous_allow_once(&request),
        Some(PermissionOptionId::new("allow_once"))
    );

    let ambiguous = RequestPermissionRequest::new(
        "session-1",
        request.tool_call.clone(),
        vec![
            PermissionOption::new(
                PermissionOptionId::new("allow-1"),
                "Allow once A",
                PermissionOptionKind::AllowOnce,
            ),
            PermissionOption::new(
                PermissionOptionId::new("allow-2"),
                "Allow once B",
                PermissionOptionKind::AllowOnce,
            ),
        ],
    );
    assert_eq!(select_unambiguous_allow_once(&ambiguous), None);

    let persistent_only = RequestPermissionRequest::new(
        "session-1",
        request.tool_call,
        vec![PermissionOption::new(
            PermissionOptionId::new("allow-always"),
            "Always allow",
            PermissionOptionKind::AllowAlways,
        )],
    );
    assert_eq!(select_unambiguous_allow_once(&persistent_only), None);
}

#[tokio::test]
async fn mock_runner_receives_exact_selected_permission_option() {
    let mut env = HashMap::new();
    env.insert(
        "ACP_MOCK_PERMISSION_SCENARIO".to_string(),
        "codex".to_string(),
    );
    let service = AcpService::new(make_config(env), "/tmp".to_string());
    let chat_id = SessionId::new(uuid::Uuid::new_v4().to_string());
    service
        .open_session(chat_id.clone(), "mock_agent", None)
        .await
        .expect("open session");

    let mut events = service
        .run_turn(
            chat_id.clone(),
            "mock_agent",
            "request permission",
            TurnMode::Message,
            None,
        )
        .await
        .expect("run turn");

    let request_id = loop {
        let event = tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .expect("permission event timeout")
            .expect("event stream closed");
        match event {
            RuntimeEvent::ToolApprovalPending {
                request_id: Some(request_id),
                tool_name,
                prompt,
                ..
            } => {
                assert_eq!(tool_name, "mcp.knowledge.search");
                assert!(prompt.is_some_and(|value| value.contains("ACP")));
                break request_id;
            }
            RuntimeEvent::Error { error } => panic!("turn failed: {error}"),
            _ => {}
        }
    };

    let snapshot = service.permission_snapshot(&chat_id).await;
    assert_eq!(snapshot.pending.len(), 1);
    assert_eq!(snapshot.pending[0].id, request_id);
    assert_eq!(snapshot.pending[0].options.len(), 4);
    assert_eq!(snapshot.pending[0].tool_name, "mcp.knowledge.search");

    service
        .decide_permission(&chat_id, &request_id, "allow_session")
        .await
        .expect("select runner option");

    let mut output = String::new();
    loop {
        let event = tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .expect("turn completion timeout")
            .expect("event stream closed");
        match event {
            RuntimeEvent::TextDelta { delta, .. } => output.push_str(&delta),
            RuntimeEvent::End { .. } => break,
            RuntimeEvent::Error { error } => panic!("turn failed: {error}"),
            _ => {}
        }
    }
    assert_eq!(output, "permission=allow_session");
    assert!(service
        .permission_snapshot(&chat_id)
        .await
        .pending
        .is_empty());

    service.close_all().await;
}

#[tokio::test]
async fn stopping_turn_cancels_pending_permission_relay() {
    let mut env = HashMap::new();
    env.insert(
        "ACP_MOCK_PERMISSION_SCENARIO".to_string(),
        "codex".to_string(),
    );
    let service = AcpService::new(make_config(env), "/tmp".to_string());
    let chat_id = SessionId::new(uuid::Uuid::new_v4().to_string());
    service
        .open_session(chat_id.clone(), "mock_agent", None)
        .await
        .expect("open session");

    let mut events = service
        .run_turn(
            chat_id.clone(),
            "mock_agent",
            "request permission",
            TurnMode::Message,
            None,
        )
        .await
        .expect("run turn");

    loop {
        let event = tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .expect("permission event timeout")
            .expect("event stream closed");
        if matches!(
            event,
            RuntimeEvent::ToolApprovalPending {
                request_id: Some(_),
                ..
            }
        ) {
            break;
        }
    }

    service
        .cancel_turn(&chat_id, "mock_agent")
        .await
        .expect("cancel active turn");

    loop {
        let event = tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .expect("turn cancellation timeout")
            .expect("event stream closed");
        if matches!(event, RuntimeEvent::End { .. }) {
            break;
        }
    }

    assert!(service
        .permission_snapshot(&chat_id)
        .await
        .pending
        .is_empty());
    service.close_all().await;
}

#[tokio::test]
async fn closing_service_sessions_cancels_pending_permission_relay() {
    let mut env = HashMap::new();
    env.insert(
        "ACP_MOCK_PERMISSION_SCENARIO".to_string(),
        "codex".to_string(),
    );
    let service = AcpService::new(make_config(env), "/tmp".to_string());
    let chat_id = SessionId::new(uuid::Uuid::new_v4().to_string());
    let mut events = service
        .run_turn(
            chat_id.clone(),
            "mock_agent",
            "request permission",
            TurnMode::Start,
            None,
        )
        .await
        .expect("run turn");

    loop {
        let event = tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .expect("permission event timeout")
            .expect("event stream closed");
        if matches!(
            event,
            RuntimeEvent::ToolApprovalPending {
                request_id: Some(_),
                ..
            }
        ) {
            break;
        }
    }

    service.close_all().await;
    assert!(service
        .permission_snapshot(&chat_id)
        .await
        .pending
        .is_empty());
}
