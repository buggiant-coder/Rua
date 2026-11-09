use std::collections::HashMap;
use std::sync::Arc;

use agent_client_protocol::schema::v1::{
    AgentCapabilities, AvailableCommand, CancelNotification, CloseSessionRequest,
    CloseSessionResponse, ContentBlock, ContentChunk, Implementation, InitializeRequest,
    InitializeResponse, ListSessionsRequest, ListSessionsResponse, McpServer, Meta,
    NewSessionRequest, NewSessionResponse, PermissionOption, PermissionOptionKind, PromptRequest,
    PromptResponse, RequestPermissionOutcome, RequestPermissionRequest, ResourceLink,
    ResumeSessionRequest, ResumeSessionResponse, SessionCapabilities, SessionCloseCapabilities,
    SessionInfo, SessionListCapabilities, SessionNotification, SessionResumeCapabilities,
    SessionUpdate, StopReason, TextContent, ToolCall as AcpToolCall, ToolCallContent, ToolCallId,
    ToolCallStatus, ToolCallUpdate as AcpToolCallUpdate, ToolCallUpdateFields, ToolKind,
};
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::{Agent, Client, ConnectTo, ConnectionTo, Stdio};
use futures::StreamExt;
use rua_contract::agent::AttachmentRef;
use rua_core::event::RuntimeEvent;
use rua_core::message::ContentPart;

use super::{
    AcpApprovalOptionKind, AcpApprovalRequest, AcpProtocolVersion, AcpServerError, AcpSessionSetup,
    ActiveTurn, RuaAcpServer, ACP_AGENT_META_KEY, ACP_ERROR_META_KEY,
};

impl From<AcpApprovalOptionKind> for PermissionOptionKind {
    fn from(value: AcpApprovalOptionKind) -> Self {
        match value {
            AcpApprovalOptionKind::AllowOnce => Self::AllowOnce,
            AcpApprovalOptionKind::AllowAlways => Self::AllowAlways,
            AcpApprovalOptionKind::RejectOnce => Self::RejectOnce,
            AcpApprovalOptionKind::RejectAlways => Self::RejectAlways,
        }
    }
}

#[derive(Debug)]
pub struct MappedRuntimeEvent {
    pub notification: Option<SessionNotification>,
    pub stop_reason: Option<StopReason>,
    pub approval: Option<ApprovalSignal>,
}

#[derive(Debug)]
pub struct ApprovalSignal {
    pub request_id: Option<String>,
}

#[derive(Default)]
pub struct EventMapper {
    tool_inputs: HashMap<String, String>,
    tool_outputs: HashMap<String, String>,
}

