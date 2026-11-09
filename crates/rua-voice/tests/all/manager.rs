#[path = "../voice_test_helpers.rs"]
mod voice_test_helpers;

use rua_voice::{VoiceManager, MAX_PENDING_WS_TOKENS};
use std::sync::Arc;
use tempfile::TempDir;

mod stub_runtime {
    use async_trait::async_trait;
    use futures::stream;
    use rua_contract::agent::{AgentRequest, AgentRuntime, RuntimeWarmupContext};
    use rua_core::event::RuntimeEvent;
    use rua_core::{Result, RuaError};
    use std::pin::Pin;

    pub struct StubRuntime;

    #[async_trait]
    impl AgentRuntime for StubRuntime {
        async fn stream(
            &self,
            _req: AgentRequest,
        ) -> Result<
            Pin<
                Box<dyn futures::Stream<Item = std::result::Result<RuntimeEvent, RuaError>> + Send>,
            >,
        > {
            let events = vec![
                Ok(RuntimeEvent::Start {
                    session_id: "test".into(),
                    agent_id: None,
                }),
                Ok(RuntimeEvent::TextDelta {
                    delta: "stub reply".into(),
                    message_id: None,
                }),
                Ok(RuntimeEvent::End {
                    finish_reason: Some("stop".into()),
                }),
            ];
            Ok(Box::pin(stream::iter(events)))
        }

        async fn cancel(&self, _session_id: &rua_core::id::SessionId) -> Result<()> {
            Ok(())
        }
        async fn warmup(&self, _ctx: RuntimeWarmupContext) -> Result<()> {
            Ok(())
        }
        async fn shutdown(&self) -> Result<()> {
            Ok(())
        }
    }
}

async fn manager() -> (VoiceManager, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    let data_dir = tmp.path().join("data");
    let secret_dir = tmp.path().join("secret");
    std::fs::create_dir_all(&data_dir).unwrap();
    rua_config::save_config(
        &rua_config::RootConfig::default(),
        &data_dir.join("config.json"),
    )
    .unwrap();
    std::fs::create_dir_all(&secret_dir).unwrap();
    let runtime: Arc<dyn rua_contract::agent::AgentRuntime> = Arc::new(stub_runtime::StubRuntime);
    let registry =
        voice_test_helpers::StubRegistry::with_runtime("default", tmp.path(), runtime).await;
    let mgr = VoiceManager::with_dirs(
        "test-secret".into(),
        data_dir,
        secret_dir,
        registry,
        "default".into(),
    )
    .expect("valid test configuration");
    (mgr, tmp)
}

fn token_from_twiml(twiml: &str) -> String {
    twiml
        .split("token=")
        .nth(1)
        .and_then(|s| s.split('"').next())
        .expect("token in twiml")
        .to_string()
}

#[tokio::test]
async fn handle_incoming_issues_token_and_renders_twiml() {
    let (mgr, _tmp) = manager().await;

    let twiml = mgr
        .handle_incoming("CA-test-001", "/voice/ws")
        .await
        .expect("handle_incoming ok");

    assert!(twiml.contains("<ConversationRelay"));
    assert!(twiml.contains("/voice/ws?token="));
    assert_eq!(mgr.token_count().await, 1);
}

#[tokio::test]
async fn freshly_issued_token_validates_ok() {
    let (mgr, _tmp) = manager().await;

    let twiml = mgr
        .handle_incoming("CA-test-002", "/voice/ws")
        .await
        .unwrap();
    let token = token_from_twiml(&twiml);

    let validated = mgr.validate_ws_token(&token).await.expect("token valid");
    assert_eq!(validated, "CA-test-002");
    assert_eq!(mgr.token_count().await, 0, "valid token is consumed");
}

#[tokio::test]
async fn freshly_issued_token_is_single_use() {
    let (mgr, _tmp) = manager().await;

    let twiml = mgr
        .handle_incoming("CA-test-003", "/voice/ws")
        .await
        .unwrap();
    let token = token_from_twiml(&twiml);

    mgr.validate_ws_token(&token)
        .await
        .expect("first use valid");
    let err = mgr
        .validate_ws_token(&token)
        .await
        .expect_err("second use rejected");
    assert!(err.to_string().contains("invalid voice ws token"));
}

