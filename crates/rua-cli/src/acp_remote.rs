use std::collections::HashMap;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::{stream, Stream, StreamExt};
use rua_acp::remote::{AcpRuntimeStreamItem, AcpRuntimeTurnRequest};
use rua_acp::server::{
    AcpApprovalBackend, AcpApprovalOption, AcpApprovalOptionKind, AcpApprovalRequest,
};
use rua_contract::agent::{AgentRequest, AgentRuntime, RuntimeWarmupContext};
use rua_core::event::RuntimeEvent;
use rua_core::id::SessionId;
use rua_core::{Result, RuaError};
use rua_security::approval_authority::{ApprovalRequest, ApprovalReuseScope, ApprovalSnapshot};

use crate::http_client::ApiClient;

pub async fn probe_runtime(client: &ApiClient, agent_id: &str) -> anyhow::Result<bool> {
    let url = client.streaming_url(&format!("/api/agents/{agent_id}/acp/runtime/health"));
    let response = client
        .inner()
        .get(&url)
        .header("X-Agent-Id", agent_id)
        .timeout(Duration::from_secs(3))
        .send()
        .await;

    match response {
        Ok(response) if response.status().is_success() => Ok(true),
        Ok(response) => {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!(
                "Rua server at {url} does not provide a usable ACP runtime bridge \
                 ({status}): {body}"
            )
        }
        Err(error) if error.is_connect() => Ok(false),
        Err(error) => Err(anyhow::anyhow!(
            "failed to probe Rua ACP runtime bridge at {url}: {error}"
        )),
    }
}

pub struct HttpAgentRuntime {
    client: Arc<ApiClient>,
    agent_id: String,
}

impl HttpAgentRuntime {
    pub fn new(client: Arc<ApiClient>, agent_id: impl Into<String>) -> Self {
        Self {
            client,
            agent_id: agent_id.into(),
        }
    }
}

struct RemoteEventStreamState {
    chunks: Pin<Box<dyn Stream<Item = std::result::Result<bytes::Bytes, reqwest::Error>> + Send>>,
    buffer: Vec<u8>,
    done: bool,
}

fn decode_stream_item(line: &[u8]) -> Result<RuntimeEvent> {
    let item = serde_json::from_slice::<AcpRuntimeStreamItem>(line).map_err(|error| {
        RuaError::Runtime {
            message: format!("decode remote ACP runtime event: {error}"),
        }
    })?;
    match item {
        AcpRuntimeStreamItem::Event { event } => Ok(*event),
        AcpRuntimeStreamItem::Error { error } => Err(error),
    }
}

fn response_event_stream(
    response: reqwest::Response,
) -> Pin<Box<dyn Stream<Item = Result<RuntimeEvent>> + Send>> {
    let state = RemoteEventStreamState {
        chunks: Box::pin(response.bytes_stream()),
        buffer: Vec::new(),
        done: false,
    };

    Box::pin(stream::unfold(state, |mut state| async move {
        loop {
            if let Some(newline) = state.buffer.iter().position(|byte| *byte == b'\n') {
                let mut line = state.buffer.drain(..=newline).collect::<Vec<_>>();
                line.pop();
                if line.is_empty() {
                    continue;
                }
                return Some((decode_stream_item(&line), state));
            }

            if state.done {
                if state.buffer.is_empty() {
                    return None;
                }
                let line = std::mem::take(&mut state.buffer);
                return Some((decode_stream_item(&line), state));
            }

            match state.chunks.next().await {
                Some(Ok(chunk)) => state.buffer.extend_from_slice(&chunk),
                Some(Err(error)) => {
                    state.done = true;
                    return Some((
                        Err(RuaError::Io {
                            message: format!("read remote ACP runtime stream: {error}"),
                        }),
                        state,
                    ));
                }
                None => state.done = true,
            }
        }
    }))
}

