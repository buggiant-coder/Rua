use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::stream::{self, StreamExt};
use futures::Stream;

use rua_channel::core::channel::Process;
use rua_channel::core::content::{AgentChannelRequest, IncomingContentPart};
use rua_contract::agent::{AgentRequest, AgentRuntime, RuntimeWarmupContext};
use rua_core::event::RuntimeEvent;
use rua_core::id::SessionId;
use rua_core::{Result, RuaError};

pub(super) async fn mounted_adapter(
    root: &std::path::Path,
    agent: &str,
    runtime: Arc<dyn AgentRuntime>,
) -> (rua_server::AppState, Arc<dyn Process>) {
    let state =
        rua_server::AppState::from_tempdir(root.to_path_buf(), rua_server::ServerProfile::Test);
    state.wait_platform_initialization().await.unwrap();
    if agent != "default"
        && state
            .agent_service
            .get_agent(agent)
            .await
            .unwrap()
            .is_none()
    {
        state
            .agent_service
            .create_agent(
                serde_json::from_value(serde_json::json!({"id":agent,"name":agent})).unwrap(),
            )
            .await
            .unwrap();
    }
    let workspace = state.registry.get_or_create(agent).await.unwrap();
    workspace.set_react_runtime(runtime).await;
    let process = workspace.channel_chat_process().unwrap();
    (state, process)
}

pub(super) async fn collect_turn(
    process: &dyn Process,
    request: AgentChannelRequest,
) -> Vec<RuntimeEvent> {
    tokio::time::timeout(std::time::Duration::from_secs(20), async move {
        process.handle(request).await.collect().await
    })
    .await
    .expect("external turn must reach terminal")
}

struct MockRuntime {
    response_text: String,
}