#[tokio::test]
async fn pending_ws_tokens_evict_oldest_when_capacity_is_exceeded() {
    let (mgr, _tmp) = manager().await;
    let mut issued = Vec::with_capacity(MAX_PENDING_WS_TOKENS + 1);

    for idx in 0..=MAX_PENDING_WS_TOKENS {
        let twiml = mgr
            .handle_incoming(&format!("CA-cap-{idx}"), "/voice/ws")
            .await
            .unwrap();
        issued.push(token_from_twiml(&twiml));
    }

    assert_eq!(
        mgr.token_count().await,
        MAX_PENDING_WS_TOKENS,
        "pending token store must be capped"
    );
    let first_err = mgr
        .validate_ws_token(&issued[0])
        .await
        .expect_err("oldest token should be evicted");
    assert!(first_err.to_string().contains("invalid voice ws token"));

    let newest = issued.last().expect("newest token");
    assert_eq!(
        mgr.validate_ws_token(newest).await.expect("newest valid"),
        format!("CA-cap-{MAX_PENDING_WS_TOKENS}")
    );
}

#[tokio::test]
async fn unknown_token_rejected() {
    let (mgr, _tmp) = manager().await;

    let err = mgr
        .validate_ws_token("never-issued")
        .await
        .expect_err("unknown token rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("invalid voice ws token"),
        "expected invalid-token error, got: {msg}"
    );
}

#[tokio::test]
async fn expired_token_rejected_and_removed() {
    let (mgr, _tmp) = manager().await;
    mgr.inject_token_for_test("expired-token", 1).await;
    assert_eq!(mgr.token_count().await, 1);

    let err = mgr
        .validate_ws_token("expired-token")
        .await
        .expect_err("expired token rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("voice ws token expired"),
        "expected expired-token error, got: {msg}"
    );
    assert_eq!(
        mgr.token_count().await,
        0,
        "expired token should be removed from the map"
    );
}

#[tokio::test]
async fn second_validate_after_expiry_still_rejected() {
    let (mgr, _tmp) = manager().await;
    mgr.inject_token_for_test("expired-token", 1).await;

    let _ = mgr.validate_ws_token("expired-token").await;

    let err = mgr.validate_ws_token("expired-token").await.unwrap_err();
    assert!(err.to_string().contains("invalid voice ws token"));
}

#[tokio::test]
async fn terminal_status_cleans_session() {
    let (mgr, _tmp) = manager().await;
    mgr.create_session("CA-x", "+1", "+2").await.unwrap();
    assert_eq!(mgr.session_count().await, 1);

    mgr.handle_status_callback("CA-x", "completed")
        .await
        .unwrap();
    assert_eq!(mgr.session_count().await, 0);
}

#[tokio::test]
async fn non_terminal_status_keeps_session() {
    let (mgr, _tmp) = manager().await;
    mgr.create_session("CA-y", "+1", "+2").await.unwrap();

    mgr.handle_status_callback("CA-y", "ringing").await.unwrap();
    assert_eq!(
        mgr.session_count().await,
        1,
        "non-terminal status must not drop the session"
    );
}

#[tokio::test]
async fn status_callback_idempotent_for_unknown_session() {
    let (mgr, _tmp) = manager().await;

    mgr.handle_status_callback("CA-missing", "completed")
        .await
        .unwrap();
    assert_eq!(mgr.session_count().await, 0);
}

#[tokio::test]
async fn enabled_toggle_round_trip() {
    let (mgr, _tmp) = manager().await;
    assert!(mgr.is_enabled());
    mgr.set_enabled(false);
    assert!(!mgr.is_enabled());
    mgr.set_enabled(true);
    assert!(mgr.is_enabled());
}

async fn manager_with_config(config_json: &str) -> (VoiceManager, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    let data_dir = tmp.path().join("data");
    let secret_dir = tmp.path().join("secret");
    std::fs::create_dir_all(&data_dir).unwrap();
    rua_config::save_config(
        &rua_config::RootConfig::default(),
        &data_dir.join("config.json"),
    )
    .unwrap();
    std::fs::create_dir_all(&secret_dir).unwrap();
    std::fs::write(data_dir.join("config.json"), config_json).unwrap();
    let runtime: Arc<dyn rua_contract::agent::AgentRuntime> = Arc::new(stub_runtime::StubRuntime);
    let registry =
        voice_test_helpers::StubRegistry::with_runtime("default", tmp.path(), runtime).await;
    let mgr = VoiceManager::with_dirs(
        "test-secret".into(),
        data_dir,
        secret_dir,
        registry,
        "default".into(),
    )
    .expect("valid test configuration");
    (mgr, tmp)
}