#[async_trait]
impl AgentRuntime for HttpAgentRuntime {
    async fn stream(
        &self,
        mut req: AgentRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<RuntimeEvent>> + Send>>> {
        let workspace_dir = req.workspace_dir.clone().ok_or_else(|| RuaError::Runtime {
            message: "ACP runtime request is missing workspace_dir".into(),
        })?;
        let mcp_config_handle = req
            .metadata
            .remove(rua_acp::server::ACP_MCP_CONFIG_HANDLE_META_KEY)
            .and_then(|value| value.as_str().map(str::to_string));
        let mcp_config = mcp_config_handle
            .as_deref()
            .and_then(rua_acp::server::take_request_mcp_config);
        let body = AcpRuntimeTurnRequest {
            request: req,
            workspace_dir,
            mcp_config,
        };
        let url = self
            .client
            .streaming_url(&format!("/api/agents/{}/acp/runtime/turn", self.agent_id));
        let response = self
            .client
            .inner()
            .post(&url)
            .header("X-Agent-Id", &self.agent_id)
            .json(&body)
            .send()
            .await
            .map_err(|error| RuaError::Io {
                message: format!("POST remote ACP runtime turn: {error}"),
            })?;
        if !response.status().is_success() {
            let status = response.status();
            let detail = response.text().await.unwrap_or_default();
            return Err(RuaError::Runtime {
                message: format!("remote ACP runtime returned {status}: {detail}"),
            });
        }
        Ok(response_event_stream(response))
    }

    async fn cancel(&self, session_id: &SessionId) -> Result<()> {
        let path = format!(
            "/api/agents/{}/acp/runtime/{}/cancel",
            self.agent_id, session_id
        );
        self.client
            .post(&path, &serde_json::json!({}))
            .await
            .map(|_| ())
            .map_err(|error| RuaError::Runtime {
                message: format!("cancel remote ACP runtime turn: {error}"),
            })
    }

    async fn warmup(&self, _context: RuntimeWarmupContext) -> Result<()> {
        Ok(())
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}

pub struct HttpApprovalBackend {
    client: Arc<ApiClient>,
    requests: Mutex<HashMap<String, ApprovalRequest>>,
}

impl HttpApprovalBackend {
    pub fn new(client: Arc<ApiClient>) -> Self {
        Self {
            client,
            requests: Mutex::new(HashMap::new()),
        }
    }

    async fn fetch_pending(
        &self,
        agent_id: &str,
        session_id: &str,
        request_id: &str,
    ) -> std::result::Result<ApprovalRequest, String> {
        let value = self
            .client
            .get(&format!(
                "/api/agents/{agent_id}/approvals/runtime-sessions/{session_id}/snapshot"
            ))
            .await
            .map_err(|error| error.to_string())?;
        let snapshot: ApprovalSnapshot =
            serde_json::from_value(value).map_err(|error| error.to_string())?;
        snapshot
            .pending
            .into_iter()
            .find(|request| {
                request.id == request_id && request.owner.is_runtime_session(agent_id, session_id)
            })
            .ok_or_else(|| "approval request is no longer pending".to_string())
    }
}

fn map_approval_request(request: ApprovalRequest) -> AcpApprovalRequest {
    let mut options = vec![AcpApprovalOption {
        id: "run_once".to_string(),
        label: "Allow once".to_string(),
        kind: AcpApprovalOptionKind::AllowOnce,
    }];
    for (index, option) in request.reuse_options.iter().enumerate() {
        let prefix = match option.scope {
            ApprovalReuseScope::Chat => continue,
            ApprovalReuseScope::RuntimeSession => "allow_runtime_session",
            ApprovalReuseScope::Agent => "allow_agent",
        };
        options.push(AcpApprovalOption {
            id: format!("{prefix}:{index}"),
            label: option.label.clone(),
            kind: AcpApprovalOptionKind::AllowAlways,
        });
    }
    options.push(AcpApprovalOption {
        id: "reject".to_string(),
        label: "Reject".to_string(),
        kind: AcpApprovalOptionKind::RejectOnce,
    });

    AcpApprovalRequest {
        request_id: request.id,
        tool_call_id: request.tool_call_id,
        title: format!("{} requires approval", request.tool_name),
        raw_input: request.tool_call.args,
        options,
    }
}

fn option_index(option_id: &str, prefix: &str) -> std::result::Result<usize, String> {
    option_id
        .strip_prefix(prefix)
        .ok_or_else(|| "invalid ACP approval option".to_string())?
        .parse()
        .map_err(|_| "invalid ACP approval option index".to_string())
}

#[async_trait]
impl AcpApprovalBackend for HttpApprovalBackend {
    async fn pending_request(
        &self,
        agent_id: &str,
        session_id: &str,
        request_id: &str,
    ) -> std::result::Result<Option<AcpApprovalRequest>, String> {
        let request = self.fetch_pending(agent_id, session_id, request_id).await?;
        self.requests
            .lock()
            .map_err(|_| "ACP approval cache poisoned".to_string())?
            .insert(request.id.clone(), request.clone());
        Ok(Some(map_approval_request(request)))
    }

    async fn resolve(
        &self,
        agent_id: &str,
        request_id: &str,
        option_id: Option<&str>,
    ) -> std::result::Result<(), String> {
        let request = self
            .requests
            .lock()
            .map_err(|_| "ACP approval cache poisoned".to_string())?
            .get(request_id)
            .cloned()
            .ok_or_else(|| "approval request is no longer pending".to_string())?;

        let body = match option_id {
            Some("run_once") => serde_json::json!({
                "decision": "run_once",
                "decided_by": "acp-client",
            }),
            Some(option_id) if option_id.starts_with("allow_runtime_session:") => {
                let index = option_index(option_id, "allow_runtime_session:")?;
                let option = request
                    .reuse_options
                    .get(index)
                    .filter(|option| option.scope == ApprovalReuseScope::RuntimeSession)
                    .ok_or_else(|| "invalid runtime session approval option".to_string())?;
                serde_json::json!({
                    "decision": "allow_runtime_session",
                    "decided_by": "acp-client",
                    "matcher": option.matcher,
                })
            }
            Some(option_id) if option_id.starts_with("allow_agent:") => {
                let index = option_index(option_id, "allow_agent:")?;
                let option = request
                    .reuse_options
                    .get(index)
                    .filter(|option| option.scope == ApprovalReuseScope::Agent)
                    .ok_or_else(|| "invalid agent approval option".to_string())?;
                serde_json::json!({
                    "decision": "allow_agent",
                    "decided_by": "acp-client",
                    "matcher": option.matcher,
                })
            }
            _ => serde_json::json!({
                "decision": "reject",
                "decided_by": "acp-client",
            }),
        };
        let result = self
            .client
            .post(
                &format!("/api/agents/{agent_id}/approvals/{request_id}/decide"),
                &body,
            )
            .await
            .map(|_| ())
            .map_err(|error| error.to_string());
        if result.is_ok() {
            self.requests
                .lock()
                .map_err(|_| "ACP approval cache poisoned".to_string())?
                .remove(request_id);
        }
        result
    }

    async fn cancel_session(
        &self,
        agent_id: &str,
        session_id: &str,
    ) -> std::result::Result<(), String> {
        self.requests
            .lock()
            .map_err(|_| "ACP approval cache poisoned".to_string())?
            .retain(|_, request| !request.owner.is_runtime_session(agent_id, session_id));
        Ok(())
    }
}
