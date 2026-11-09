use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use agent_client_protocol::schema::v1::{
    CancelNotification, ContentBlock, EnvVariable, InitializeRequest, McpServer, McpServerStdio,
    NewSessionRequest, PromptRequest, RequestPermissionOutcome, RequestPermissionRequest,
    RequestPermissionResponse, ResourceLink, SelectedPermissionOutcome, SessionNotification,
    SessionUpdate, StopReason, TextContent,
};
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::{Agent, Client, ConnectionTo};
use async_trait::async_trait;
use futures::{stream, Stream};
use rua_acp::server::v1;
use rua_acp::server::{
    AcpApprovalBackend, AcpApprovalOption, AcpApprovalOptionKind, AcpApprovalRequest, RuaAcpServer,
    ACP_AGENT_META_KEY,
};
use rua_contract::agent::{AgentRequest, AgentRuntime, RuntimeWarmupContext};
use rua_core::event::RuntimeEvent;
use rua_core::id::SessionId;
use rua_core::message::ContentPart;
use rua_core::{Result, RuaError};

struct RecordingRuntime {
    events: Vec<RuntimeEvent>,
    requests: Arc<Mutex<Vec<AgentRequest>>>,
}

#[async_trait]
impl AgentRuntime for RecordingRuntime {
    async fn stream(
        &self,
        mut req: AgentRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = std::result::Result<RuntimeEvent, RuaError>> + Send>>>
    {
        if let Some(handle) = req
            .metadata
            .get(rua_acp::server::ACP_MCP_CONFIG_HANDLE_META_KEY)
            .and_then(serde_json::Value::as_str)
        {
            if let Some(config) = rua_acp::server::take_request_mcp_config(handle) {
                req.metadata
                    .insert("test.captured_mcp_config".to_string(), config);
            }
        }
        self.requests.lock().expect("requests lock").push(req);
        Ok(Box::pin(stream::iter(
            self.events.iter().cloned().map(Ok).collect::<Vec<_>>(),
        )))
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

struct CancelRuntime;

#[async_trait]
impl AgentRuntime for CancelRuntime {
    async fn stream(
        &self,
        req: AgentRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = std::result::Result<RuntimeEvent, RuaError>> + Send>>>
    {
        let mut cancel = req.cancel.expect("ACP request must carry cancellation");
        Ok(Box::pin(stream::once(async move {
            loop {
                if *cancel.borrow() {
                    break;
                }
                if cancel.changed().await.is_err() {
                    break;
                }
            }
            Ok(RuntimeEvent::End {
                finish_reason: Some("cancelled".to_string()),
            })
        })))
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

struct FakeApprovalBackend {
    resolved: Arc<Mutex<Vec<Option<String>>>>,
}

#[async_trait]
impl AcpApprovalBackend for FakeApprovalBackend {
    async fn pending_request(
        &self,
        _agent_id: &str,
        _session_id: &str,
        request_id: &str,
    ) -> std::result::Result<Option<AcpApprovalRequest>, String> {
        Ok(Some(AcpApprovalRequest {
            request_id: request_id.to_string(),
            tool_call_id: "tool-1".to_string(),
            title: "shell requires approval".to_string(),
            raw_input: serde_json::json!({"command": "echo hi"}),
            options: vec![AcpApprovalOption {
                id: "allow".to_string(),
                label: "Allow once".to_string(),
                kind: AcpApprovalOptionKind::AllowOnce,
            }],
        }))
    }

    async fn resolve(
        &self,
        _agent_id: &str,
        _request_id: &str,
        option_id: Option<&str>,
    ) -> std::result::Result<(), String> {
        self.resolved
            .lock()
            .expect("resolved lock")
            .push(option_id.map(str::to_string));
        Ok(())
    }

    async fn cancel_session(
        &self,
        _agent_id: &str,
        _session_id: &str,
    ) -> std::result::Result<(), String> {
        Ok(())
    }
}

#[tokio::test]
async fn rua_client_receives_updates_before_prompt_response() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let runtime = RecordingRuntime {
        events: vec![
            RuntimeEvent::TextDelta {
                delta: "hello from rua".to_string(),
                message_id: None,
            },
            RuntimeEvent::End {
                finish_reason: Some("stop".to_string()),
            },
        ],
        requests: Arc::clone(&requests),
    };
    let server = Arc::new(
        RuaAcpServer::new_with_agent_id(Arc::new(runtime), "agent-acp")
            .with_model("acp-override-provider", "acp-override-model"),
    );
    let saw_text = Arc::new(AtomicBool::new(false));
    let saw_text_handler = Arc::clone(&saw_text);

    Client
        .builder()
        .on_receive_notification(
            async move |notification: SessionNotification, _cx| {
                if matches!(notification.update, SessionUpdate::AgentMessageChunk(_)) {
                    saw_text_handler.store(true, Ordering::SeqCst);
                }
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .connect_with(
            v1::agent(server),
            |connection: ConnectionTo<Agent>| async move {
                let initialize = connection
                    .send_request(InitializeRequest::new(ProtocolVersion::V1))
                    .block_task()
                    .await?;
                assert_eq!(initialize.protocol_version, ProtocolVersion::V1);

                let session = connection
                    .send_request(NewSessionRequest::new("/tmp/acp-project"))
                    .block_task()
                    .await?;
                assert_eq!(
                    session
                        .meta
                        .as_ref()
                        .and_then(|meta| meta.get(ACP_AGENT_META_KEY)),
                    Some(&serde_json::Value::String("agent-acp".to_string()))
                );
                let prompt = vec![
                    ContentBlock::Text(TextContent::new("inspect this file")),
                    ContentBlock::ResourceLink(
                        ResourceLink::new("notes.txt", "file:///tmp/acp-project/notes.txt")
                            .mime_type("text/plain"),
                    ),
                ];
                let response = connection
                    .send_request(PromptRequest::new(session.session_id, prompt))
                    .block_task()
                    .await?;

                assert_eq!(response.stop_reason, StopReason::EndTurn);
                assert!(
                    saw_text.load(Ordering::SeqCst),
                    "session/update must be processed before session/prompt returns"
                );
                Ok(())
            },
        )
        .await
        .expect("Rua ACP client/server connection");

    let requests = requests.lock().expect("requests lock");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].agent_id.as_str(), "agent-acp");
    assert_eq!(
        requests[0].metadata["chat_model_slot"],
        serde_json::json!({
            "provider_id": "acp-override-provider",
            "model": "acp-override-model",
        })
    );
    assert_eq!(
        requests[0].workspace_dir.as_deref(),
        Some(std::path::Path::new("/tmp/acp-project"))
    );
    assert_eq!(requests[0].metadata["acp_protocol"], "v1");
    assert!(matches!(
        &requests[0].messages[0].content[1],
        ContentPart::File { name, .. } if name == "notes.txt"
    ));
    assert_eq!(requests[0].attachments.len(), 1);
}

#[tokio::test]
async fn session_cancel_returns_cancelled_stop_reason() {
    let server = Arc::new(RuaAcpServer::new(Arc::new(CancelRuntime)));

    Client
        .builder()
        .connect_with(
            v1::agent(server),
            |connection: ConnectionTo<Agent>| async move {
                connection
                    .send_request(InitializeRequest::new(ProtocolVersion::V1))
                    .block_task()
                    .await?;
                let session = connection
                    .send_request(NewSessionRequest::new("/tmp"))
                    .block_task()
                    .await?;
                let session_id = session.session_id;
                let prompt = connection.send_request(PromptRequest::new(
                    session_id.clone(),
                    vec![ContentBlock::Text(TextContent::new("wait"))],
                ));

                let cancel_connection = connection.clone();
                let cancel_session_id = session_id.clone();
                let (response, cancel_result) = tokio::join!(prompt.block_task(), async move {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                    cancel_connection.send_notification(CancelNotification::new(cancel_session_id))
                });
                cancel_result?;
                assert_eq!(response?.stop_reason, StopReason::Cancelled);
                Ok(())
            },
        )
        .await
        .expect("cancel ACP prompt");
}

#[tokio::test]
async fn stdio_mcp_servers_are_forwarded_as_session_runtime_config() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let runtime = RecordingRuntime {
        events: vec![RuntimeEvent::End {
            finish_reason: Some("stop".to_string()),
        }],
        requests: Arc::clone(&requests),
    };
    let server = Arc::new(RuaAcpServer::new(Arc::new(runtime)));

    Client
        .builder()
        .connect_with(
            v1::agent(server),
            |connection: ConnectionTo<Agent>| async move {
                connection
                    .send_request(InitializeRequest::new(ProtocolVersion::V1))
                    .block_task()
                    .await?;
                let mcp_server = McpServer::Stdio(
                    McpServerStdio::new("filesystem", "/usr/bin/example-mcp")
                        .args(vec!["--stdio".to_string()])
                        .env(vec![EnvVariable::new("TOKEN", "secret")]),
                );
                let session = connection
                    .send_request(
                        NewSessionRequest::new("/tmp/acp-mcp").mcp_servers(vec![mcp_server]),
                    )
                    .block_task()
                    .await?;
                connection
                    .send_request(PromptRequest::new(
                        session.session_id,
                        vec![ContentBlock::Text(TextContent::new("use the tool"))],
                    ))
                    .block_task()
                    .await?;
                Ok(())
            },
        )
        .await
        .expect("stdio MCP ACP prompt");

    let requests = requests.lock().expect("requests lock");
    let config = requests[0]
        .metadata
        .get("test.captured_mcp_config")
        .expect("ACP MCP runtime config");
    assert_eq!(config["clients"]["acp_0_filesystem"]["transport"], "stdio");
    assert_eq!(
        config["clients"]["acp_0_filesystem"]["command"],
        "/usr/bin/example-mcp"
    );
    assert_eq!(
        config["clients"]["acp_0_filesystem"]["env"]["TOKEN"],
        "secret"
    );
}

#[tokio::test]
async fn permission_request_round_trips_through_client() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let runtime = RecordingRuntime {
        events: vec![
            RuntimeEvent::ToolApprovalPending {
                call_id: "tool-1".to_string(),
                tool_name: "shell".to_string(),
                request_id: Some("approval-1".to_string()),
                prompt: Some("run command".to_string()),
            },
            RuntimeEvent::End {
                finish_reason: Some("stop".to_string()),
            },
        ],
        requests,
    };
    let resolved = Arc::new(Mutex::new(Vec::new()));
    let backend = Arc::new(FakeApprovalBackend {
        resolved: Arc::clone(&resolved),
    });
    let server = Arc::new(RuaAcpServer::new(Arc::new(runtime)).with_approval_backend(backend));

    Client
        .builder()
        .on_receive_request(
            async move |request: RequestPermissionRequest, responder, _cx| {
                assert_eq!(request.options.len(), 1);
                responder.respond(RequestPermissionResponse::new(
                    RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(
                        request.options[0].option_id.clone(),
                    )),
                ))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_with(
            v1::agent(server),
            |connection: ConnectionTo<Agent>| async move {
                connection
                    .send_request(InitializeRequest::new(ProtocolVersion::V1))
                    .block_task()
                    .await?;
                let session = connection
                    .send_request(NewSessionRequest::new("/tmp"))
                    .block_task()
                    .await?;
                let response = connection
                    .send_request(PromptRequest::new(
                        session.session_id,
                        vec![ContentBlock::Text(TextContent::new("run it"))],
                    ))
                    .block_task()
                    .await?;
                assert_eq!(response.stop_reason, StopReason::EndTurn);
                Ok(())
            },
        )
        .await
        .expect("permission ACP prompt");

    assert_eq!(
        *resolved.lock().expect("resolved lock"),
        vec![Some("allow".to_string())]
    );
}
