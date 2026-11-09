use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use rua_acp::service::{AcpService, TurnMode};
use rua_acp::AcpSessionId;
use rua_config::types::acp::{AcpAgentConfig, AcpConfig, ToolParseMode};
use rua_core::event::RuntimeEvent;
use rua_core::id::SessionId;

fn make_config(env: HashMap<String, String>) -> Arc<AcpConfig> {
    let mut fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    fixture.push("tests/fixtures/mock_acp_agent.py");

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

fn mock_acp_command() -> &'static str {
    if cfg!(windows) {
        "python"
    } else {
        "python3"
    }
}

async fn drain_text(rx: &mut tokio::sync::mpsc::UnboundedReceiver<RuntimeEvent>) -> String {
    let mut out = String::new();
    loop {
        let ev = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("event timed out")
            .expect("stream closed early");
        match ev {
            RuntimeEvent::TextDelta { delta, .. } => out.push_str(&delta),
            RuntimeEvent::End { .. } => break,
            RuntimeEvent::Error { error } => panic!("turn failed: {error}"),
            _ => {}
        }
    }
    out
}

#[tokio::test]
async fn two_turns_reuse_the_same_session_id() {
    let mut env = HashMap::new();
    env.insert("ACP_MOCK_ECHO_SESSION_ID".to_string(), "1".to_string());
    let config = make_config(env);
    let service = AcpService::new(config, "/tmp".to_string());
    let chat_id = SessionId::new(uuid::Uuid::new_v4().to_string());

    let session_id = service
        .open_session(chat_id.clone(), "mock_agent", None)
        .await
        .expect("open_session should succeed");
    assert!(
        !session_id.trim().is_empty(),
        "fresh ACP session id should be returned"
    );

    let mut turn1 = service
        .run_turn(
            chat_id.clone(),
            "mock_agent",
            "first",
            TurnMode::Message,
            None,
        )
        .await
        .expect("first turn should start");
    let out1 = drain_text(&mut turn1).await;

    let mut turn2 = service
        .run_turn(
            chat_id.clone(),
            "mock_agent",
            "second",
            TurnMode::Message,
            None,
        )
        .await
        .expect("second turn should start");
    let out2 = drain_text(&mut turn2).await;

    assert_eq!(
        out1, out2,
        "both turns should reference the same session id (got {out1:?} and {out2:?})"
    );
    assert!(
        out1.starts_with("sid="),
        "expected sid= echo from mock agent, got {out1:?}"
    );

    service.close_all().await;
}

#[tokio::test]
async fn open_session_resume_uses_load_session_and_supplied_id() {
    let mut env = HashMap::new();
    env.insert("ACP_MOCK_ECHO_SESSION_ID".to_string(), "1".to_string());
    let config = make_config(env);
    let service = AcpService::new(config, "/tmp".to_string());
    let chat_id = SessionId::new(uuid::Uuid::new_v4().to_string());

    let supplied = AcpSessionId::from("pre-existing-sess".to_string());
    let opened = service
        .open_session_resume(chat_id.clone(), "mock_agent", supplied.clone(), None)
        .await
        .expect("open_session_resume should succeed");
    assert_eq!(opened.session_id, "pre-existing-sess");

    let mut turn = service
        .run_turn(
            chat_id.clone(),
            "mock_agent",
            "continue",
            TurnMode::Message,
            None,
        )
        .await
        .expect("resume turn should start");
    let out = drain_text(&mut turn).await;

    assert_eq!(
        out, "sid=pre-existing-sess",
        "resumed session should keep the supplied id, got {out:?}"
    );

    service.close_all().await;
}

