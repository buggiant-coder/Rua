use std::pin::Pin;
use std::sync::Arc;

use agent_client_protocol::schema::v1::InitializeRequest;
use agent_client_protocol::schema::ProtocolVersion;
use async_trait::async_trait;
use futures::{stream, Stream};
use rua_acp::server::{v1, AcpSessionSetup, RuaAcpServer};
use rua_contract::agent::{AgentRequest, AgentRuntime, RuntimeWarmupContext};
use rua_core::event::RuntimeEvent;
use rua_core::id::SessionId;
use rua_core::{Result, RuaError};

struct NoopRuntime;

#[async_trait]
impl AgentRuntime for NoopRuntime {
    async fn stream(
        &self,
        _req: AgentRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = std::result::Result<RuntimeEvent, RuaError>> + Send>>>
    {
        Ok(Box::pin(stream::empty()))
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

fn server(agent_id: &str) -> RuaAcpServer {
    RuaAcpServer::new_with_agent_id(Arc::new(NoopRuntime), agent_id)
}

#[tokio::test]
async fn initialize_advertises_only_implemented_v1_capabilities() {
    let response = v1::initialize_response(InitializeRequest::new(ProtocolVersion::V1));

    assert_eq!(response.protocol_version, ProtocolVersion::V1);
    assert!(!response.agent_capabilities.load_session);
    assert!(response
        .agent_capabilities
        .session_capabilities
        .list
        .is_some());
    assert!(response
        .agent_capabilities
        .session_capabilities
        .resume
        .is_some());
    assert!(response
        .agent_capabilities
        .session_capabilities
        .close
        .is_some());
    assert!(!response.agent_capabilities.prompt_capabilities.image);
    assert!(!response.agent_capabilities.prompt_capabilities.audio);
    assert!(
        !response
            .agent_capabilities
            .prompt_capabilities
            .embedded_context
    );
    let agent_info = response.agent_info.expect("agent info");
    assert_eq!(agent_info.name, "rua");
    assert_eq!(agent_info.title.as_deref(), Some("Rua Agent"));
}

#[tokio::test]
async fn initialize_falls_back_to_latest_supported_protocol_version() {
    let response = v1::initialize_response(InitializeRequest::new(2_u16.into()));

    assert_eq!(response.protocol_version, ProtocolVersion::V1);
}

#[test]
fn shared_server_core_is_protocol_neutral() {
    let source = include_str!("../../src/server.rs");

    assert!(!source.contains("agent_client_protocol::schema::v1"));
    assert!(!source.contains("agent_client_protocol::schema::v2"));
    assert!(source.contains("pub enum AcpProtocolVersion"));
    assert!(source.contains("pub struct AcpSessionSetup"));
}

#[tokio::test]
async fn protocol_neutral_session_core_creates_and_filters_sessions() {
    let server = server("agent-acp");
    let session = server
        .create_session(AcpSessionSetup::new("/tmp/acp-workspace"))
        .await;
    server
        .create_session(AcpSessionSetup::new("/tmp/other-workspace"))
        .await;

    let list = server
        .list_sessions(Some(std::path::Path::new("/tmp/acp-workspace")))
        .await;
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].session_id, session.session_id);
    assert_eq!(list[0].cwd, std::path::PathBuf::from("/tmp/acp-workspace"));
}

#[tokio::test]
async fn protocol_neutral_resume_and_close_require_existing_session() {
    let server = server("agent-acp");
    let session = server.create_session(AcpSessionSetup::new("/tmp")).await;

    assert!(
        server
            .resume_session(&session.session_id, AcpSessionSetup::new("/tmp/resumed"),)
            .await
    );
    assert!(server.close_session(&session.session_id).await);
    assert!(
        !server
            .resume_session(&session.session_id, AcpSessionSetup::new("/tmp"))
            .await
    );
}
