#[path = "../voice_test_helpers.rs"]
mod voice_test_helpers;

use async_trait::async_trait;
use futures::stream;
use rua_contract::agent::{AgentRequest, AgentRuntime, RuntimeWarmupContext};
use rua_core::event::RuntimeEvent;
use rua_core::id::SessionId;
use rua_core::message::ContentPart;
use rua_core::{Result, RuaError};
use rua_voice::VoiceManager;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use tempfile::TempDir;

struct CapturingRuntime {
    captured_prompts: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl AgentRuntime for CapturingRuntime {
    async fn stream(
        &self,
        req: AgentRequest,
    ) -> Result<
        Pin<Box<dyn futures::Stream<Item = std::result::Result<RuntimeEvent, RuaError>> + Send>>,
    > {
        for msg in &req.messages {
            for part in &msg.content {
                if let ContentPart::Text { text } = part {
                    self.captured_prompts.lock().unwrap().push(text.clone());
                }
            }
        }

        let events = vec![
            Ok(RuntimeEvent::Start {
                session_id: "test".into(),
                agent_id: None,
            }),
            Ok(RuntimeEvent::TextDelta {
                delta: "ack".into(),
                message_id: None,
            }),
            Ok(RuntimeEvent::End {
                finish_reason: Some("stop".into()),
            }),
        ];
        Ok(Box::pin(stream::iter(events)))
    }

    async fn cancel(&self, _session_id: &SessionId) -> Result<()> {
        Ok(())
    }
    async fn warmup(&self, _ctx: RuntimeWarmupContext) -> Result<()> {
        Ok(())
    }
    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}

async fn make_manager(runtime: Arc<dyn AgentRuntime>) -> (VoiceManager, TempDir) {
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
async fn dtmf_digit_forwarded_as_user_message() {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let runtime: Arc<dyn AgentRuntime> = Arc::new(CapturingRuntime {
        captured_prompts: captured.clone(),
    });
    let (mgr, _tmp) = make_manager(runtime).await;
    mgr.create_session("CA-dtmf-1", "+1", "+2").await.unwrap();

    let reply = mgr.handle_dtmf("CA-dtmf-1", "5").await.unwrap();
    assert_eq!(reply, "ack");

    let prompts = captured.lock().unwrap();
    assert_eq!(prompts.len(), 1);
    assert_eq!(prompts[0], "[DTMF] 5");
}

#[tokio::test]
async fn dtmf_star_digit() {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let runtime: Arc<dyn AgentRuntime> = Arc::new(CapturingRuntime {
        captured_prompts: captured.clone(),
    });
    let (mgr, _tmp) = make_manager(runtime).await;
    mgr.create_session("CA-dtmf-2", "+1", "+2").await.unwrap();

    let reply = mgr.handle_dtmf("CA-dtmf-2", "*").await.unwrap();
    assert_eq!(reply, "ack");

    let prompts = captured.lock().unwrap();
    assert_eq!(prompts[0], "[DTMF] *");
}

#[tokio::test]
async fn dtmf_missing_session_returns_not_found() {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let runtime: Arc<dyn AgentRuntime> = Arc::new(CapturingRuntime {
        captured_prompts: captured.clone(),
    });
    let (mgr, _tmp) = make_manager(runtime).await;

    let err = mgr
        .handle_dtmf("CA-missing", "5")
        .await
        .expect_err("missing session");
    assert!(err.to_string().contains("voice_session"));
}