impl EventMapper {
    pub fn map(
        &mut self,
        session_id: &agent_client_protocol::schema::v1::SessionId,
        event: RuntimeEvent,
    ) -> MappedRuntimeEvent {
        let mut notification = None;
        let mut stop_reason = None;
        let mut approval = None;

        let update = match event {
            RuntimeEvent::TextDelta { delta, .. } => {
                Some(SessionUpdate::AgentMessageChunk(ContentChunk::new(
                    ContentBlock::Text(agent_client_protocol::schema::v1::TextContent::new(delta)),
                )))
            }
            RuntimeEvent::ThinkingDelta { delta, .. } => {
                Some(SessionUpdate::AgentThoughtChunk(ContentChunk::new(
                    ContentBlock::Text(agent_client_protocol::schema::v1::TextContent::new(delta)),
                )))
            }
            RuntimeEvent::ToolCall { call } => {
                self.tool_inputs.remove(&call.id);
                Some(SessionUpdate::ToolCall(
                    AcpToolCall::new(ToolCallId::new(call.id.as_str()), call.name.clone())
                        .kind(tool_kind(&call.name))
                        .raw_input(Some(call.arguments))
                        .status(ToolCallStatus::InProgress),
                ))
            }
            RuntimeEvent::ToolCallDelta {
                id,
                name,
                arguments_delta,
                ..
            } => {
                let input = self.tool_inputs.entry(id.clone()).or_default();
                input.push_str(&arguments_delta);
                let raw_input = serde_json::from_str(input)
                    .unwrap_or_else(|_| serde_json::Value::String(input.clone()));
                let mut fields = ToolCallUpdateFields::new().raw_input(raw_input);
                if let Some(name) = name {
                    fields = fields.title(name.clone()).kind(tool_kind(&name));
                }
                Some(SessionUpdate::ToolCallUpdate(AcpToolCallUpdate::new(
                    ToolCallId::new(id),
                    fields,
                )))
            }
            RuntimeEvent::ToolResultDelta {
                id, output_delta, ..
            } => {
                let output = self.tool_outputs.entry(id.clone()).or_default();
                output.push_str(&output_delta);
                Some(SessionUpdate::ToolCallUpdate(AcpToolCallUpdate::new(
                    ToolCallId::new(id),
                    ToolCallUpdateFields::new()
                        .raw_output(serde_json::Value::String(output.clone())),
                )))
            }
            RuntimeEvent::ToolResult { result } => {
                self.tool_inputs.remove(&result.call_id);
                self.tool_outputs.remove(&result.call_id);
                let fields = ToolCallUpdateFields::new()
                    .status(if result.is_error {
                        ToolCallStatus::Failed
                    } else {
                        ToolCallStatus::Completed
                    })
                    .content(tool_result_content(&result))
                    .raw_output(serde_json::Value::String(result.text_content()));
                Some(SessionUpdate::ToolCallUpdate(AcpToolCallUpdate::new(
                    ToolCallId::new(result.call_id.as_str()),
                    fields,
                )))
            }
            RuntimeEvent::ToolApprovalPending {
                call_id,
                tool_name,
                request_id,
                ..
            } => {
                approval = Some(ApprovalSignal { request_id });
                Some(SessionUpdate::ToolCall(
                    AcpToolCall::new(ToolCallId::new(call_id), tool_name.clone())
                        .kind(tool_kind(&tool_name))
                        .status(ToolCallStatus::Pending),
                ))
            }
            RuntimeEvent::Error { error } => Some(SessionUpdate::AgentMessageChunk(error_chunk(
                format!("Error: {error}"),
            ))),
            RuntimeEvent::End { finish_reason } => {
                stop_reason = Some(stop_reason_from_runtime(finish_reason.as_deref()));
                None
            }
            RuntimeEvent::ModelCallReset { .. } => {
                self.tool_inputs.clear();
                None
            }
            RuntimeEvent::Start { .. }
            | RuntimeEvent::Usage { .. }
            | RuntimeEvent::IterationStart { .. }
            | RuntimeEvent::IterationEnd { .. }
            | RuntimeEvent::ContextCompaction { .. }
            | RuntimeEvent::AskUser { .. }
            | RuntimeEvent::ToolApprovalRequest { .. } => None,
        };

        if let Some(update) = update {
            notification = Some(SessionNotification::new(session_id.clone(), update));
        }
        MappedRuntimeEvent {
            notification,
            stop_reason,
            approval,
        }
    }
}

pub fn initialize_response(_request: InitializeRequest) -> InitializeResponse {
    let session_capabilities = SessionCapabilities::new()
        .list(SessionListCapabilities::new())
        .resume(SessionResumeCapabilities::new())
        .close(SessionCloseCapabilities::new());
    InitializeResponse::new(ProtocolVersion::V1)
        .agent_capabilities(
            AgentCapabilities::new()
                .load_session(false)
                .session_capabilities(session_capabilities),
        )
        .agent_info(Implementation::new("rua", env!("CARGO_PKG_VERSION")).title("Rua Agent"))
}

