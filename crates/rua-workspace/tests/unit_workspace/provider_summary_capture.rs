use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use agent_types::{
    AgentEvent, ChatRequest, ChatResponse, FinishReason, ModelDialogueItem, Role, TokenUsage,
};
use async_trait::async_trait;
use futures::stream;
use llm_multiplex::traits::{
    ChatModel, ConnectionStatus, EventStream, ModelInfo, Provider, ProviderInfo,
};
use llm_multiplex::LlmError;
use rua_agent::context_manager::ContextSummaryRequest;
use rua_workspace::{PlatformProfile, RuaRoot, WorkspaceRuntime};

use super::test_helpers;

struct CapturingModel {
    requests: Arc<Mutex<Vec<ChatRequest>>>,
    info: ModelInfo,
}

#[async_trait]
impl ChatModel for CapturingModel {
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, LlmError> {
        self.requests.lock().unwrap().push(request);
        Ok(ChatResponse {
            message: ModelDialogueItem::text(Role::Assistant, "captured summary"),
            usage: Some(TokenUsage::new(10, 2)),
            model: Some(self.info.id.clone()),
            finish_reason: FinishReason::Stop,
            metadata: serde_json::Map::new(),
            extra: HashMap::new(),
        })
    }

    fn chat_stream(&self, _request: ChatRequest) -> EventStream {
        Box::pin(stream::empty::<Result<AgentEvent, LlmError>>())
    }

    fn model_info(&self) -> &ModelInfo {
        &self.info
    }
}

struct CapturingProvider {
    requests: Arc<Mutex<Vec<ChatRequest>>>,
    info: ProviderInfo,
}

#[async_trait]
impl Provider for CapturingProvider {
    fn info(&self) -> &ProviderInfo {
        &self.info
    }

    fn supports_model(&self, model: &str) -> bool {
        model == "capture-model"
    }

    fn create_chat_model(&self, _model: &str) -> Result<Box<dyn ChatModel>, LlmError> {
        Ok(Box::new(CapturingModel {
            requests: Arc::clone(&self.requests),
            info: ModelInfo {
                id: "capture-model".to_string(),
                name: "Capture Model".to_string(),
                context_window: Some(32_000),
                max_output_tokens: Some(1_024),
                supports_multimodal: None,
                supports_image: None,
                supports_video: None,
                probe_source: None,
                generate_kwargs: serde_json::Map::new(),
            },
        }))
    }

    async fn fetch_models(&self) -> Result<Vec<ModelInfo>, LlmError> {
        Ok(Vec::new())
    }

    async fn check_connection(&self) -> Result<ConnectionStatus, LlmError> {
        Ok(ConnectionStatus::Connected)
    }
}

#[tokio::test]
async fn provider_summary_is_captured_from_model_call_identity() {
    let tmp = tempfile::tempdir().unwrap();
    let working = tmp.path().join("working");
    let secret = tmp.path().join("secret");
    std::fs::create_dir_all(&working).unwrap();
    std::fs::create_dir_all(&secret).unwrap();
    let context = test_helpers::test_context(working.clone());
    let requests = Arc::new(Mutex::new(Vec::new()));
    context
        .provider_manager
        .register(Arc::new(CapturingProvider {
            requests: Arc::clone(&requests),
            info: ProviderInfo {
                id: "capture-provider".to_string(),
                name: "Capture Provider".to_string(),
                provider_type: Some("test".to_string()),
                is_local: true,
                support_model_discovery: false,
                support_connection_check: false,
            },
        }));
    let runtime = WorkspaceRuntime::new(
        "agent-1",
        "Agent 1",
        RuaRoot::from_paths(&working, &secret)
            .agent_workspace("default")
            .expect("valid Agent Workspace"),
        PlatformProfile::Desktop,
        context,
    );
    runtime.initialize_for_platform().await.unwrap();

    let capture_id = "capture-summary";
    let capture_path = tmp
        .path()
        .join("debug")
        .join("llm-captures")
        .join(capture_id);
    let address = rua_core::ChatAddress::agent("agent-1", "chat-1");
    llm_multiplex::debug_capture::register_capture(
        address.clone(),
        llm_multiplex::debug_capture::ActiveCapture {
            capture_id: capture_id.to_string(),
            capture_path: capture_path.clone(),
        },
    );
    let identity =
        rua_agent::model_call_context::ModelCallIdentityContext::from_chat_address(&address);
    let manager = runtime.context_manager().await.unwrap();
    let result = manager
        .summarize_context(ContextSummaryRequest {
            session_id: "chat-1".to_string(),
            agent_id: "agent-1".to_string(),
            messages: vec![agent_types::ModelDialogueItem::text(
                agent_types::Role::User,
                "Long conversation content",
            )],
            previous_summary: None,
            max_tokens: None,
            model: Some(rua_session::CheckpointModelIdentity {
                provider: "capture-provider".to_string(),
                model: "capture-model".to_string(),
            }),
            model_call_context: Some(identity.model_call()),
        })
        .await
        .unwrap();

    assert_eq!(result.summary, "captured summary");
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request.max_tokens, None);
    assert_eq!(
        request.metadata[rua_agent::hooks::SECURITY_CHAT_ADDRESS_METADATA_KEY],
        serde_json::to_value(&address).unwrap()
    );
    let call_id = request.metadata[llm_multiplex::debug_capture::MODEL_CALL_ID_METADATA_KEY]
        .as_str()
        .unwrap();
    let call_dir = capture_path.join("calls").join(call_id);
    llm_multiplex::debug_capture::flush_capture(&capture_path, std::time::Duration::from_secs(5))
        .unwrap();
    assert!(call_dir.join("logical-request.json").is_file());
    assert!(call_dir.join("call.json").is_file());
    llm_multiplex::debug_capture::unregister_capture(&address);
}