#[tokio::test]
async fn twiml_contains_language_when_configured() {
    let config = r#"{"agents":{"tts_language":"zh-CN","speech_language":"zh-CN"}}"#;
    let (mgr, _tmp) = manager_with_config(config).await;

    let twiml = mgr
        .handle_incoming("CA-lang-1", "/voice/ws")
        .await
        .expect("ok");

    assert!(
        twiml.contains(r#"ttsLanguage="zh-CN""#),
        "expected ttsLanguage in TwiML, got: {twiml}"
    );
    assert!(
        twiml.contains(r#"speechLanguage="zh-CN""#),
        "expected speechLanguage in TwiML, got: {twiml}"
    );
}

#[tokio::test]
async fn twiml_omits_language_when_none() {
    let config = r#"{"agents":{}}"#;
    let (mgr, _tmp) = manager_with_config(config).await;

    let twiml = mgr
        .handle_incoming("CA-lang-2", "/voice/ws")
        .await
        .expect("ok");

    assert!(
        !twiml.contains("ttsLanguage"),
        "ttsLanguage should not appear when None, got: {twiml}"
    );
    assert!(
        !twiml.contains("speechLanguage"),
        "speechLanguage should not appear when None, got: {twiml}"
    );
}

#[tokio::test]
async fn twiml_partial_language_only_tts() {
    let config = r#"{"agents":{"tts_language":"en-US"}}"#;
    let (mgr, _tmp) = manager_with_config(config).await;

    let twiml = mgr
        .handle_incoming("CA-lang-3", "/voice/ws")
        .await
        .expect("ok");

    assert!(
        twiml.contains(r#"ttsLanguage="en-US""#),
        "expected ttsLanguage, got: {twiml}"
    );
    assert!(
        !twiml.contains("speechLanguage"),
        "speechLanguage should not appear, got: {twiml}"
    );
}

#[tokio::test]
async fn channel_settings_generate_public_twiml_and_keep_call_identity() {
    let (manager, _tmp) = manager().await;
    let settings = rua_config::channel_catalog::resolve_channel_config(
        "voice",
        &serde_json::json!({
            "enabled": true, "public_url": "https://calls.example.test",
            "tts_provider": "google", "tts_voice": "voice-custom",
            "stt_provider": "deepgram", "language": "zh-CN", "welcome_greeting": "你好 & welcome"
        }),
    )
    .unwrap();
    let twiml = manager
        .handle_incoming_with_config("CA-settings", &settings)
        .await
        .unwrap();
    assert!(twiml.contains("wss://calls.example.test/voice/ws?token="));
    assert!(twiml.contains("welcomeGreeting=\"你好 &amp; welcome\""));
    assert!(twiml.contains("voice=\"voice-custom\""));
    assert!(twiml.contains("language=\"zh-CN\""));
    assert_eq!(
        manager
            .validate_ws_token(&token_from_twiml(&twiml))
            .await
            .unwrap(),
        "CA-settings"
    );
}

#[tokio::test]
async fn channel_send_delivers_to_active_call_and_stop_closes_it() {
    use rua_channel::core::channel::Channel;
    use rua_channel::core::content::{ChannelTarget, OutgoingContentPart, OutgoingMessage};
    let (manager, _tmp) = manager().await;
    let manager = Arc::new(manager);
    manager
        .create_session("CA-outbound", "caller", "bot")
        .await
        .unwrap();
    let mut receiver = manager.attach_outbound("CA-outbound").await;
    let channel = rua_voice::VoiceChannel::new(manager.clone());
    channel
        .send(
            ChannelTarget {
                agent_id: "default".into(),
                conversation_id: "CA-outbound".into(),
                sender_id: None,
                reply_to: None,
                metadata: serde_json::json!({}),
            },
            OutgoingMessage {
                channel_id: "voice".into(),
                conversation_id: "CA-outbound".into(),
                receiver_id: None,
                content: vec![OutgoingContentPart::Text {
                    text: "hello caller".into(),
                }],
                reply_to: None,
                metadata: serde_json::json!({}),
            },
        )
        .await
        .unwrap();
    assert_eq!(receiver.recv().await.as_deref(), Some("hello caller"));
    channel.stop().await.unwrap();
    assert!(receiver.recv().await.is_none());
    assert_eq!(manager.session_count().await, 0);
    assert!(!manager.is_enabled());
}
