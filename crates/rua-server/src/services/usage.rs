use std::pin::Pin;
use std::sync::Arc;

use crate::endpoints::dto;
use crate::services::console_chat::ConsoleChatService;

pub struct UsageRecordingChatService {
    inner: Arc<ConsoleChatService>,
}

impl UsageRecordingChatService {
    pub fn new(inner: Arc<ConsoleChatService>) -> Self {
        Self { inner }
    }
}

impl UsageRecordingChatService {
    pub fn chat_stream(
        &self,
        agent_id: &str,
        runtime: Arc<rua_workspace::WorkspaceRuntime>,
        req: dto::ChatRequest,
        cancel_rx: tokio::sync::watch::Receiver<rua_core::RunControl>,
    ) -> Pin<
        Box<
            dyn futures::Stream<Item = std::result::Result<dto::ChatEvent, String>>
                + Send
                + 'static,
        >,
    > {
        self.inner.chat_stream(agent_id, runtime, req, cancel_rx)
    }

    pub fn chat_stream_in_context(
        &self,
        execution: crate::services::console_chat::ConversationExecutionContext,
        runtime: Arc<rua_workspace::WorkspaceRuntime>,
        req: dto::ChatRequest,
        cancel_rx: tokio::sync::watch::Receiver<rua_core::RunControl>,
    ) -> Pin<
        Box<
            dyn futures::Stream<Item = std::result::Result<dto::ChatEvent, String>>
                + Send
                + 'static,
        >,
    > {
        self.inner
            .chat_stream_in_context(execution, runtime, req, cancel_rx)
    }
}
