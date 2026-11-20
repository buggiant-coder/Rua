use rua_core::RuaRoot;
use rua_session::external_chat::{route_for_chat, ExternalChatRoute};
use rua_session::WorkspaceSessionStore;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_chat_bindings_preserve_platform_account_and_native_thread_across_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let shared = RuaRoot::new(root).chat_workspace();
    let route = ExternalChatRoute {
        account_id: "subscribed-agent".into(),
        platform_account_id: Some("bot-1".into()),
        channel: "slack".into(),
        session_id: "room:thread".into(),
        conversation_id: "room".into(),
    };
    assert!(route.resolve(root, false).await.unwrap().is_none());
    let (first, duplicate) = tokio::join!(route.resolve(root, true), route.resolve(root, true));
    let binding = first.unwrap().unwrap();
    assert_eq!(binding.address, duplicate.unwrap().unwrap().address);
    assert_ne!(binding.address.chat_id().as_str(), route.session_id);
    let store = WorkspaceSessionStore::new(shared.as_path());
    assert!(store
        .load_chat_header(binding.address.chat_id().as_str())
        .await
        .unwrap()
        .is_none());
    assert!(!binding.admitted);
    binding.confirm(root).await.unwrap();
    let reopened = route.resolve(root, false).await.unwrap().unwrap();
    assert_eq!(binding.address, reopened.address);
    assert!(reopened.admitted);
    assert_eq!(
        route_for_chat(shared.as_path(), &binding.address)
            .await
            .unwrap(),
        Some(route.clone())
    );
    for other in [
        ExternalChatRoute {
            account_id: "second-subscription".into(),
            ..route.clone()
        },
        ExternalChatRoute {
            platform_account_id: Some("bot-2".into()),
            ..route.clone()
        },
        ExternalChatRoute {
            channel: "telegram".into(),
            ..route.clone()
        },
        ExternalChatRoute {
            session_id: "room:second-thread".into(),
            ..route.clone()
        },
    ] {
        let distinct = other.resolve(root, true).await.unwrap().unwrap();
        assert_ne!(distinct.address, binding.address);
        assert_ne!(
            distinct.platform_message_key("msg/1"),
            binding.platform_message_key("msg/1")
        );
    }
}

