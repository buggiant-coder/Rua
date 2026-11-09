#[path = "../voice_test_helpers.rs"]
mod voice_test_helpers;

use async_trait::async_trait;
use rua_contract::agent::{AgentRequest, AgentRuntime, RuntimeWarmupContext};
use rua_core::event::RuntimeEvent;
use rua_core::id::SessionId;
use rua_core::{Result, RuaError};
use rua_voice::VoiceManager;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tempfile::TempDir;

struct SlowRuntime {
    cancelled: Arc<AtomicBool>,
}

#[async_trait]
impl AgentRuntime for SlowRuntime {
    async fn stream(
        &self,
        req: AgentRequest,
    ) -> Result<
        Pin<Box<dyn futures::Stream<Item = std::result::Result<RuntimeEvent, RuaError>> + Send>>,
    > {
        let cancel_rx = req.cancel.clone();
        let events = vec![
            Ok(RuntimeEvent::Start {
                session_id: "test".into(),
                agent_id: None,
            }),
            Ok(RuntimeEvent::TextDelta {
                delta: "partial ".into(),
                message_id: None,
            }),
            Ok(RuntimeEvent::TextDelta {
                delta: "response".into(),
                message_id: None,
            }),
        ];

        let stream = async_stream::stream! {
            for event in events {
                if let Some(ref rx) = cancel_rx {
                    if *rx.borrow() {
                        break;
                    }
                }
                tokio::task::yield_now().await;
                yield event;
            }
        };
        Ok(Box::pin(stream))
    }

    async fn cancel(&self, _session_id: &SessionId) -> Result<()> {
        self.cancelled.store(true, Ordering::SeqCst);
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
async fn cancel_via_watch_channel_stops_stream() {
    let cancelled = Arc::new(AtomicBool::new(false));
    let runtime: Arc<dyn AgentRuntime> = Arc::new(SlowRuntime {
        cancelled: cancelled.clone(),
    });
    let (mgr, _tmp) = make_manager(runtime).await;
    mgr.create_session("CA-int-1", "+1", "+2").await.unwrap();

    let (tx, rx) = tokio::sync::watch::channel(false);

    tx.send(true).unwrap();

    let reply = mgr
        .process_prompt_with_cancel("CA-int-1", "hello", Some(rx))
        .await
        .unwrap();

    assert_eq!(reply, "抱歉，处理出错了");
}

#[tokio::test]
async fn handle_interrupt_calls_runtime_cancel() {
    let cancelled = Arc::new(AtomicBool::new(false));
    let runtime: Arc<dyn AgentRuntime> = Arc::new(SlowRuntime {
        cancelled: cancelled.clone(),
    });
    let (mgr, _tmp) = make_manager(runtime).await;
    mgr.create_session("CA-int-2", "+1", "+2").await.unwrap();

    mgr.handle_interrupt("CA-int-2").await;
    assert!(
        cancelled.load(Ordering::SeqCst),
        "runtime.cancel should have been called"
    );
}

#[tokio::test]
async fn uncancelled_stream_returns_full_response() {
    let cancelled = Arc::new(AtomicBool::new(false));
    let runtime: Arc<dyn AgentRuntime> = Arc::new(SlowRuntime {
        cancelled: cancelled.clone(),
    });
    let (mgr, _tmp) = make_manager(runtime).await;
    mgr.create_session("CA-int-3", "+1", "+2").await.unwrap();

    let reply = mgr
        .process_prompt_with_cancel("CA-int-3", "hello", None)
        .await
        .unwrap();

    assert_eq!(reply, "partial response");
}