pub fn agent(server: Arc<RuaAcpServer>) -> impl ConnectTo<Client> {
    let new_session_server = Arc::clone(&server);
    let resume_server = Arc::clone(&server);
    let close_server = Arc::clone(&server);
    let list_server = Arc::clone(&server);
    let prompt_server = Arc::clone(&server);

    Agent
        .builder()
        .name("rua-acp-v1")
        .on_receive_request(
            async move |req: InitializeRequest, responder, _cx: ConnectionTo<_>| {
                responder.respond(initialize_response(req))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |req: NewSessionRequest, responder, cx: ConnectionTo<_>| {
                if !req.cwd.is_absolute() {
                    return responder.respond_with_error(
                        agent_client_protocol::schema::v1::Error::new(
                            -32602,
                            "ACP session cwd must be an absolute path",
                        ),
                    );
                }
                if !req.additional_directories.is_empty() {
                    return responder.respond_with_error(
                        agent_client_protocol::schema::v1::Error::new(
                            -32602,
                            "Rua ACP v1 does not advertise additional directory support",
                        ),
                    );
                }
                let mcp_config = match mcp_config_from_servers(&req.mcp_servers, &req.cwd) {
                    Ok(config) => config,
                    Err(message) => {
                        return responder.respond_with_error(
                            agent_client_protocol::schema::v1::Error::new(-32602, message),
                        );
                    }
                };
                let session = new_session_server
                    .create_session(AcpSessionSetup::new(req.cwd).with_mcp_config(mcp_config))
                    .await;
                let session_id =
                    agent_client_protocol::schema::v1::SessionId::new(session.session_id);
                let response = NewSessionResponse::new(session_id.clone())
                    .meta(session_meta(&new_session_server));
                responder.respond(response)?;
                send_available_commands(&cx, session_id)
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |req: ResumeSessionRequest, responder, cx: ConnectionTo<_>| {
                if !req.cwd.is_absolute() {
                    return responder.respond_with_error(
                        agent_client_protocol::schema::v1::Error::new(
                            -32602,
                            "ACP session cwd must be an absolute path",
                        ),
                    );
                }
                if !req.additional_directories.is_empty() {
                    return responder.respond_with_error(
                        agent_client_protocol::schema::v1::Error::new(
                            -32602,
                            "Rua ACP v1 does not advertise additional directory support",
                        ),
                    );
                }
                let mcp_config = match mcp_config_from_servers(&req.mcp_servers, &req.cwd) {
                    Ok(config) => config,
                    Err(message) => {
                        return responder.respond_with_error(
                            agent_client_protocol::schema::v1::Error::new(-32602, message),
                        );
                    }
                };
                let session_id = req.session_id;
                if resume_server
                    .resume_session(
                        session_id.0.as_ref(),
                        AcpSessionSetup::new(req.cwd).with_mcp_config(mcp_config),
                    )
                    .await
                {
                    responder.respond(ResumeSessionResponse::new())?;
                    send_available_commands(&cx, session_id)
                } else {
                    responder.respond_with_error(agent_client_protocol::schema::v1::Error::new(
                        -32602,
                        "session not found",
                    ))
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |req: CloseSessionRequest, responder, _cx: ConnectionTo<_>| {
                if close_server.close_session(req.session_id.0.as_ref()).await {
                    responder.respond(CloseSessionResponse::new())
                } else {
                    responder.respond_with_error(agent_client_protocol::schema::v1::Error::new(
                        -32602,
                        "session not found",
                    ))
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |req: ListSessionsRequest, responder, _cx: ConnectionTo<_>| {
                let sessions = list_server
                    .list_sessions(req.cwd.as_deref())
                    .await
                    .into_iter()
                    .map(|session| SessionInfo::new(session.session_id, session.cwd))
                    .collect();
                responder.respond(ListSessionsResponse::new(sessions))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |req: PromptRequest, responder, cx: ConnectionTo<_>| {
                let (content, attachments) = match prompt_to_agent_input(&req.prompt) {
                    Ok(input) => input,
                    Err(message) => {
                        return responder.respond_with_error(
                            agent_client_protocol::schema::v1::Error::new(-32602, message),
                        );
                    }
                };
                let turn = match prompt_server
                    .start_turn(
                        req.session_id.0.as_ref(),
                        content,
                        attachments,
                        AcpProtocolVersion::V1,
                    )
                    .await
                {
                    Ok(turn) => turn,
                    Err(error) => return responder.respond_with_error(server_error(error)),
                };
                let task_server = Arc::clone(&prompt_server);
                let task_cx = cx.clone();
                cx.spawn(async move {
                    run_prompt(task_server, turn, responder, task_cx).await;
                    Ok(())
                })?;
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_notification(
            async move |notification: CancelNotification, _cx: ConnectionTo<_>| {
                server
                    .cancel_session(notification.session_id.0.as_ref())
                    .await;
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
}

pub async fn serve_stdio(server: Arc<RuaAcpServer>) -> Result<(), agent_client_protocol::Error> {
    agent(server).connect_to(Stdio::new()).await
}

async fn run_prompt(
    server: Arc<RuaAcpServer>,
    mut turn: ActiveTurn,
    responder: agent_client_protocol::Responder<PromptResponse>,
    connection: ConnectionTo<Client>,
) {
    let mut mapper = EventMapper::default();
    let mut stop_reason = StopReason::EndTurn;
    let mut stream_error = None;
    let session_id = agent_client_protocol::schema::v1::SessionId::new(turn.acp_session_id.clone());

    while let Some(result) = turn.events.next().await {
        match result {
            Ok(event) => {
                let mapped = mapper.map(&session_id, event);
                if let Some(notification) = mapped.notification {
                    if let Err(error) = connection.send_notification(notification) {
                        stream_error = Some(format!("failed to send ACP session update: {error}"));
                        break;
                    }
                }
                if let Some(signal) = mapped.approval {
                    relay_approval(&server, &connection, &session_id, signal).await;
                }
                if let Some(reason) = mapped.stop_reason {
                    stop_reason = reason;
                    break;
                }
            }
            Err(error) if turn.was_cancelled() => {
                tracing::debug!(error = %error, "ACP runtime stopped after cancellation");
                stop_reason = StopReason::Cancelled;
                break;
            }
            Err(error) => {
                let _ = connection
                    .send_notification(error_notification(&session_id, error.to_string()));
                stream_error = Some(error.to_string());
                break;
            }
        }
    }

    if turn.was_cancelled() {
        stop_reason = StopReason::Cancelled;
    }
    turn.finish().await;

    if let Some(error) = stream_error {
        let _ = responder
            .respond_with_error(agent_client_protocol::schema::v1::Error::new(-32000, error));
    } else {
        let _ = responder.respond(PromptResponse::new(stop_reason));
    }
}

async fn relay_approval(
    server: &RuaAcpServer,
    connection: &ConnectionTo<Client>,
    session_id: &agent_client_protocol::schema::v1::SessionId,
    signal: ApprovalSignal,
) {
    let Some(request_id) = signal.request_id else {
        tracing::warn!(
            session_id = %session_id,
            "ACP tool approval event did not include an approval request id"
        );
        return;
    };

    let pending = match server
        .pending_approval(session_id.0.as_ref(), &request_id)
        .await
    {
        Ok(Some(pending)) => pending,
        Ok(None) => {
            tracing::warn!(request_id, "ACP approval request was not found");
            let _ = server.resolve_approval(&request_id, None).await;
            return;
        }
        Err(error) => {
            tracing::warn!(request_id, error, "failed to load ACP approval request");
            let _ = server.resolve_approval(&request_id, None).await;
            return;
        }
    };

    let permission_request = permission_request(session_id.clone(), &pending);
    let option_id = match connection
        .send_request(permission_request)
        .block_task()
        .await
    {
        Ok(response) => match response.outcome {
            RequestPermissionOutcome::Selected(selected) => Some(selected.option_id.0.to_string()),
            RequestPermissionOutcome::Cancelled => None,
            _ => None,
        },
        Err(error) => {
            tracing::warn!(request_id, error = %error, "ACP client permission request failed");
            None
        }
    };

    if let Err(error) = server
        .resolve_approval(&request_id, option_id.as_deref())
        .await
    {
        tracing::warn!(request_id, error, "failed to resolve ACP approval request");
    }
}

fn permission_request(
    session_id: agent_client_protocol::schema::v1::SessionId,
    pending: &AcpApprovalRequest,
) -> RequestPermissionRequest {
    let tool_call = AcpToolCallUpdate::new(
        ToolCallId::new(pending.tool_call_id.clone()),
        ToolCallUpdateFields::new()
            .title(pending.title.clone())
            .kind(tool_kind(&pending.title))
            .status(ToolCallStatus::Pending)
            .raw_input(pending.raw_input.clone()),
    );
    let options = pending
        .options
        .iter()
        .map(|option| {
            PermissionOption::new(option.id.clone(), option.label.clone(), option.kind.into())
        })
        .collect();
    RequestPermissionRequest::new(session_id, tool_call, options)
}

fn session_meta(server: &RuaAcpServer) -> Meta {
    let mut meta = Meta::new();
    meta.insert(
        ACP_AGENT_META_KEY.to_string(),
        serde_json::Value::String(server.agent_id().to_string()),
    );
    meta
}

pub fn available_commands() -> Vec<AvailableCommand> {
    vec![
        AvailableCommand::new("clear", "Clear current session history"),
        AvailableCommand::new("compact", "Trigger context compaction for this session"),
        AvailableCommand::new("skills", "List enabled skills for this channel"),
    ]
}

fn error_notification(
    session_id: &agent_client_protocol::schema::v1::SessionId,
    error: String,
) -> SessionNotification {
    SessionNotification::new(
        session_id.clone(),
        SessionUpdate::AgentMessageChunk(error_chunk(format!("Error: {error}"))),
    )
}

fn error_chunk(message: String) -> ContentChunk {
    ContentChunk::new(ContentBlock::Text(TextContent::new(message))).meta(error_meta())
}

fn error_meta() -> Meta {
    let mut meta = Meta::new();
    meta.insert(
        ACP_ERROR_META_KEY.to_string(),
        serde_json::Value::Bool(true),
    );
    meta
}

fn tool_result_content(result: &agent_types::tool::ToolResult) -> Vec<ToolCallContent> {
    let mut content = Vec::new();
    let text = tool_result_display_text(result);
    if !text.is_empty() {
        content.push(ToolCallContent::from(ContentBlock::Text(TextContent::new(
            text,
        ))));
    }

    for part in &result.content {
        if let Some(link) = resource_link_from_content_part(part) {
            content.push(ToolCallContent::from(ContentBlock::ResourceLink(link)));
        }
    }

    content
}

fn tool_result_display_text(result: &agent_types::tool::ToolResult) -> String {
    let mut lines = Vec::new();
    for part in &result.content {
        match part {
            ContentPart::Text { text } => lines.push(text.clone()),
            ContentPart::File { name, .. } => lines.push(format!("[File: {name}]")),
            ContentPart::Image { url, alt, .. } => {
                lines.push(format!("[Image: {}]", alt.as_deref().unwrap_or(url)));
            }
            ContentPart::Audio { url, .. } => lines.push(format!("[Audio: {url}]")),
            ContentPart::Video { url, .. } => lines.push(format!("[Video: {url}]")),
            ContentPart::Data { value } => lines.push(value.to_string()),
            ContentPart::Thinking { .. }
            | ContentPart::ToolUse { .. }
            | ContentPart::ToolResult { .. } => {}
            _ => {}
        }
    }
    lines.join("\n")
}

fn resource_link_from_content_part(part: &ContentPart) -> Option<ResourceLink> {
    let (url, name, mime_type, size) = match part {
        ContentPart::File {
            url,
            name,
            media_type,
            size,
        } => (
            url,
            name.clone(),
            media_type.clone(),
            size.map(|s| s as i64),
        ),
        ContentPart::Image {
            url,
            media_type,
            alt,
        } => (
            url,
            alt.clone()
                .unwrap_or_else(|| file_name_from_uri(url).unwrap_or_else(|| url.clone())),
            media_type.clone(),
            None,
        ),
        ContentPart::Audio { url, media_type } | ContentPart::Video { url, media_type } => (
            url,
            file_name_from_uri(url).unwrap_or_else(|| url.clone()),
            media_type.clone(),
            None,
        ),
        _ => return None,
    };

    if !url.starts_with("file://") {
        return None;
    }

    let mut link = ResourceLink::new(name, url.clone());
    if let Some(mime_type) = mime_type {
        link = link.mime_type(mime_type);
    }
    if let Some(size) = size {
        link = link.size(size);
    }
    Some(link)
}

fn file_name_from_uri(uri: &str) -> Option<String> {
    uri.trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .map(ToString::to_string)
}

fn prompt_to_agent_input(
    blocks: &[ContentBlock],
) -> Result<(Vec<ContentPart>, Vec<AttachmentRef>), String> {
    let mut content = Vec::with_capacity(blocks.len());
    let mut attachments = Vec::new();

    for block in blocks {
        match block {
            ContentBlock::Text(text) => {
                content.push(ContentPart::Text {
                    text: text.text.clone(),
                });
            }
            ContentBlock::ResourceLink(resource) => {
                let size = resource.size.and_then(|size| u64::try_from(size).ok());
                content.push(ContentPart::File {
                    url: resource.uri.clone(),
                    name: resource.name.clone(),
                    media_type: resource.mime_type.clone(),
                    size,
                });
                attachments.push(AttachmentRef {
                    url: resource.uri.clone(),
                    name: resource.name.clone(),
                    mime_type: resource.mime_type.clone(),
                });
            }
            ContentBlock::Image(_) => {
                return Err("Rua ACP v1 did not advertise image prompt support".to_string());
            }
            ContentBlock::Audio(_) => {
                return Err("Rua ACP v1 did not advertise audio prompt support".to_string());
            }
            ContentBlock::Resource(_) => {
                return Err("Rua ACP v1 did not advertise embedded context support".to_string());
            }
            _ => return Err("unsupported ACP v1 prompt content block".to_string()),
        }
    }

    Ok((content, attachments))
}

fn mcp_config_from_servers(
    servers: &[McpServer],
    cwd: &std::path::Path,
) -> Result<Option<serde_json::Value>, String> {
    if servers.is_empty() {
        return Ok(None);
    }

    let mut clients = serde_json::Map::new();
    for (index, server) in servers.iter().enumerate() {
        let McpServer::Stdio(stdio) = server else {
            return Err("Rua ACP v1 only advertises the required stdio MCP transport".to_string());
        };
        if stdio.command.as_os_str().is_empty() {
            return Err(format!("MCP server '{}' has an empty command", stdio.name));
        }
        let env = stdio
            .env
            .iter()
            .map(|entry| {
                (
                    entry.name.clone(),
                    serde_json::Value::String(entry.value.clone()),
                )
            })
            .collect::<serde_json::Map<_, _>>();
        let key = format!("acp_{index}_{}", mcp_key(&stdio.name));
        clients.insert(
            key,
            serde_json::json!({
                "name": stdio.name,
                "enabled": true,
                "transport": "stdio",
                "command": stdio.command.to_string_lossy(),
                "args": stdio.args,
                "env": env,
                "cwd": cwd,
                "auto_connect": "manual"
            }),
        );
    }
    Ok(Some(serde_json::json!({ "clients": clients })))
}

fn mcp_key(name: &str) -> String {
    let key = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' || character == '-' {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    if key.is_empty() {
        "server".to_string()
    } else {
        key
    }
}

fn send_available_commands(
    connection: &ConnectionTo<Client>,
    session_id: agent_client_protocol::schema::v1::SessionId,
) -> Result<(), agent_client_protocol::Error> {
    connection.send_notification(SessionNotification::new(
        session_id,
        SessionUpdate::AvailableCommandsUpdate(
            agent_client_protocol::schema::v1::AvailableCommandsUpdate::new(available_commands()),
        ),
    ))
}

fn stop_reason_from_runtime(reason: Option<&str>) -> StopReason {
    match reason.unwrap_or_default().to_ascii_lowercase().as_str() {
        "cancelled" | "canceled" => StopReason::Cancelled,
        "length" | "max_tokens" => StopReason::MaxTokens,
        "max_turn_requests" | "max_iterations" | "iteration_limit" => StopReason::MaxTurnRequests,
        "content_filter" | "safety" | "refusal" => StopReason::Refusal,
        _ => StopReason::EndTurn,
    }
}

fn tool_kind(name: &str) -> ToolKind {
    let name = name.to_ascii_lowercase();
    if name.contains("delete") || name.contains("remove") {
        ToolKind::Delete
    } else if name.contains("move") || name.contains("rename") {
        ToolKind::Move
    } else if name.contains("write") || name.contains("edit") || name.contains("patch") {
        ToolKind::Edit
    } else if name.contains("read") || name.contains("list") || name.contains("get") {
        ToolKind::Read
    } else if name.contains("search") || name.contains("grep") || name.contains("find") {
        ToolKind::Search
    } else if name.contains("fetch") || name.contains("http") || name.contains("web") {
        ToolKind::Fetch
    } else if name.contains("shell")
        || name.contains("bash")
        || name.contains("exec")
        || name.contains("python")
    {
        ToolKind::Execute
    } else {
        ToolKind::Other
    }
}

fn server_error(error: AcpServerError) -> agent_client_protocol::schema::v1::Error {
    match error {
        AcpServerError::SessionNotFound(message) | AcpServerError::SessionBusy(message) => {
            agent_client_protocol::schema::v1::Error::new(-32602, message)
        }
        AcpServerError::Runtime(error) => {
            agent_client_protocol::schema::v1::Error::new(-32000, error.to_string())
        }
    }
}
