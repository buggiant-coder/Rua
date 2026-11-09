use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use agent_client_protocol::schema::v1::{
    ToolCall as AcpToolCall, ToolCallId, ToolCallStatus, ToolCallUpdate, ToolCallUpdateFields,
    ToolKind,
};
use rua_acp::service::{AcpService, TurnMode};
use rua_acp::tool_projection::AcpToolProjection;
use rua_acp::AcpSessionId;
use rua_config::types::acp::{AcpAgentConfig, AcpConfig, ToolParseMode};
use rua_core::event::RuntimeEvent;
use rua_core::id::SessionId;

fn make_config(env: HashMap<String, String>) -> Arc<AcpConfig> {
    make_config_with_mode(env, ToolParseMode::default())
}

fn make_config_with_mode(
    env: HashMap<String, String>,
    tool_parse_mode: ToolParseMode,
) -> Arc<AcpConfig> {
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
            trusted: true,
            tool_parse_mode,
            stdio_buffer_limit_bytes: None,
            extra: serde_json::Map::new(),
        },
    );
    Arc::new(AcpConfig {
        agents,
        extra: serde_json::Map::new(),
    })
}

fn content_text(event: &RuntimeEvent) -> String {
    let RuntimeEvent::ToolResult { result } = event else {
        return String::new();
    };
    result
        .content
        .iter()
        .filter_map(|part| match part {
            agent_types::content::ContentPart::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn legacy_parse_modes_share_identical_tool_name_input_and_output() {
    for mode in [
        ToolParseMode::CallTitle,
        ToolParseMode::CallDetail,
        ToolParseMode::UpdateDetail,
    ] {
        let mut env = HashMap::new();
        env.insert(
            "ACP_MOCK_TOOL_SCENARIO".to_string(),
            "interleaved".to_string(),
        );
        let service = AcpService::new(make_config_with_mode(env, mode), "/tmp".to_string());
        let chat_id = SessionId::new(uuid::Uuid::new_v4().to_string());
        let mut rx = service
            .run_turn(
                chat_id,
                "mock_agent",
                "exercise tools",
                TurnMode::Start,
                None,
            )
            .await
            .expect("start turn");
        let mut call = None;
        let mut result = None;
        loop {
            let event = tokio::time::timeout(Duration::from_secs(5), rx.recv())
                .await
                .expect("event timeout")
                .expect("stream closed");
            match event {
                RuntimeEvent::ToolCall { call: next } if next.id == "call-a" => {
                    call = Some(next);
                }
                RuntimeEvent::ToolResult { result: next } if next.call_id == "call-a" => {
                    result = Some(next);
                }
                RuntimeEvent::End { .. } => break,
                RuntimeEvent::Error { error } => panic!("turn failed: {error}"),
                _ => {}
            }
        }
        let call = call.expect("terminal bash call");
        let result = result.expect("terminal bash result");
        assert_eq!(call.name, "bash");
        assert_eq!(
            call.arguments,
            serde_json::json!({"command": "printf final", "cwd": "/workspace"})
        );
        assert_eq!(result.name.as_deref(), Some("bash"));
        assert_eq!(
            result.metadata.get("acp_raw_output"),
            Some(&serde_json::json!({
                "formatted_output": "final",
                "exit_code": 0
            }))
        );
        service.close_all().await;
    }
}

#[test]
fn sparse_terminal_update_keeps_title_input_and_complete_output() {
    let mut projection = AcpToolProjection::default();
    let started = projection.apply_tool_call(
        AcpToolCall::new("call-a", "bash")
            .kind(ToolKind::Execute)
            .status(ToolCallStatus::InProgress)
            .raw_input(serde_json::json!({
                "command": "printf first",
                "cwd": "/workspace"
            })),
    );

    assert_eq!(started.snapshot().index, 0);
    assert_eq!(started.snapshot().tool_call.title, "bash");

    let terminal = projection
        .apply_tool_call_update(ToolCallUpdate::new(
            ToolCallId::new("call-a"),
            ToolCallUpdateFields::new()
                .status(ToolCallStatus::Completed)
                .raw_output(serde_json::json!({
                    "formatted_output": "first",
                    "exit_code": 0
                })),
        ))
        .expect("known tool call should accept sparse update");

    assert_eq!(terminal.snapshot().tool_call.title, "bash");
    assert_eq!(
        terminal.snapshot().tool_call.raw_input,
        Some(serde_json::json!({
            "command": "printf first",
            "cwd": "/workspace"
        }))
    );
    assert!(terminal.became_terminal());

    let events = terminal.into_runtime_events();
    assert_eq!(events.len(), 2);
    match &events[0] {
        RuntimeEvent::ToolCall { call } => {
            assert_eq!(call.name, "bash");
            assert_eq!(
                call.arguments,
                serde_json::json!({
                    "command": "printf first",
                    "cwd": "/workspace"
                })
            );
        }
        other => panic!("expected authoritative ToolCall, got {other:?}"),
    }
    match &events[1] {
        RuntimeEvent::ToolResult { result } => {
            assert_eq!(result.name.as_deref(), Some("bash"));
            assert!(!result.is_error);
            assert_eq!(
                result.metadata.get("acp_raw_output"),
                Some(&serde_json::json!({
                    "formatted_output": "first",
                    "exit_code": 0
                }))
            );
            assert!(content_text(&events[1]).contains("\"formatted_output\": \"first\""));
        }
        other => panic!("expected terminal ToolResult, got {other:?}"),
    }
}

#[test]
fn replacement_snapshots_overwrite_and_interleaved_calls_keep_unique_indices() {
    let mut projection = AcpToolProjection::default();
    projection.apply_tool_call(
        AcpToolCall::new("call-a", "bash")
            .status(ToolCallStatus::InProgress)
            .raw_input(serde_json::json!({"command": "old"})),
    );
    let second = projection.apply_tool_call(
        AcpToolCall::new("call-b", "read")
            .status(ToolCallStatus::InProgress)
            .raw_input(serde_json::json!({"path": "README.md"})),
    );
    let replaced = projection
        .apply_tool_call_update(ToolCallUpdate::new(
            ToolCallId::new("call-a"),
            ToolCallUpdateFields::new()
                .raw_input(serde_json::json!({"command": "new"}))
                .raw_output(serde_json::json!({"chunk": "replacement"})),
        ))
        .expect("known tool call");

    assert_eq!(second.snapshot().index, 1);
    assert_eq!(replaced.snapshot().index, 0);
    assert_eq!(
        replaced.snapshot().tool_call.raw_input,
        Some(serde_json::json!({"command": "new"}))
    );
    assert_eq!(
        replaced.snapshot().tool_call.raw_output,
        Some(serde_json::json!({"chunk": "replacement"}))
    );
    assert_ne!(
        replaced.snapshot().index,
        second.snapshot().index,
        "interleaved calls must never share a projection index"
    );
}

#[tokio::test]
async fn mock_runner_projects_interleaved_calls_without_delta_corruption() {
    let mut env = HashMap::new();
    env.insert(
        "ACP_MOCK_TOOL_SCENARIO".to_string(),
        "interleaved".to_string(),
    );
    let service = AcpService::new(make_config(env), "/tmp".to_string());
    let chat_id = SessionId::new(uuid::Uuid::new_v4().to_string());
    service
        .open_session(chat_id.clone(), "mock_agent", None)
        .await
        .expect("open session");

    let mut rx = service
        .run_turn(
            chat_id,
            "mock_agent",
            "exercise tools",
            TurnMode::Message,
            None,
        )
        .await
        .expect("start turn");

    let mut final_calls = HashMap::new();
    let mut results = HashMap::new();
    loop {
        let event = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("event timeout")
            .expect("stream closed");
        match event {
            RuntimeEvent::ToolCall { call } => {
                final_calls.insert(call.id.clone(), call);
            }
            RuntimeEvent::ToolResult { result } => {
                results.insert(result.call_id.clone(), result);
            }
            RuntimeEvent::ToolCallDelta { .. } | RuntimeEvent::ToolResultDelta { .. } => {
                panic!("ACP snapshot updates must not be emitted as append-only deltas");
            }
            RuntimeEvent::End { .. } => break,
            RuntimeEvent::Error { error } => panic!("turn failed: {error}"),
            _ => {}
        }
    }

    assert_eq!(final_calls["call-a"].name, "bash");
    assert_eq!(
        final_calls["call-a"].arguments,
        serde_json::json!({"command": "printf final", "cwd": "/workspace"})
    );
    assert_eq!(final_calls["call-b"].name, "read");
    assert_eq!(
        final_calls["call-b"].arguments,
        serde_json::json!({"path": "README.md"})
    );
    assert_eq!(results["call-a"].name.as_deref(), Some("bash"));
    assert_eq!(results["call-b"].name.as_deref(), Some("read"));
    assert_eq!(
        results["call-a"].metadata.get("acp_raw_output"),
        Some(&serde_json::json!({
            "formatted_output": "final",
            "exit_code": 0
        }))
    );

    service.close_all().await;
}

#[tokio::test]
async fn resumed_history_uses_projection_for_sparse_terminal_updates() {
    let mut env = HashMap::new();
    env.insert(
        "ACP_MOCK_LOAD_TOOL_SCENARIO".to_string(),
        "interleaved".to_string(),
    );
    let service = AcpService::new(make_config(env), "/tmp".to_string());
    let chat_id = SessionId::new(uuid::Uuid::new_v4().to_string());

    let opened = service
        .open_session_resume(
            chat_id,
            "mock_agent",
            AcpSessionId::from("existing-session".to_string()),
            None,
        )
        .await
        .expect("resume session");
    let preview = opened.history_preview.expect("history preview");

    assert!(preview.contains("[tool] bash"));
    assert!(preview.contains("[tool] read"));
    assert!(preview.contains("\"formatted_output\": \"final\""));
    assert!(preview.contains("\"content\": \"README\""));

    service.close_all().await;
}
