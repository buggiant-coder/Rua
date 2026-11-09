use async_trait::async_trait;
use rua_channel::core::{
    AgentRequest, Channel, OutMessage, OutgoingContentPart, Process, ToHandle,
};
use rua_core::{RuaError, RuntimeEvent};
use rua_cron::{CronAgentRunner, CronChannelSender, CronExecutor, RuntimeEventStream};
use rua_server::endpoints::messages::PushMessageStore;
use rua_server::{AppState, ServerProfile, WorkspaceCronChannelSender};
use std::sync::{Arc, Mutex};

struct Runner;
#[async_trait]
impl CronAgentRunner for Runner {
    async fn run_query(&self, _: &str, _: &str, _: &str, _: &str) -> Result<String, RuaError> {
        unreachable!()
    }
    async fn stream_query(
        &self,
        _: &str,
        _: &str,
        _: &str,
        _: &str,
    ) -> Result<RuntimeEventStream, RuaError> {
        Ok(Box::pin(futures::stream::iter(vec![
            Ok(RuntimeEvent::TextDelta {
                delta: "hello ".into(),
                message_id: None,
            }),
            Ok(RuntimeEvent::TextDelta {
                delta: "world".into(),
                message_id: None,
            }),
            Ok(RuntimeEvent::End {
                finish_reason: Some("stop".into()),
            }),
        ])))
    }
}

struct Sink {
    id: String,
    messages: Mutex<Vec<OutMessage>>,
    fail: bool,
}
#[async_trait]
impl Channel for Sink {
    fn id(&self) -> &String {
        &self.id
    }
    async fn start(&self, _: Arc<dyn Process>) -> rua_core::Result<()> {
        Ok(())
    }
    async fn stop(&self) -> rua_core::Result<()> {
        Ok(())
    }
    async fn send(&self, target: ToHandle, message: OutMessage) -> rua_core::Result<()> {
        assert_eq!(target.agent_id, "default");
        assert_eq!(target.conversation_id, "cron-target");
        assert_eq!(message.receiver_id.as_deref(), Some("recipient"));
        if self.fail {
            return Err(RuaError::Runtime {
                message: "delivery refused".into(),
            });
        }
        self.messages.lock().unwrap().push(message);
        Ok(())
    }
    fn build_request_from_native(&self, _: serde_json::Value) -> rua_core::Result<AgentRequest> {
        unreachable!()
    }
}

fn job(channel: &str, mode: &str) -> rua_config::types::cron::CronJobSpec {
    serde_json::from_value(serde_json::json!({
        "id": "delivery-test", "name": "Delivery test", "task_type": "agent", "prompt": "hello",
        "agent_id": "default", "save_result_to_inbox": true,
        "dispatch": {"channel": channel, "mode": mode, "target": {"user_id": "recipient", "session_id": "cron-target"}}
    })).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn production_sender_delivers_stream_final_and_propagates_send_failures() {
    let tmp = tempfile::tempdir().unwrap();
    let state = AppState::from_tempdir(tmp.path().to_path_buf(), ServerProfile::Test);
    let manager = state.channel_manager_for("default").await.unwrap();
    let sink = Arc::new(Sink {
        id: "test-delivery".into(),
        messages: Mutex::new(Vec::new()),
        fail: false,
    });
    manager.register(sink.clone()).await.unwrap();
    let store = PushMessageStore::new(None);
    let sender = Arc::new(
        WorkspaceCronChannelSender::new(state.registry.clone()).with_push_store(store.clone()),
    );
    let executor = CronExecutor::new(sender.clone(), Arc::new(Runner));
    assert!(
        executor
            .execute(&job("test-delivery", "stream"))
            .await
            .success
    );
    {
        let messages = sink.messages.lock().unwrap();
        assert_eq!(messages.len(), 2);
        assert!(
            matches!(&messages[0].content[0], OutgoingContentPart::Text { text } if text == "hello ")
        );
        assert!(
            matches!(&messages[1].content[0], OutgoingContentPart::Text { text } if text == "world")
        );
    }
    assert_eq!(store.list().await.len(), 1);
    sink.messages.lock().unwrap().clear();
    assert!(
        executor
            .execute(&job("test-delivery", "final"))
            .await
            .success
    );
    {
        let messages = sink.messages.lock().unwrap();
        assert_eq!(messages.len(), 1);
        assert!(
            matches!(&messages[0].content[0], OutgoingContentPart::Text { text } if text == "hello world")
        );
    }
    let failing = Arc::new(Sink {
        id: "failed-delivery".into(),
        messages: Mutex::new(Vec::new()),
        fail: true,
    });
    manager.register(failing).await.unwrap();
    let before = store.list().await.len();
    for mode in ["stream", "final"] {
        let result = executor.execute(&job("failed-delivery", mode)).await;
        assert!(!result.success);
        assert!(result.error.unwrap().contains("delivery refused"));
    }
    let approval = RuntimeEvent::ToolApprovalPending {
        call_id: "call".into(),
        request_id: Some("request".into()),
        tool_name: "tool".into(),
        prompt: None,
    };
    assert!(sender
        .send_event(
            "default",
            "failed-delivery",
            "recipient",
            "cron-target",
            &approval
        )
        .await
        .is_err());
    assert_eq!(store.list().await.len(), before);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn console_delivery_is_visible_once_without_a_legacy_channel() {
    let tmp = tempfile::tempdir().unwrap();
    let state = AppState::from_tempdir(tmp.path().to_path_buf(), ServerProfile::Test);
    let store = state.push_message_store.clone();
    let sender = Arc::new(
        WorkspaceCronChannelSender::new(state.registry.clone()).with_push_store(store.clone()),
    );
    let executor = CronExecutor::new(sender, Arc::new(Runner));
    assert!(executor.execute(&job("console", "final")).await.success);
    let messages = store.list().await;
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].content, "hello world");
    assert_eq!(messages[0].extra["agent_id"], "default");
    assert!(executor.execute(&job("console", "stream")).await.success);
    assert_eq!(store.list().await.len(), 3);
    let missing_store = WorkspaceCronChannelSender::new(state.registry.clone());
    assert!(missing_store
        .send_text("default", "console", "recipient", "cron-target", "text")
        .await
        .is_err());
}
