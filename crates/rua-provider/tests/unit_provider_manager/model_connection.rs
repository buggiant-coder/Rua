use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};

use agent_types::{ChatRequest, ChatResponse, FinishReason, ModelDialogueItem, Role, TokenUsage};
use async_trait::async_trait;
use futures::stream;
use llm_multiplex::{
    ChatModel, ConnectionStatus, EventStream, LlmError, ModelInfo, Provider, ProviderInfo,
};
use rua_provider::ProviderManager;

struct RecordingProvider {
    create_calls: Arc<AtomicUsize>,
    created_models: Arc<Mutex<Vec<String>>>,
    chat_requests: Arc<Mutex<Vec<ProbeRequest>>>,
    fail_chat: bool,
    info: ProviderInfo,
}

#[derive(Debug, PartialEq, Eq)]
struct ProbeRequest {
    model: Option<String>,
    max_tokens: Option<u32>,
    text: String,
}

#[async_trait]
impl Provider for RecordingProvider {
    fn info(&self) -> &ProviderInfo {
        &self.info
    }

    fn supports_model(&self, _model: &str) -> bool {
        true
    }

    fn create_chat_model(&self, model: &str) -> Result<Box<dyn ChatModel>, LlmError> {
        self.create_calls.fetch_add(1, Ordering::SeqCst);
        self.created_models
            .lock()
            .expect("created models")
            .push(model.into());
        Ok(Box::new(OkChatModel {
            info: model_info(model),
            chat_requests: Arc::clone(&self.chat_requests),
            fail_chat: self.fail_chat,
        }))
    }

    async fn fetch_models(&self) -> Result<Vec<ModelInfo>, LlmError> {
        Ok(vec![])
    }

    async fn check_connection(&self) -> Result<ConnectionStatus, LlmError> {
        Ok(ConnectionStatus::Connected)
    }
}

struct OkChatModel {
    info: ModelInfo,
    chat_requests: Arc<Mutex<Vec<ProbeRequest>>>,
    fail_chat: bool,
}

#[async_trait]
impl ChatModel for OkChatModel {
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, LlmError> {
        let text = request
            .messages
            .first()
            .map(ModelDialogueItem::text_content)
            .unwrap_or_default();
        self.chat_requests
            .lock()
            .expect("chat requests")
            .push(ProbeRequest {
                model: request.model,
                max_tokens: request.max_tokens,
                text,
            });
        if self.fail_chat {
            return Err(LlmError::Unknown {
                provider: "provider".into(),
                message: "probe failed".into(),
            });
        }
        Ok(ChatResponse {
            message: ModelDialogueItem::text(Role::Assistant, "ok"),
            usage: Some(TokenUsage::new(1, 1)),
            model: Some(self.info.id.clone()),
            finish_reason: FinishReason::Stop,
            metadata: serde_json::Map::new(),
            extra: Default::default(),
        })
    }

    fn chat_stream(&self, _request: agent_types::ChatRequest) -> EventStream {
        Box::pin(stream::empty())
    }

    fn model_info(&self) -> &ModelInfo {
        &self.info
    }
}

fn model_info(id: &str) -> ModelInfo {
    ModelInfo {
        id: id.into(),
        name: id.into(),
        context_window: None,
        max_output_tokens: None,
        supports_multimodal: None,
        supports_image: None,
        supports_video: None,
        probe_source: None,
        generate_kwargs: serde_json::Map::new(),
    }
}

#[tokio::test]
async fn check_model_connection_rejects_blank_model_id_before_chat_probe() {
    let create_calls = Arc::new(AtomicUsize::new(0));
    let created_models = Arc::new(Mutex::new(Vec::new()));
    let chat_requests = Arc::new(Mutex::new(Vec::new()));
    let manager = ProviderManager::new();
    manager.register(Arc::new(RecordingProvider {
        create_calls: Arc::clone(&create_calls),
        created_models,
        chat_requests,
        fail_chat: false,
        info: ProviderInfo {
            id: "provider".into(),
            name: "Provider".into(),
            provider_type: Some("OpenAIChatModel".into()),
            is_local: false,
            support_model_discovery: true,
            support_connection_check: true,
        },
    }));

    let (ok, message) = manager
        .check_model_connection("provider", "   ")
        .await
        .expect("connection result");

    assert!(!ok);
    assert_eq!(message, "Empty model ID");
    assert_eq!(
        create_calls.load(Ordering::SeqCst),
        0,
        "blank model ids must not create a provider chat model or issue a probe"
    );
}

