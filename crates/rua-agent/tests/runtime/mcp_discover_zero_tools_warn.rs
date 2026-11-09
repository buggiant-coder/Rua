use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::{stream, StreamExt};
use tracing_subscriber::fmt::MakeWriter;

use agent_types::{AgentEvent, ChatRequest, ChatResponse, FinishReason, ModelDialogueItem, Role};
use llm_multiplex::traits::{ChatModel, EventStream, ModelInfo, Provider, ProviderInfo};
use llm_multiplex::LlmError;
use mcp_manager::error::McpResult;
use mcp_manager::manager::McpManager;
use mcp_manager::status::{McpConnectionState, McpServerStatus};
use mcp_manager::types::{
    McpCallContext, McpCallResult, McpConfig, McpReloadReport, McpServerConfig, McpServerId,
    McpShutdownReport, McpToolInfo, ToolFilter,
};
use rua_agent::runtime::ReactAgentRuntime;
use rua_contract::agent::{AgentInputMessage, AgentRequest, AgentRuntime};
use rua_core::message::ContentPart;
use rua_provider::manager::ProviderManager;

struct EmptyToolsMcpManager;

#[async_trait]
impl McpManager for EmptyToolsMcpManager {
    async fn suspend_new_calls(&self) -> mcp_manager::error::McpResult<()> {
        Ok(())
    }
    async fn configuration(&self) -> mcp_manager::error::McpResult<McpConfig> {
        Ok(McpConfig::default())
    }

    async fn load_config(&self, _config: McpConfig) -> McpResult<McpReloadReport> {
        Ok(McpReloadReport::default())
    }

    async fn list_servers(&self) -> McpResult<Vec<McpServerStatus>> {
        Ok(vec![McpServerStatus {
            key: "test-server".to_string(),
            name: "Test Server".to_string(),
            enabled: true,
            state: McpConnectionState::Ready,
            transport_type: "stdio".to_string(),
            tools_count: 0,
            last_error: None,
            connected_at: None,
            mobile_unavailable_reason: None,
            config_errors: vec![],
        }])
    }

    async fn list_tools(&self, _filter: ToolFilter) -> McpResult<Vec<McpToolInfo>> {
        Ok(vec![])
    }

    async fn call_tool(
        &self,
        _server: &McpServerId,
        _tool: &str,
        _args: serde_json::Value,
        _ctx: McpCallContext,
    ) -> McpResult<McpCallResult> {
        unreachable!()
    }

    async fn reload(&self, _new_config: McpConfig) -> McpResult<McpReloadReport> {
        Ok(McpReloadReport::default())
    }

    async fn invalidate_tool_cache(&self, _server: &McpServerId) -> McpResult<()> {
        Ok(())
    }

    async fn connect_server(&self, _server: &McpServerId) -> McpResult<()> {
        Ok(())
    }

    async fn disconnect_server(&self, _server: &McpServerId) -> McpResult<()> {
        Ok(())
    }

    async fn replace_client(
        &self,
        _server_id: &McpServerId,
        _config: McpServerConfig,
    ) -> McpResult<()> {
        Ok(())
    }

    async fn shutdown_all(&self) -> McpResult<McpShutdownReport> {
        Ok(McpShutdownReport::default())
    }
}

struct MockChatModel;

#[async_trait]
impl ChatModel for MockChatModel {
    async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, LlmError> {
        Ok(ChatResponse {
            message: ModelDialogueItem::text(Role::Assistant, "ok"),
            usage: None,
            model: Some("mock-model".to_string()),
            finish_reason: FinishReason::Stop,
            metadata: Default::default(),
            extra: Default::default(),
        })
    }

    fn chat_stream(&self, _request: ChatRequest) -> EventStream {
        let events = vec![
            Ok(AgentEvent::Start {
                run_id: "test-run".to_string(),
                model: Some("mock-model".to_string()),
                metadata: serde_json::Value::Null,
            }),
            Ok(AgentEvent::TextDelta {
                delta: "ok".to_string(),
                metadata: serde_json::Value::Null,
            }),
            Ok(AgentEvent::End {
                finish_reason: FinishReason::Stop,
                usage: None,
                metadata: serde_json::Value::Null,
            }),
        ];
        Box::pin(stream::iter(events))
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

#[derive(Clone, Default)]
struct LogCapture(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for LogCapture {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for LogCapture {
    type Writer = LogCapture;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

#[tokio::test]
async fn mcp_discover_zero_tools_emits_warn() {
    let capture = LogCapture::default();
    let capture_clone = capture.clone();

    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .with_writer(capture_clone)
        .with_ansi(false)
        .finish();

    let pm = ProviderManager::new();
    pm.register(Arc::new(MockProvider::new()));
    pm.store_default_model(rua_provider::config_loader::DefaultModel {
        provider_id: "mock-provider".to_string(),
        model: "mock-model".to_string(),
        source: None,
        extra: Default::default(),
    })
    .await;

    let runtime =
        ReactAgentRuntime::new(Arc::new(pm)).with_mcp_manager(Arc::new(EmptyToolsMcpManager));

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

    {
        let _guard = tracing::subscriber::set_default(subscriber);
        let stream = runtime.stream(req).await.expect("stream() should succeed");
        let _: Vec<_> = stream.collect().await;
        drop(_guard);
    };

    let log_output = String::from_utf8_lossy(&capture.0.lock().unwrap()).to_string();
    assert!(
        log_output.contains("MCP discovered 0 tools"),
        "expected warn log about 0 tools, got: {log_output}"
    );
    assert!(
        log_output.contains("test-server"),
        "expected server name in log, got: {log_output}"
    );
}
