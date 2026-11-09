use std::collections::HashMap;
use std::sync::Arc;

use react_agent::ReactAgent;
use rua_core::tool::ToolContext;
use rua_security::ApprovalAuthority;

fn make_echo_model() -> Arc<dyn llm_multiplex::ChatModel> {
    Arc::new(EchoModel {
        info: llm_multiplex::ModelInfo {
            id: "echo".into(),
            name: "Echo".into(),
            context_window: Some(4096),
            max_output_tokens: Some(1024),
            supports_multimodal: None,
            supports_image: None,
            supports_video: None,
            probe_source: None,
            generate_kwargs: serde_json::Map::new(),
        },
    })
}

struct EchoModel {
    info: llm_multiplex::ModelInfo,
}

#[async_trait::async_trait]
impl llm_multiplex::ChatModel for EchoModel {
    async fn chat(
        &self,
        _request: agent_types::request::ChatRequest,
    ) -> Result<agent_types::response::ChatResponse, llm_multiplex::LlmError> {
        use agent_types::model_dialogue_item::ModelDialogueItem;
        Ok(agent_types::response::ChatResponse {
            message: ModelDialogueItem::text(agent_types::Role::Assistant, "ok"),
            usage: None,
            model: Some("echo".into()),
            finish_reason: agent_types::FinishReason::Stop,
            metadata: serde_json::Map::new(),
            extra: HashMap::new(),
        })
    }

    fn chat_stream(
        &self,
        _request: agent_types::request::ChatRequest,
    ) -> std::pin::Pin<
        Box<
            dyn futures::Stream<Item = Result<agent_types::AgentEvent, llm_multiplex::LlmError>>
                + Send,
        >,
    > {
        Box::pin(futures::stream::empty())
    }

    fn model_info(&self) -> &llm_multiplex::ModelInfo {
        &self.info
    }
}

#[test]
fn builder_wires_approval_authority_from_security() {
    let authority = Arc::new(ApprovalAuthority::new());

    let agent = ReactAgent::builder(make_echo_model())
        .approval_authority(authority.clone())
        .build();

    assert!(
        agent.ctx.approval_authority.is_some(),
        "approval_authority must be Some after builder.approval_authority() call"
    );
    assert!(
        Arc::ptr_eq(agent.ctx.approval_authority.as_ref().unwrap(), &authority),
        "approval_authority must be the same Arc instance passed to builder"
    );
}

#[test]
fn builder_wires_agent_id_in_tool_ctx_metadata() {
    let agent_id = "test-agent-42";
    let mut metadata = HashMap::new();
    metadata.insert(
        "agent_id".to_string(),
        serde_json::Value::String(agent_id.to_string()),
    );

    let tool_ctx = ToolContext {
        agent_id: agent_id.to_string(),
        session_id: "sess-1".to_string(),
        working_dir: std::path::PathBuf::from("/tmp"),
        conversation_store_dir: None,
        tool_result_store_dir: None,
        metadata,
        cancel: None,
        task_runtime: None,
        task_session: None,
        collaboration_runtime: None,
        sync_transfer_runtime: None,
        event_tx: None,
        execution_output_max_tokens: 12_500,
        raw_capture_max_bytes: 1024 * 1024,
        recall_slice_max_tokens: 10_000,
        old_max_bytes: 3_000,
        token_estimate_divisor: 4.0,
    };

    let agent = ReactAgent::builder(make_echo_model())
        .tool_ctx(tool_ctx)
        .build();

    let got = agent.ctx.tool_ctx.metadata.get("agent_id");
    assert_eq!(
        got,
        Some(&serde_json::Value::String(agent_id.to_string())),
        "tool_ctx.metadata must contain agent_id matching the agent"
    );
}

#[test]
fn builder_wires_approval_authority_and_metadata_together() {
    let agent_id = "agent-xyz";
    let authority = Arc::new(ApprovalAuthority::new());

    let mut metadata = HashMap::new();
    metadata.insert(
        "agent_id".to_string(),
        serde_json::Value::String(agent_id.to_string()),
    );

    let tool_ctx = ToolContext {
        agent_id: agent_id.to_string(),
        session_id: "sess-2".to_string(),
        working_dir: std::path::PathBuf::from("/tmp"),
        conversation_store_dir: None,
        tool_result_store_dir: None,
        metadata,
        cancel: None,
        task_runtime: None,
        task_session: None,
        collaboration_runtime: None,
        sync_transfer_runtime: None,
        event_tx: None,
        execution_output_max_tokens: 12_500,
        raw_capture_max_bytes: 1024 * 1024,
        recall_slice_max_tokens: 10_000,
        old_max_bytes: 3_000,
        token_estimate_divisor: 4.0,
    };

    let agent = ReactAgent::builder(make_echo_model())
        .tool_ctx(tool_ctx)
        .approval_authority(authority.clone())
        .build();

    assert!(
        agent.ctx.approval_authority.is_some(),
        "approval_authority must be Some"
    );
    assert_eq!(
        agent.ctx.tool_ctx.metadata.get("agent_id"),
        Some(&serde_json::Value::String(agent_id.to_string())),
        "metadata[agent_id] must match"
    );
}