#[tokio::test]
async fn open_session_resume_returns_load_history_without_replaying_it_on_next_turn() {
    let mut env = HashMap::new();
    env.insert("ACP_MOCK_ECHO_SESSION_ID".to_string(), "1".to_string());
    env.insert(
        "ACP_MOCK_LOAD_HISTORY".to_string(),
        "previous ACP transcript".to_string(),
    );
    let config = make_config(env);
    let service = AcpService::new(config, "/tmp".to_string());
    let chat_id = SessionId::new(uuid::Uuid::new_v4().to_string());

    let supplied = AcpSessionId::from("pre-existing-sess".to_string());
    let history = service
        .open_session_resume(chat_id.clone(), "mock_agent", supplied.clone(), None)
        .await
        .expect("open_session_resume should succeed");

    assert_eq!(
        history.history_preview.as_deref(),
        Some("previous ACP transcript")
    );

    let mut turn = service
        .run_turn(
            chat_id.clone(),
            "mock_agent",
            "continue",
            TurnMode::Message,
            None,
        )
        .await
        .expect("resume turn should start");
    let out = drain_text(&mut turn).await;

    assert_eq!(out, "sid=pre-existing-sess");
    assert!(
        !out.contains("previous ACP transcript"),
        "load replay should be drained into preview, not replayed in the next turn"
    );

    service.close_all().await;
}

#[tokio::test]
async fn open_session_resume_captures_load_history_sent_before_load_response() {
    let mut env = HashMap::new();
    env.insert("ACP_MOCK_ECHO_SESSION_ID".to_string(), "1".to_string());
    env.insert(
        "ACP_MOCK_LOAD_HISTORY".to_string(),
        "early ACP transcript".to_string(),
    );
    env.insert(
        "ACP_MOCK_LOAD_HISTORY_BEFORE_RESPONSE".to_string(),
        "1".to_string(),
    );
    let config = make_config(env);
    let service = AcpService::new(config, "/tmp".to_string());
    let chat_id = SessionId::new(uuid::Uuid::new_v4().to_string());

    let supplied = AcpSessionId::from("pre-existing-sess".to_string());
    let history = service
        .open_session_resume(chat_id.clone(), "mock_agent", supplied.clone(), None)
        .await
        .expect("open_session_resume should succeed");

    assert_eq!(
        history.history_preview.as_deref(),
        Some("early ACP transcript")
    );

    service.close_all().await;
}

#[tokio::test]
async fn open_session_resume_includes_user_message_chunks_in_load_history() {
    let mut env = HashMap::new();
    env.insert(
        "ACP_MOCK_LOAD_HISTORY".to_string(),
        "previous user prompt".to_string(),
    );
    env.insert(
        "ACP_MOCK_LOAD_HISTORY_KIND".to_string(),
        "user_message_chunk".to_string(),
    );
    let config = make_config(env);
    let service = AcpService::new(config, "/tmp".to_string());
    let chat_id = SessionId::new(uuid::Uuid::new_v4().to_string());

    let history = service
        .open_session_resume(
            chat_id,
            "mock_agent",
            AcpSessionId::from("pre-existing-sess".to_string()),
            None,
        )
        .await
        .expect("open_session_resume should succeed");

    assert_eq!(
        history.history_preview.as_deref(),
        Some("previous user prompt")
    );

    service.close_all().await;
}

#[tokio::test]
async fn open_session_resume_waits_for_delayed_load_history_before_accepting_prompts() {
    let mut env = HashMap::new();
    env.insert("ACP_MOCK_ECHO_SESSION_ID".to_string(), "1".to_string());
    env.insert(
        "ACP_MOCK_LOAD_HISTORY".to_string(),
        "delayed ACP transcript".to_string(),
    );
    env.insert(
        "ACP_MOCK_LOAD_HISTORY_DELAY_MS".to_string(),
        "600".to_string(),
    );
    let config = make_config(env);
    let service = AcpService::new(config, "/tmp".to_string());
    let chat_id = SessionId::new(uuid::Uuid::new_v4().to_string());

    let supplied = AcpSessionId::from("pre-existing-sess".to_string());
    let history = service
        .open_session_resume(chat_id.clone(), "mock_agent", supplied.clone(), None)
        .await
        .expect("open_session_resume should wait for delayed history");

    assert_eq!(
        history.history_preview.as_deref(),
        Some("delayed ACP transcript")
    );

    let mut turn = service
        .run_turn(
            chat_id.clone(),
            "mock_agent",
            "continue",
            TurnMode::Message,
            None,
        )
        .await
        .expect("resume turn should start after load completes");
    let out = drain_text(&mut turn).await;

    assert_eq!(out, "sid=pre-existing-sess");
    assert!(
        !out.contains("delayed ACP transcript"),
        "delayed load replay should not leak into the first prompt"
    );

    service.close_all().await;
}

