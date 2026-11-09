use std::sync::Arc;

use async_trait::async_trait;
use futures::{stream, StreamExt};

use agent_types::{AgentEvent, ChatRequest, ChatResponse, FinishReason, ModelDialogueItem, Role};
use llm_multiplex::traits::{ChatModel, EventStream, ModelInfo, Provider, ProviderInfo};
use llm_multiplex::LlmError;
use rua_agent::memory_manager::{InMemoryMemoryManager, MemoryManager};
use rua_agent::runtime::ReactAgentRuntime;
use rua_contract::agent::{AgentInputMessage, AgentRequest, AgentRuntime};
use rua_core::message::ContentPart;
use rua_provider::manager::ProviderManager;

struct MockChatModel;

#[async_trait]
impl ChatModel for MockChatModel {
    async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, LlmError> {
        Ok(ChatResponse {
            message: ModelDialogueItem::text(Role::Assistant, "Hello"),
            usage: None,
            model: Some("mock-model".to_string()),
            finish_reason: FinishReason::Stop,
            metadata: Default::default(),
            extra: Default::default(),
        })
    }

    fn chat_stream(&self, _request: ChatRequest) -> EventStream {
        Box::pin(stream::iter(vec![
            Ok(AgentEvent::Start {
                run_id: "test-run".to_string(),
                model: Some("mock-model".to_string()),
                metadata: serde_json::Value::Null,
            }),
            Ok(AgentEvent::TextDelta {
                delta: "Hello".to_string(),
                metadata: serde_json::Value::Null,
            }),
            Ok(AgentEvent::End {
                finish_reason: FinishReason::Stop,
                usage: None,
                metadata: serde_json::Value::Null,
            }),
        ]))
    }

    fn model_info(&self) -> &ModelInfo {
        Box::leak(Box::new(ModelInfo {
            id: "mock-model".to_string(),
            name: "Mock Model".to_string(),
            context_window: Some(4096),
            max_output_tokens: Some(1024),
            supports_multimodal: None,
            supports_image: None,
            supports_video: None,
            probe_source: None,
            generate_kwargs: Default::default(),
        }))
    }
}

struct MockProvider {
    info: ProviderInfo,
}

impl MockProvider {
    fn new() -> Self {
        Self {
            info: ProviderInfo {
                id: "mock-provider".to_string(),
                name: "Mock Provider".to_string(),
                provider_type: Some("mock".to_string()),
                is_local: false,
                support_model_discovery: false,
                support_connection_check: false,
            },
        }
    }
}

#[async_trait]
impl Provider for MockProvider {
    fn info(&self) -> &ProviderInfo {
        &self.info
    }

    fn supports_model(&self, model: &str) -> bool {
        model == "mock-model"
    }

    fn create_chat_model(&self, _model: &str) -> Result<Box<dyn ChatModel>, LlmError> {
        Ok(Box::new(MockChatModel))
    }

    async fn fetch_models(&self) -> Result<Vec<ModelInfo>, LlmError> {
        Ok(vec![])
    }

    async fn check_connection(&self) -> Result<llm_multiplex::traits::ConnectionStatus, LlmError> {
        Ok(llm_multiplex::traits::ConnectionStatus::Connected)
    }
}

async fn make_provider_manager() -> Arc<ProviderManager> {
    let pm = ProviderManager::new();
    pm.register(Arc::new(MockProvider::new()));
    pm.store_default_model(rua_provider::config_loader::DefaultModel {
        provider_id: "mock-provider".to_string(),
        model: "mock-model".to_string(),
        source: None,
        extra: Default::default(),
    })
    .await;
    Arc::new(pm)
}