#[async_trait]
impl AgentRuntime for MockRuntime {
    async fn stream(
        &self,
        req: AgentRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = std::result::Result<RuntimeEvent, RuaError>> + Send>>>
    {
        let text = self.response_text.clone();
        let events: Vec<std::result::Result<RuntimeEvent, RuaError>> = vec![
            Ok(RuntimeEvent::Start {
                session_id: req.session_id.clone(),
                agent_id: Some(req.agent_id.clone()),
            }),
            Ok(RuntimeEvent::TextDelta {
                delta: text,
                message_id: Some(format!("answer-{}", uuid::Uuid::new_v4()).into()),
            }),
            Ok(RuntimeEvent::End {
                finish_reason: Some("stop".into()),
            }),
        ];
        Ok(rua_agent::turn_persistence::persist_runtime_stream(
            &req,
            Box::pin(stream::iter(events)),
        ))
    }

    async fn cancel(&self, _session_id: &SessionId) -> Result<()> {
        Ok(())
    }
    async fn warmup(&self, _context: RuntimeWarmupContext) -> Result<()> {
        Ok(())
    }
    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}

fn make_text_request(text: &str) -> AgentChannelRequest {
    AgentChannelRequest {
        agent_id: "test-agent".into(),
        user_id: "user-1".to_string(),
        session_id: "session-1".to_string(),
        input: vec![IncomingContentPart::Text {
            text: text.to_string(),
        }],
        channel_id: "dingtalk".into(),
        conversation_id: "conv-1".into(),
        message_id: None,
        metadata: serde_json::Value::Null,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn adapter_forwards_text_to_runtime_and_streams_events() {
    let runtime = Arc::new(MockRuntime {
        response_text: "hello from agent".to_string(),
    });
    let root = tempfile::tempdir().unwrap();
    let (_state, adapter) = mounted_adapter(root.path(), "test-agent", runtime).await;

    let req = make_text_request("hi");
    let events: Vec<RuntimeEvent> = collect_turn(adapter.as_ref(), req).await;

    assert_eq!(events.len(), 3, "{events:?}");
    assert!(matches!(&events[0], RuntimeEvent::Start { .. }));
    assert!(matches!(
        &events[1],
        RuntimeEvent::TextDelta { delta, .. } if delta == "hello from agent"
    ));
    assert!(matches!(&events[2], RuntimeEvent::End { .. }));
}

struct CaptureRuntime {
    seen: Mutex<Option<AgentRequest>>,
}

#[async_trait]
impl AgentRuntime for CaptureRuntime {
    async fn stream(
        &self,
        req: AgentRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = std::result::Result<RuntimeEvent, RuaError>> + Send>>>
    {
        *self.seen.lock().expect("capture mutex poisoned") = Some(req);
        Ok(Box::pin(stream::iter(vec![Ok(RuntimeEvent::End {
            finish_reason: Some("stop".into()),
        })])))
    }

    async fn cancel(&self, _session_id: &SessionId) -> Result<()> {
        Ok(())
    }
    async fn warmup(&self, _context: RuntimeWarmupContext) -> Result<()> {
        Ok(())
    }
    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn adapter_preserves_audio_and_video_channel_inputs() {
    let runtime = Arc::new(CaptureRuntime {
        seen: Mutex::new(None),
    });
    let root = tempfile::tempdir().unwrap();
    let (_state, adapter) = mounted_adapter(root.path(), "test-agent", runtime.clone()).await;
    let req = AgentChannelRequest {
        agent_id: "test-agent".into(),
        user_id: "user-1".to_string(),
        session_id: "session-1".to_string(),
        input: vec![
            IncomingContentPart::Audio {
                file_id: "aud-1".into(),
                path: PathBuf::from("/tmp/voice.ogg"),
                mime: Some("audio/ogg".into()),
                name: Some("voice.ogg".into()),
            },
            IncomingContentPart::Video {
                file_id: "vid-1".into(),
                path: PathBuf::from("/tmp/clip.mp4"),
                mime: Some("video/mp4".into()),
                name: Some("clip.mp4".into()),
            },
        ],
        channel_id: "telegram".into(),
        conversation_id: "conv-1".into(),
        message_id: None,
        metadata: serde_json::Value::Null,
    };

    let events: Vec<RuntimeEvent> = collect_turn(adapter.as_ref(), req).await;
    assert!(
        events.iter().any(|e| matches!(e, RuntimeEvent::End { .. })),
        "{events:?}"
    );
    let captured = runtime
        .seen
        .lock()
        .expect("capture mutex poisoned")
        .take()
        .expect("runtime request captured");
    let content: Vec<_> = captured
        .messages
        .iter()
        .flat_map(|m| m.content.iter())
        .filter(|part| {
            matches!(
                part,
                agent_types::ContentPart::Audio { .. } | agent_types::ContentPart::Video { .. }
            )
        })
        .collect();

    assert!(matches!(
        &content[0],
        agent_types::ContentPart::Audio { url, media_type }
            if url == "file:///tmp/voice.ogg" && media_type.as_deref() == Some("audio/ogg")
    ));
    assert!(matches!(
        &content[1],
        agent_types::ContentPart::Video { url, media_type }
            if url == "file:///tmp/clip.mp4" && media_type.as_deref() == Some("video/mp4")
    ));
}

struct ErrorRuntime;

#[async_trait]
impl AgentRuntime for ErrorRuntime {
    async fn stream(
        &self,
        _req: AgentRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = std::result::Result<RuntimeEvent, RuaError>> + Send>>>
    {
        Err(RuaError::Runtime {
            message: "runtime exploded".into(),
        })
    }

    async fn cancel(&self, _session_id: &SessionId) -> Result<()> {
        Ok(())
    }
    async fn warmup(&self, _context: RuntimeWarmupContext) -> Result<()> {
        Ok(())
    }
    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn adapter_converts_runtime_error_to_error_event() {
    let root = tempfile::tempdir().unwrap();
    let (_state, adapter) =
        mounted_adapter(root.path(), "test-agent", Arc::new(ErrorRuntime)).await;

    let req = make_text_request("hi");
    let events: Vec<RuntimeEvent> = collect_turn(adapter.as_ref(), req).await;

    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, RuntimeEvent::Error { .. }))
            .count(),
        1,
        "{events:?}"
    );
    assert!(
        matches!(events.last(), Some(RuntimeEvent::Error{error}) if error.to_string().contains("runtime exploded")),
        "{events:?}"
    );
}

struct StreamErrorRuntime;

#[async_trait]
impl AgentRuntime for StreamErrorRuntime {
    async fn stream(
        &self,
        _req: AgentRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = std::result::Result<RuntimeEvent, RuaError>> + Send>>>
    {
        let events: Vec<std::result::Result<RuntimeEvent, RuaError>> = vec![
            Ok(RuntimeEvent::Start {
                session_id: "s1".into(),
                agent_id: None,
            }),
            Err(RuaError::Runtime {
                message: "mid-stream failure".into(),
            }),
        ];
        Ok(Box::pin(stream::iter(events)))
    }

    async fn cancel(&self, _session_id: &SessionId) -> Result<()> {
        Ok(())
    }
    async fn warmup(&self, _context: RuntimeWarmupContext) -> Result<()> {
        Ok(())
    }
    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn adapter_maps_stream_item_errors_to_error_events() {
    let root = tempfile::tempdir().unwrap();
    let (_state, adapter) =
        mounted_adapter(root.path(), "test-agent", Arc::new(StreamErrorRuntime)).await;

    let req = make_text_request("hi");
    let events: Vec<RuntimeEvent> = collect_turn(adapter.as_ref(), req).await;

    assert_eq!(events.len(), 2, "{events:?}");
    assert!(matches!(&events[0], RuntimeEvent::Start { .. }));
    assert!(matches!(&events[1], RuntimeEvent::Error { .. }));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_reply_and_reopened_chat_keep_distinct_platform_and_rua_identities() {
    use rua_channel::core::pipeline::{run_inbound, SendFn};
    use rua_channel::core::renderer::{MessageRenderer, RenderStyle};
    use rua_core::ChatAddress;
    use rua_session::external_chat::{ExternalChatRoute, EXTERNAL_CHANNEL_META_KEY};
    use rua_session::{chat_deletion::ChatDeleteIntentStore, WorkspaceSessionStore};

    let root = tempfile::tempdir().unwrap();
    let runtime = Arc::new(MockRuntime {
        response_text: "reply on original route".into(),
    });
    let (state, adapter) = mounted_adapter(root.path(), "subscribed", runtime.clone()).await;
    let mut req = make_text_request("first message");
    req.agent_id.clear();
    req.session_id = "native/thread-7".into();
    req.message_id = Some("native/message-1".into());
    req.metadata = serde_json::json!({"account_id":"bot-A","sender_id":"native-sender","user_name":"display","thread_ts":"7"});
    let sent = Arc::new(Mutex::new(vec![]));
    let captured = Arc::clone(&sent);
    let send: SendFn = Arc::new(move |target, msg| {
        let captured = Arc::clone(&captured);
        Box::pin(async move {
            captured.lock().unwrap().push((target, msg));
            Ok(())
        })
    });
    run_inbound(
        Arc::clone(&adapter),
        req.clone(),
        None,
        Arc::new(MessageRenderer::new(RenderStyle::PlainText)),
        send,
    )
    .await
    .unwrap();
    {
        let sent = sent.lock().unwrap();
        assert_eq!(sent.len(), 1, "{sent:?}");
        let (target, msg) = &sent[0];
        assert_eq!(target.conversation_id, req.conversation_id);
        assert_eq!(target.sender_id.as_ref(), Some(&req.user_id));
        assert_eq!(target.reply_to, req.message_id);
        assert_eq!(target.metadata["thread_ts"], "7");
        assert!(format!("{:?}", msg.content).contains("reply on original route"));
    }
    let id = adapter.control_chat_id(&req).await.unwrap().unwrap();
    uuid::Uuid::parse_str(&id).unwrap();
    let store = WorkspaceSessionStore::new(root.path().join("chats"));
    let header = store.load_chat_header(&id).await.unwrap().unwrap();
    assert_eq!(header.agent_id, "subscribed");
    assert_eq!(header.channel, "dingtalk");
    assert_eq!(header.user_id, "native-sender");
    assert_eq!(header.metadata.title, "dingtalk: display");
    let route: ExternalChatRoute =
        serde_json::from_value(header.extra[EXTERNAL_CHANNEL_META_KEY].clone()).unwrap();
    assert_eq!(route.account_id, "subscribed");
    assert_eq!(route.platform_account_id.as_deref(), Some("bot-A"));
    assert_eq!(route.session_id, req.session_id);
    assert_eq!(route.conversation_id, req.conversation_id);
    state.signal_shutdown();
    drop(adapter);
    drop(state);
    let (state, reopened) = mounted_adapter(root.path(), "subscribed", runtime).await;
    let replay = collect_turn(reopened.as_ref(), req.clone()).await;
    assert!(
        replay.is_empty(),
        "settled native delivery must not send another reply: {replay:?}"
    );
    req.agent_id = "different-suggestion".into();
    req.message_id = Some("native/message-2".into());
    req.input = vec![IncomingContentPart::Text {
        text: "second message".into(),
    }];
    let events: Vec<_> = collect_turn(reopened.as_ref(), req.clone()).await;
    assert!(
        matches!(events.last(), Some(RuntimeEvent::End { .. })),
        "{events:?}"
    );
    assert_eq!(
        reopened.control_chat_id(&req).await.unwrap(),
        Some(id.clone())
    );
    let history = store.read_turn_range(&id, 1, 10).await.unwrap();
    assert_eq!(history.len(), 2, "{history:?}");
    let mut other = req.clone();
    other.channel_id = "telegram".into();
    other.agent_id = "subscribed".into();
    let events: Vec<_> = collect_turn(reopened.as_ref(), other.clone()).await;
    assert!(
        matches!(events.last(), Some(RuntimeEvent::End { .. })),
        "{events:?}"
    );
    let other_id = reopened.control_chat_id(&other).await.unwrap().unwrap();
    assert_ne!(id, other_id);
    let authority = state.security.approval_authority();
    let approval = authority
        .create_request(rua_security::ApprovalRequestInput {
            agent_id: "subscribed".into(),
            owner: rua_security::ApprovalOwner::chat(ChatAddress::new(&id)),
            turn_id: history[1].turn_id.clone(),
            tool_call_id: "call".into(),
            tool_name: "demo".into(),
            trigger: rua_security::ApprovalTrigger::approval_mode("demo"),
            tool_call: rua_security::ToolCallSnapshot {
                display: "demo".into(),
                args: serde_json::json!({}),
            },
        })
        .await;
    other.input = vec![IncomingContentPart::Text {
        text: format!("/approval approve {}", approval.id),
    }];
    let _ = reopened.handle_chat_control(&other).await.unwrap();
    assert_eq!(
        authority
            .list_pending_for_address(&ChatAddress::new(&id))
            .await
            .len(),
        1
    );
    req.input = vec![IncomingContentPart::Text {
        text: "/approve".into(),
    }];
    let decision = reopened.handle_chat_control(&req).await.unwrap();
    assert!(
        matches!(decision,RuntimeEvent::TextDelta{ref delta,..} if delta == "已批准"),
        "{decision:?}"
    );
    assert_eq!(
        authority
            .request(&approval.id)
            .await
            .unwrap()
            .decided_by
            .as_deref(),
        Some("native-sender")
    );
    let address = ChatAddress::new(&id);
    ChatDeleteIntentStore::from_rua_root(root.path())
        .persist(&address)
        .unwrap();
    req.input = vec![IncomingContentPart::Text {
        text: "after deletion".into(),
    }];
    let events: Vec<_> = collect_turn(reopened.as_ref(), req).await;
    assert!(
        matches!(&events[..], [RuntimeEvent::Error{error}] if error.to_string().contains("deleted")),
        "{events:?}"
    );
    assert_eq!(store.read_turn_range(&id, 1, 10).await.unwrap().len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn explicit_agent_and_native_stop_use_same_canonical_active_chat() {
    use async_trait::async_trait;
    use rua_session::WorkspaceSessionStore;
    use tokio::sync::Notify;
    struct Running(Arc<Notify>);
    #[async_trait]
    impl AgentRuntime for Running {
        async fn stream(
            &self,
            req: AgentRequest,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<RuntimeEvent>> + Send>>> {
            assert_eq!(req.agent_id.as_str(), "chosen-agent");
            assert_eq!(req.user_id, "user-1");
            assert_eq!(req.metadata["channel"], "dingtalk");
            self.0.notify_one();
            Ok(Box::pin(stream::pending()))
        }
        async fn cancel(&self, _: &SessionId) -> Result<()> {
            Ok(())
        }
        async fn warmup(&self, _: RuntimeWarmupContext) -> Result<()> {
            Ok(())
        }
        async fn shutdown(&self) -> Result<()> {
            Ok(())
        }
    }
    let root = tempfile::tempdir().unwrap();
    let observed = Arc::new(Notify::new());
    let (state, _) = mounted_adapter(
        root.path(),
        "chosen-agent",
        Arc::new(Running(Arc::clone(&observed))),
    )
    .await;
    let default = state.registry.get_or_create("default").await.unwrap();
    let process = default.channel_chat_process().unwrap();
    let mut req = make_text_request("wait for stop");
    req.agent_id = "chosen-agent".into();
    let mut pending = process.handle(req.clone()).await;
    tokio::time::timeout(std::time::Duration::from_secs(15), observed.notified())
        .await
        .unwrap();
    let id = process.control_chat_id(&req).await.unwrap().unwrap();
    let store = WorkspaceSessionStore::new(root.path().join("chats"));
    assert_eq!(
        store.load_chat_header(&id).await.unwrap().unwrap().agent_id,
        "chosen-agent"
    );
    req.agent_id.clear();
    req.input = vec![IncomingContentPart::Text {
        text: "/stop".into(),
    }];
    let response = process.handle_chat_control(&req).await.unwrap();
    assert!(
        matches!(response, RuntimeEvent::TextDelta {ref delta,..} if delta == "已停止当前任务"),
        "{response:?}"
    );
    let events = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        pending.by_ref().collect::<Vec<_>>(),
    )
    .await
    .unwrap();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, RuntimeEvent::Error { .. })),
        "{events:?}"
    );
    let history = store.read_turn_range(&id, 1, 10).await.unwrap();
    assert_eq!(history.len(), 1);
    assert_ne!(history[0].lifecycle.status, "in_progress");
}