#[tokio::test]
async fn check_model_connection_uses_requested_model_and_minimal_probe() {
    let create_calls = Arc::new(AtomicUsize::new(0));
    let created_models = Arc::new(Mutex::new(Vec::new()));
    let chat_requests = Arc::new(Mutex::new(Vec::new()));
    let manager = ProviderManager::new();
    manager.register(Arc::new(RecordingProvider {
        create_calls: Arc::clone(&create_calls),
        created_models: Arc::clone(&created_models),
        chat_requests: Arc::clone(&chat_requests),
        fail_chat: false,
        info: ProviderInfo {
            id: "provider".into(),
            name: "Provider".into(),
            provider_type: Some("OpenAIChatModel".into()),
            is_local: false,
            support_model_discovery: true,
            support_connection_check: true,
        },
    }));

    let (ok, message) = manager
        .check_model_connection("provider", " target-model ")
        .await
        .expect("connection result");

    assert!(ok);
    assert_eq!(message, "connected");
    assert_eq!(create_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        created_models.lock().expect("created models").as_slice(),
        ["target-model"],
        "manager should trim and pass the requested model id to provider model creation"
    );
    assert_eq!(
        chat_requests.lock().expect("chat requests").as_slice(),
        [ProbeRequest {
            model: None,
            max_tokens: Some(1),
            text: "hi".into(),
        }],
        "Rust probes through the created model with a one-token lightweight chat request"
    );
}

#[tokio::test]
async fn check_model_connection_uses_responses_api_minimum_output_limit() {
    let create_calls = Arc::new(AtomicUsize::new(0));
    let created_models = Arc::new(Mutex::new(Vec::new()));
    let chat_requests = Arc::new(Mutex::new(Vec::new()));
    let manager = ProviderManager::new();
    manager.register(Arc::new(RecordingProvider {
        create_calls,
        created_models,
        chat_requests: Arc::clone(&chat_requests),
        fail_chat: false,
        info: ProviderInfo {
            id: "responses-provider".into(),
            name: "Responses Provider".into(),
            provider_type: Some("OpenAIResponseModel".into()),
            is_local: false,
            support_model_discovery: true,
            support_connection_check: true,
        },
    }));

    let (ok, message) = manager
        .check_model_connection("responses-provider", "gpt-5.6-sol")
        .await
        .expect("connection result");

    assert!(ok);
    assert_eq!(message, "connected");
    assert_eq!(
        chat_requests.lock().expect("chat requests").as_slice(),
        [ProbeRequest {
            model: None,
            max_tokens: Some(16),
            text: "hi".into(),
        }],
        "Responses API probes must respect the provider's minimum output token limit"
    );
}

#[tokio::test]
async fn check_model_connection_uses_idealab_chat_minimum_output_limit() {
    let create_calls = Arc::new(AtomicUsize::new(0));
    let created_models = Arc::new(Mutex::new(Vec::new()));
    let chat_requests = Arc::new(Mutex::new(Vec::new()));
    let manager = ProviderManager::new();
    manager.register(Arc::new(RecordingProvider {
        create_calls,
        created_models,
        chat_requests: Arc::clone(&chat_requests),
        fail_chat: false,
        info: ProviderInfo {
            id: "idealab".into(),
            name: "Idealab".into(),
            provider_type: Some("OpenAIChatModel".into()),
            is_local: false,
            support_model_discovery: false,
            support_connection_check: true,
        },
    }));

    let (ok, message) = manager
        .check_model_connection("idealab", "gpt-5.6-sol")
        .await
        .expect("connection result");

    assert!(ok);
    assert_eq!(message, "connected");
    assert_eq!(
        chat_requests.lock().expect("chat requests").as_slice(),
        [ProbeRequest {
            model: None,
            max_tokens: Some(16),
            text: "hi".into(),
        }],
        "Idealab chat probes must respect the proxy's minimum output token limit"
    );
}

#[tokio::test]
async fn check_model_connection_returns_false_for_probe_errors() {
    let create_calls = Arc::new(AtomicUsize::new(0));
    let created_models = Arc::new(Mutex::new(Vec::new()));
    let chat_requests = Arc::new(Mutex::new(Vec::new()));
    let manager = ProviderManager::new();
    manager.register(Arc::new(RecordingProvider {
        create_calls: Arc::clone(&create_calls),
        created_models: Arc::clone(&created_models),
        chat_requests: Arc::clone(&chat_requests),
        fail_chat: true,
        info: ProviderInfo {
            id: "provider".into(),
            name: "Provider".into(),
            provider_type: Some("OpenAIChatModel".into()),
            is_local: false,
            support_model_discovery: true,
            support_connection_check: true,
        },
    }));

    let (ok, message) = manager
        .check_model_connection("provider", "target-model")
        .await
        .expect("manager should convert probe errors into a failed result");

    assert!(!ok);
    assert_eq!(
        message,
        "Chat completion failed: unknown provider error: probe failed"
    );
    assert_eq!(create_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        created_models.lock().expect("created models").as_slice(),
        ["target-model"]
    );
    assert_eq!(chat_requests.lock().expect("chat requests").len(), 1);
}
