#[path = "../voice_test_helpers.rs"]
mod voice_test_helpers;

use async_trait::async_trait;
use futures::stream;
use rua_contract::agent::{AgentRequest, AgentRuntime, RuntimeWarmupContext};
use rua_core::event::RuntimeEvent;
use rua_core::{Result, RuaError};
use rua_voice::VoiceManager;
use std::pin::Pin;
use std::sync::Arc;
use tempfile::TempDir;

struct MockRuntime {
    response_text: String,
}

#[async_trait]
impl AgentRuntime for MockRuntime {
    async fn stream(
        &self,
        _req: AgentRequest,
    ) -> Result<
        Pin<Box<dyn futures::Stream<Item = std::result::Result<RuntimeEvent, RuaError>> + Send>>,
    > {
        let text = self.response_text.clone();
        let events = vec![
            Ok(RuntimeEvent::Start {
                session_id: "test".into(),
                agent_id: None,
            }),
            Ok(RuntimeEvent::TextDelta {
                delta: text,
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

struct ErrorRuntime;

#[async_trait]
impl AgentRuntime for ErrorRuntime {
    async fn stream(
        &self,
        _req: AgentRequest,
    ) -> Result<
        Pin<Box<dyn futures::Stream<Item = std::result::Result<RuntimeEvent, RuaError>> + Send>>,
    > {
        Err(RuaError::Provider {
            provider: "test".into(),
            retryable: false,
            message: "mock LLM error".into(),
        })
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
async fn process_prompt_collects_text_deltas() {
    let runtime: Arc<dyn AgentRuntime> = Arc::new(MockRuntime {
        response_text: "你好".into(),
    });
    let (mgr, _tmp) = make_manager(runtime).await;
    mgr.create_session("CA-1", "+1", "+2").await.unwrap();

    let reply = mgr.process_prompt("CA-1", "hello").await.unwrap();
    assert_eq!(reply, "你好");
}

#[tokio::test]
async fn process_prompt_error_returns_fallback() {
    let runtime: Arc<dyn AgentRuntime> = Arc::new(ErrorRuntime);
    let (mgr, _tmp) = make_manager(runtime).await;
    mgr.create_session("CA-2", "+1", "+2").await.unwrap();

    let reply = mgr.process_prompt("CA-2", "hello").await.unwrap();
    assert_eq!(reply, "抱歉，处理出错了");
}

#[tokio::test]
async fn process_prompt_missing_session_returns_not_found() {
    let runtime: Arc<dyn AgentRuntime> = Arc::new(MockRuntime {
        response_text: "hi".into(),
    });
    let (mgr, _tmp) = make_manager(runtime).await;

    let err = mgr
        .process_prompt("CA-missing", "hello")
        .await
        .expect_err("missing session");
    assert!(err.to_string().contains("voice_session"));
}
