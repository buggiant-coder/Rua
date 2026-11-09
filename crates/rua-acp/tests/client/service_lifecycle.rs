use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use rua_acp::client::AcpClientAdapter;
use rua_acp::core::{AcpError, SessionKey};
use rua_acp::service::{AcpService, TurnMode};
use rua_config::types::acp::{AcpAgentConfig, AcpConfig, ToolParseMode};
use rua_core::event::RuntimeEvent;
use rua_core::id::SessionId;

fn make_config_with_echo() -> Arc<AcpConfig> {
    let mut agents = HashMap::new();
    agents.insert(
        "echo_agent".to_string(),
        AcpAgentConfig {
            enabled: true,
            command: "echo".to_string(),
            args: vec!["hello".to_string()],
            env: HashMap::new(),
            trusted: false,
            tool_parse_mode: ToolParseMode::default(),
            stdio_buffer_limit_bytes: None,
            extra: serde_json::Map::new(),
        },
    );
    agents.insert(
        "disabled_agent".to_string(),
        AcpAgentConfig {
            enabled: false,
            command: "echo".to_string(),
            args: vec![],
            env: HashMap::new(),
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

fn make_config_with_mock_agent() -> Arc<AcpConfig> {
    let mut fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    fixture.push("tests/fixtures/mock_acp_agent.py");

    let mut agents = HashMap::new();
    agents.insert(
        "mock_agent".to_string(),
        AcpAgentConfig {
            enabled: true,
            command: mock_acp_command().to_string(),
            args: vec![fixture.to_string_lossy().to_string()],
            env: HashMap::new(),
            trusted: true,
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

fn make_config_with_cancel_waiting_mock_agent() -> Arc<AcpConfig> {
    let mut fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    fixture.push("tests/fixtures/mock_acp_agent.py");

    let mut env = HashMap::new();
    env.insert("ACP_MOCK_WAIT_FOR_CANCEL".to_string(), "1".to_string());
    let mut agents = HashMap::new();
    agents.insert(
        "mock_agent".to_string(),
        AcpAgentConfig {
            enabled: true,
            command: mock_acp_command().to_string(),
            args: vec![fixture.to_string_lossy().to_string()],
            env,
            trusted: true,
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

fn mock_agent_config() -> AcpAgentConfig {
    let mut fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    fixture.push("tests/fixtures/mock_acp_agent.py");

    AcpAgentConfig {
        enabled: true,
        command: mock_acp_command().to_string(),
        args: vec![fixture.to_string_lossy().to_string()],
        env: HashMap::new(),
        trusted: true,
        tool_parse_mode: ToolParseMode::default(),
        stdio_buffer_limit_bytes: None,
        extra: serde_json::Map::new(),
    }
}

fn mock_acp_command() -> &'static str {
    if cfg!(windows) {
        "python"
    } else {
        "python3"
    }
}

fn hanging_python_config() -> AcpAgentConfig {
    AcpAgentConfig {
        enabled: true,
        command: mock_acp_command().to_string(),
        args: vec!["-c".to_string(), "import time; time.sleep(2)".to_string()],
        env: HashMap::new(),
        trusted: true,
        tool_parse_mode: ToolParseMode::default(),
        stdio_buffer_limit_bytes: None,
        extra: serde_json::Map::new(),
    }
}

#[tokio::test]
async fn agent_not_found_returns_error() {
    let config = make_config_with_echo();
    let service = AcpService::new(config, "/tmp".to_string());
    let chat_id = SessionId::new(uuid::Uuid::new_v4().to_string());

    let result = service
        .run_turn(chat_id, "nonexistent", "hello", TurnMode::Start, None)
        .await;

    assert!(result.is_err());
    match result.unwrap_err() {
        AcpError::AgentNotFound(name) => assert_eq!(name, "nonexistent"),
        other => panic!("expected AgentNotFound, got: {other}"),
    }
}

#[tokio::test]
async fn disabled_agent_returns_error() {
    let config = make_config_with_echo();
    let service = AcpService::new(config, "/tmp".to_string());
    let chat_id = SessionId::new(uuid::Uuid::new_v4().to_string());

    let result = service
        .run_turn(chat_id, "disabled_agent", "hello", TurnMode::Start, None)
        .await;

    assert!(result.is_err());
    match result.unwrap_err() {
        AcpError::AgentNotEnabled(name) => assert_eq!(name, "disabled_agent"),
        other => panic!("expected AgentNotEnabled, got: {other}"),
    }
}

#[tokio::test]
async fn message_without_start_returns_not_found() {
    let config = make_config_with_echo();
    let service = AcpService::new(config, "/tmp".to_string());
    let chat_id = SessionId::new(uuid::Uuid::new_v4().to_string());

    let result = service
        .run_turn(chat_id, "echo_agent", "hello", TurnMode::Message, None)
        .await;

    assert!(result.is_err());
    match result.unwrap_err() {
        AcpError::SessionNotFound(_) => {}
        other => panic!("expected SessionNotFound, got: {other}"),
    }
}

#[tokio::test]
async fn close_nonexistent_session_is_ok() {
    let config = make_config_with_echo();
    let service = AcpService::new(config, "/tmp".to_string());
    let chat_id = SessionId::new(uuid::Uuid::new_v4().to_string());

    let result = service.close_chat_session(&chat_id, "echo_agent").await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn close_all_on_empty_service() {
    let config = make_config_with_echo();
    let service = AcpService::new(config, "/tmp".to_string());
    service.close_all().await;
}

#[tokio::test]
async fn duplicate_start_returns_already_exists() {
    let config = make_config_with_mock_agent();
    let service = AcpService::new(config, "/tmp".to_string());
    let chat_id = SessionId::new(uuid::Uuid::new_v4().to_string());

    let _first = service
        .run_turn(
            chat_id.clone(),
            "mock_agent",
            "hello",
            TurnMode::Start,
            None,
        )
        .await;

    let second = service
        .run_turn(chat_id, "mock_agent", "world", TurnMode::Start, None)
        .await;

    assert!(second.is_err());
    match second.unwrap_err() {
        AcpError::SessionAlreadyExists(_) => {}
        other => panic!("expected SessionAlreadyExists, got: {other}"),
    }

    service.close_all().await;
}

#[tokio::test]
async fn restart_replaces_existing_session() {
    let config = make_config_with_mock_agent();
    let service = AcpService::new(config, "/tmp".to_string());
    let chat_id = SessionId::new(uuid::Uuid::new_v4().to_string());

    let _first = service
        .run_turn(
            chat_id.clone(),
            "mock_agent",
            "hello",
            TurnMode::Start,
            None,
        )
        .await;

    let second = service
        .run_turn(chat_id, "mock_agent", "world", TurnMode::Restart, None)
        .await;

    assert!(second.is_ok());

    service.close_all().await;
}

#[tokio::test]
async fn non_text_acp_updates_do_not_panic_inside_tokio_runtime() {
    let config = make_config_with_mock_agent();
    let service = AcpService::new(config, "/tmp".to_string());
    let chat_id = SessionId::new(uuid::Uuid::new_v4().to_string());

    let mut events = service
        .run_turn_with_timeout(
            chat_id,
            "mock_agent",
            "hello",
            TurnMode::Start,
            None,
            Duration::from_secs(5),
        )
        .await
        .expect("mock ACP turn should start");

    let mut output = String::new();
    loop {
        let event = tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .expect("mock ACP event timed out")
            .expect("mock ACP event stream closed early");

        match event {
            RuntimeEvent::TextDelta { delta, .. } => output.push_str(&delta),
            RuntimeEvent::End { .. } => break,
            RuntimeEvent::Error { error } => panic!("mock ACP turn failed: {error}"),
            _ => {}
        }
    }

    assert_eq!(output, "mock-acp-ok");
    service.close_all().await;
}

#[tokio::test]
async fn cancel_turn_ends_active_mock_runner_turn_without_closing_session() {
    let config = make_config_with_cancel_waiting_mock_agent();
    let service = AcpService::new(config, "/tmp".to_string());
    let chat_id = SessionId::new(uuid::Uuid::new_v4().to_string());

    let mut events = service
        .run_turn_with_timeout(
            chat_id.clone(),
            "mock_agent",
            "wait",
            TurnMode::Start,
            None,
            Duration::from_secs(5),
        )
        .await
        .expect("mock ACP turn should start");

    let first = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .expect("mock ACP first event timed out")
        .expect("mock ACP event stream closed early");
    match first {
        RuntimeEvent::TextDelta { delta, .. } => assert_eq!(delta, "mock-acp-started"),
        other => panic!("expected initial ACP text delta, got {other:?}"),
    }

    service
        .cancel_turn(&chat_id, "mock_agent")
        .await
        .expect("cancel_turn should target the active ACP session");

    loop {
        let event = tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .expect("mock ACP event timed out after cancel")
            .expect("mock ACP event stream closed early after cancel");

        match event {
            RuntimeEvent::TextDelta { delta, .. } => {
                assert!(
                    !delta.contains("已请求中断"),
                    "ACP runtime should not emit its own stop hint"
                );
            }
            RuntimeEvent::End { .. } => break,
            RuntimeEvent::Error { error } => panic!("mock ACP turn failed after cancel: {error}"),
            _ => {}
        }
    }

    let mut next_events = service
        .run_turn_with_timeout(
            chat_id.clone(),
            "mock_agent",
            "next",
            TurnMode::Message,
            None,
            Duration::from_secs(5),
        )
        .await
        .expect("ACP session should remain usable after turn cancel");

    let second_first = tokio::time::timeout(Duration::from_secs(5), next_events.recv())
        .await
        .expect("mock ACP second first event timed out")
        .expect("mock ACP second event stream closed early");
    match second_first {
        RuntimeEvent::TextDelta { delta, .. } => assert_eq!(delta, "mock-acp-started"),
        other => panic!("expected second ACP text delta, got {other:?}"),
    }

    service
        .cancel_turn(&chat_id, "mock_agent")
        .await
        .expect("second cancel should still target same ACP session");
    while let Some(event) = tokio::time::timeout(Duration::from_secs(5), next_events.recv())
        .await
        .expect("mock ACP second turn timed out")
    {
        if matches!(event, RuntimeEvent::End { .. }) {
            break;
        }
    }

    service.close_all().await;
}

#[tokio::test]
async fn initialize_probe_does_not_create_session_or_prompt() {
    let mut config = mock_agent_config();
    config
        .env
        .insert("ACP_MOCK_FAIL_ON_TURN".to_string(), "1".to_string());

    AcpClientAdapter::probe_initialize(&config)
        .await
        .expect("initialize-only probe must not call session/new or session/prompt");
}

#[cfg(unix)]
#[tokio::test]
async fn initialize_probe_cleans_up_runner_process_group() {
    let marker = format!("ACP_MOCK_CHILD_MARKER_{}", uuid::Uuid::new_v4());
    let mut config = mock_agent_config();
    config
        .env
        .insert("ACP_MOCK_CHILD_MARKER".to_string(), marker.clone());

    AcpClientAdapter::probe_initialize(&config)
        .await
        .expect("initialize probe should succeed");

    for _ in 0..20 {
        if !process_list_contains(&marker) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("ACP initialize probe leaked child process marker {marker}");
}

#[tokio::test]
async fn session_start_probe_creates_session_but_does_not_prompt() {
    let mut config = mock_agent_config();
    config
        .env
        .insert("ACP_MOCK_FAIL_ON_PROMPT".to_string(), "1".to_string());

    AcpClientAdapter::probe_session_start(&config, "/tmp".to_string())
        .await
        .expect("session-start probe must not call session/prompt");
}

#[cfg(unix)]
fn process_list_contains(needle: &str) -> bool {
    let Ok(output) = std::process::Command::new("ps")
        .args(["-axo", "command"])
        .output()
    else {
        return false;
    };
    String::from_utf8_lossy(&output.stdout).contains(needle)
}

#[tokio::test]
async fn session_start_probe_times_out_when_runner_does_not_respond() {
    let timeout = Duration::from_millis(25);
    let err = AcpClientAdapter::probe_session_start_with_timeout(
        &hanging_python_config(),
        "/tmp".to_string(),
        timeout,
    )
    .await
    .expect_err("session-start probe must time out when the runner does not respond");

    assert!(
        matches!(err, AcpError::Timeout(actual) if actual == timeout),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn session_start_probe_surfaces_session_new_failure() {
    let mut config = mock_agent_config();
    config
        .env
        .insert("ACP_MOCK_FAIL_ON_TURN".to_string(), "1".to_string());

    let err = AcpClientAdapter::probe_session_start(&config, "/tmp".to_string())
        .await
        .expect_err("session-start probe must fail when session/new fails");
    assert!(
        err.to_string().contains("session/new should not be called"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn session_key_display() {
    let key = SessionKey {
        chat_id: SessionId::new(uuid::Uuid::new_v4().to_string()),
        agent_name: "test_agent".to_string(),
    };
    let display = format!("{key}");
    assert!(display.contains("test_agent"));
}