#[tokio::test]
async fn replicated_session_recovers_external_input_and_approval_or_rejects_conflicting_headers() {
    use futures::StreamExt;
    use rua_channel::core::{
        channel::Process,
        console::{ConsoleChatFacade, ConsoleChatRequest, ConsoleSubmitOutcome},
        content::{AgentChannelRequest, IncomingContentPart},
    };
    use rua_core::{ChatAddress, RuntimeEvent};
    use rua_session::{
        chat_deletion::ChatDeleteIntentStore, external_chat::EXTERNAL_CHANNEL_META_KEY,
        ChatMetadata, ChatSessionHeader,
    };
    use rua_workspace::channel_process_adapter::ChannelProcessAdapter;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct Gateway(Mutex<Vec<(ChatAddress, String, String)>>);
    #[async_trait::async_trait]
    impl ConsoleChatFacade for Gateway {
        async fn submit_scoped_chat(
            &self,
            address: ChatAddress,
            agent: &str,
            request: ConsoleChatRequest,
        ) -> ConsoleSubmitOutcome {
            self.0
                .lock()
                .unwrap()
                .push((address, agent.into(), request.message));
            ConsoleSubmitOutcome::local(false)
        }
        async fn decide_chat_approval(
            &self,
            address: &ChatAddress,
            agent: &str,
            _: Option<&str>,
            _: bool,
            _: &str,
        ) -> Result<bool, String> {
            self.0
                .lock()
                .unwrap()
                .push((address.clone(), agent.into(), "approval".into()));
            Ok(true)
        }
        async fn stop_chat(&self, _: &str, _: &str) -> bool {
            false
        }
    }
    let origin = tempfile::tempdir().unwrap();
    let receiver = tempfile::tempdir().unwrap();
    let route = ExternalChatRoute {
        account_id: "subscription".into(),
        platform_account_id: Some("bot-1".into()),
        channel: "feishu".into(),
        session_id: "thread".into(),
        conversation_id: "room".into(),
    };
    let binding = route.resolve(origin.path(), true).await.unwrap().unwrap();
    binding.confirm(origin.path()).await.unwrap();
    let chat_id = binding.address.chat_id().as_str();
    let mut header = ChatSessionHeader::new(ChatMetadata {
        chat_id: chat_id.into(),
        agent_id: "saved-agent".into(),
        title: "native thread".into(),
        created_at: "2026-01-01T00:00:00Z".into(),
        last_activity_at: "2026-01-01T00:00:00Z".into(),
        pinned: false,
        archived: false,
        preview: None,
    });
    header
        .extra
        .insert(EXTERNAL_CHANNEL_META_KEY.into(), serde_json::json!(route));
    let source = WorkspaceSessionStore::new(origin.path().join("chats"));
    source.save_chat_header(&header).await.unwrap();
    let saved = source.load_chat_header(chat_id).await.unwrap().unwrap();
    let receiver_store = WorkspaceSessionStore::new(receiver.path().join("chats"));
    receiver_store
        .merge_replicated_chat_header(chat_id, serde_json::json!(saved), |_, _| Ok(()))
        .await
        .unwrap();
    let gateway = Arc::new(Gateway::default());
    let facade: Arc<dyn ConsoleChatFacade> = gateway.clone();
    let adapter = ChannelProcessAdapter::new(
        receiver.path().to_path_buf(),
        "subscription".into(),
        Arc::downgrade(&facade),
    );
    let mut req = AgentChannelRequest {
        agent_id: "incoming-hint".into(),
        user_id: "actor".into(),
        channel_id: "feishu".into(),
        session_id: "thread".into(),
        conversation_id: "room".into(),
        metadata: serde_json::json!({"account_id":"bot-1"}),
        message_id: None,
        input: vec![IncomingContentPart::Text {
            text: "/approve".into(),
        }],
    };
    let response = adapter.handle_chat_control(&req).await.unwrap();
    assert!(
        matches!(response,RuntimeEvent::TextDelta{ref delta,..} if delta=="已批准"),
        "{response:?}"
    );
    req.input = vec![IncomingContentPart::Text {
        text: "next".into(),
    }];
    let _ = adapter.handle(req.clone()).await.collect::<Vec<_>>().await;
    assert_eq!(
        gateway.0.lock().unwrap().as_slice(),
        [
            (
                binding.address.clone(),
                "saved-agent".into(),
                "approval".into()
            ),
            (binding.address.clone(), "saved-agent".into(), "next".into())
        ]
    );
    assert!(
        route
            .resolve(receiver.path(), false)
            .await
            .unwrap()
            .unwrap()
            .admitted
    );
    let db = receiver
        .path()
        .join("chats/external-channel-bindings.sqlite");
    std::fs::remove_file(&db).unwrap();
    ChatDeleteIntentStore::from_rua_root(receiver.path())
        .persist(&binding.address)
        .unwrap();
    let events = adapter.handle(req.clone()).await.collect::<Vec<_>>().await;
    assert!(
        matches!(&events[..],[RuntimeEvent::Error{error}] if error.to_string().contains("deleted")),
        "{events:?}"
    );
    std::fs::remove_file(&db).unwrap();
    let conflicting_id = uuid::Uuid::new_v4().to_string();
    header.chat_id = conflicting_id.clone();
    header.metadata.chat_id = conflicting_id.clone();
    receiver_store
        .merge_replicated_chat_header(&conflicting_id, serde_json::json!(header), |_, _| Ok(()))
        .await
        .unwrap();
    let events = adapter.handle(req).await.collect::<Vec<_>>().await;
    assert!(
        matches!(&events[..],[RuntimeEvent::Error{error}] if error.to_string().contains("multiple Chats")),
        "{events:?}"
    );
    assert_eq!(gateway.0.lock().unwrap().len(), 2);
    assert!(!db.exists());
}