#[tokio::test]
async fn runtime_stream_with_memory_manager_completes() {
    let mm: Arc<dyn MemoryManager> = Arc::new(InMemoryMemoryManager::new());
    let pm = make_provider_manager().await;

    let runtime = ReactAgentRuntime::new(pm).with_memory_manager(Arc::clone(&mm));

    let req = AgentRequest {
        session_id: "test-session".into(),
        user_id: "test-user".to_string(),
        agent_id: "test-agent".into(),
        messages: vec![AgentInputMessage {
            role: "user".to_string(),
            content: vec![ContentPart::Text {
                text: "Hello".to_string(),
            }],
        }],
        model_override: None,
        tool_mode: "none".to_string(),
        attachments: vec![],
        display_preferences: None,
        disabled_tools: vec![],
        metadata: Default::default(),
        cancel: None,
        run_control: None,
        workspace_dir: None,
        conversation_store_dir: None,
        working_dir: None,
    };

    let event_stream = runtime.stream(req).await.expect("stream() should succeed");
    let events: Vec<_> = event_stream.collect().await;
    assert!(!events.is_empty(), "stream should produce events");
}

#[tokio::test]
async fn runtime_stream_without_memory_manager_completes() {
    let pm = make_provider_manager().await;

    let runtime = ReactAgentRuntime::new(pm);

    let req = AgentRequest {
        session_id: "test-session".into(),
        user_id: "test-user".to_string(),
        agent_id: "test-agent".into(),
        messages: vec![AgentInputMessage {
            role: "user".to_string(),
            content: vec![ContentPart::Text {
                text: "Hello".to_string(),
            }],
        }],
        model_override: None,
        tool_mode: "none".to_string(),
        attachments: vec![],
        display_preferences: None,
        disabled_tools: vec![],
        metadata: Default::default(),
        cancel: None,
        run_control: None,
        workspace_dir: None,
        conversation_store_dir: None,
        working_dir: None,
    };

    let event_stream = runtime.stream(req).await.expect("stream() should succeed");
    let events: Vec<_> = event_stream.collect().await;
    assert!(!events.is_empty(), "stream should produce events");
}

#[tokio::test(flavor = "current_thread")]
async fn runtime_memory_manager_supports_explicit_structured_writes_after_hook() {
    let mm = Arc::new(InMemoryMemoryManager::new());
    let pm = make_provider_manager().await;

    let runtime =
        ReactAgentRuntime::new(pm).with_memory_manager(Arc::clone(&mm) as Arc<dyn MemoryManager>);

    let tmp = tempfile::tempdir().unwrap();
    let req = AgentRequest {
        session_id: "session-mem-test".into(),
        user_id: "test-user".to_string(),
        agent_id: "agent-mem-test".into(),
        messages: vec![AgentInputMessage {
            role: "user".to_string(),
            content: vec![ContentPart::Text {
                text: "Tell me about Rust".to_string(),
            }],
        }],
        model_override: None,
        tool_mode: "none".to_string(),
        attachments: vec![],
        display_preferences: None,
        disabled_tools: vec![],
        metadata: Default::default(),
        cancel: None,
        run_control: None,
        workspace_dir: Some(tmp.path().to_path_buf()),
        conversation_store_dir: None,
        working_dir: None,
    };

    let event_stream = runtime.stream(req).await.expect("stream() should succeed");
    let _events: Vec<_> = event_stream.collect().await;

    mm.memory_write(rua_agent::memory_manager::MemoryWriteRequest {
        title: "Test memory".into(),
        agent_id: "agent-mem-test".into(),
        kind: rua_agent::memory_manager::MemoryKind::ProjectFact,
        scope: rua_agent::memory_manager::MemoryScope::Project,
        content: "Rust is a systems programming language".into(),
        confidence: Some(0.9),
        importance: Some(0.8),
        ttl_days: None,
        labels: vec![],
        search_terms: vec![],
        evidence: None,
        metadata: serde_json::Value::Null,
    })
    .await
    .expect("memory_write should work");

    let search_req = rua_agent::memory_manager::MemorySearchRequest {
        search_mode: rua_config::MemorySearchMode::default(),
        query: "Rust programming".into(),
        agent_id: "agent-mem-test".into(),
        top_k: 10,
    };
    let hits = mm.memory_search(search_req).await.unwrap();
    assert!(
        !hits.is_empty(),
        "structured Memory should be searchable via memory_manager"
    );
}