#[tokio::test]
async fn open_session_resume_ignores_available_commands_before_load_history() {
    let mut env = HashMap::new();
    env.insert("ACP_MOCK_ECHO_SESSION_ID".to_string(), "1".to_string());
    env.insert(
        "ACP_MOCK_LOAD_HISTORY".to_string(),
        "history after commands".to_string(),
    );
    env.insert(
        "ACP_MOCK_LOAD_AVAILABLE_COMMANDS_BEFORE_HISTORY".to_string(),
        "1".to_string(),
    );
    let config = make_config(env);
    let service = AcpService::new(config, "/tmp".to_string());
    let chat_id = SessionId::new(uuid::Uuid::new_v4().to_string());

    let supplied = AcpSessionId::from("pre-existing-sess".to_string());
    let history = service
        .open_session_resume(chat_id.clone(), "mock_agent", supplied.clone(), None)
        .await
        .expect("open_session_resume should drain history after available commands");

    assert_eq!(
        history.history_preview.as_deref(),
        Some("history after commands")
    );

    let mut turn = service
        .run_turn(
            chat_id.clone(),
            "mock_agent",
            "continue",
            TurnMode::Message,
            None,
        )
        .await
        .expect("resume turn should start after load completes");
    let out = drain_text(&mut turn).await;

    assert_eq!(out, "sid=pre-existing-sess");
    assert!(
        !out.contains("history after commands"),
        "load replay after available commands should not leak into first prompt"
    );

    service.close_all().await;
}

#[tokio::test]
async fn open_session_resume_drains_large_load_history_before_first_prompt() {
    const PREVIEW_LIMIT_BYTES: usize = 1024 * 1024;

    let mut env = HashMap::new();
    env.insert("ACP_MOCK_ECHO_SESSION_ID".to_string(), "1".to_string());
    env.insert("ACP_MOCK_LOAD_HISTORY".to_string(), "x".repeat(4 * 1024));
    env.insert(
        "ACP_MOCK_LOAD_HISTORY_REPEAT".to_string(),
        "300".to_string(),
    );
    env.insert("ACP_MOCK_LOAD_HISTORY_INDEXED".to_string(), "1".to_string());
    let config = make_config(env);
    let service = AcpService::new(config, "/tmp".to_string());
    let chat_id = SessionId::new(uuid::Uuid::new_v4().to_string());

    let supplied = AcpSessionId::from("pre-existing-sess".to_string());
    let history = service
        .open_session_resume(chat_id.clone(), "mock_agent", supplied.clone(), None)
        .await
        .expect("open_session_resume should drain large load replay");

    let preview = history
        .history_preview
        .as_deref()
        .expect("large load replay should produce a bounded preview");
    assert!(
        preview.len() <= PREVIEW_LIMIT_BYTES,
        "preview should be capped at 1 MiB, got {} bytes",
        preview.len()
    );
    assert!(
        preview.starts_with("[Earlier ACP history omitted]"),
        "truncated preview should explain that earlier history was omitted"
    );
    assert!(
        !preview.contains("000000:"),
        "oldest history chunks should be discarded"
    );
    assert!(
        preview.contains("000299:"),
        "most recent history chunk should be retained"
    );

    let mut turn = service
        .run_turn(
            chat_id.clone(),
            "mock_agent",
            "continue",
            TurnMode::Message,
            None,
        )
        .await
        .expect("resume turn should start after load completes");
    let out = drain_text(&mut turn).await;

    assert_eq!(out, "sid=pre-existing-sess");
    assert!(
        !out.contains("000299:"),
        "load replay should be fully drained before first prompt"
    );

    service.close_all().await;
}
