use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::RwLock;

use mcp_manager::client::McpClient;
use mcp_manager::error::{McpError, McpResult};
use mcp_manager::status::{McpConnectionState, McpServerStatus};
use mcp_manager::types::{
    McpCallContext, McpCallResult, McpContent, McpInitializeResult, McpToolInfo,
};

use crate::ssrf;

fn parse_sse_jsonrpc(raw: &str, expected_id: u64) -> Option<serde_json::Value> {
    for event in raw.split("\n\n").chain(raw.split("\r\n\r\n")) {
        let mut data_lines: Vec<&str> = Vec::new();
        for line in event.lines() {
            if let Some(rest) = line.strip_prefix("data:") {
                data_lines.push(rest.trim_start());
            }
        }
        if data_lines.is_empty() {
            continue;
        }
        let payload = data_lines.join("\n");
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&payload) {
            let matches_id = value
                .get("id")
                .and_then(|v| v.as_u64())
                .map(|n| n == expected_id)
                .unwrap_or(false);
            if matches_id || value.get("id").is_none() {
                return Some(value);
            }
        }
    }
    None
}

#[derive(Debug, Clone)]
pub struct HttpConfig {
    pub server_key: String,
    pub server_name: String,
    pub url: String,
    pub headers: BTreeMap<String, String>,
    pub timeout: Duration,
    pub call_timeout: Duration,

    pub allow_private_ip: bool,
}

#[derive(Debug)]
struct HttpState {
    connection: McpConnectionState,
    tools: Vec<McpToolInfo>,
    init_result: Option<McpInitializeResult>,
    last_error: Option<String>,
    request_id: u64,
    session_id: Option<String>,
}

pub struct HttpMcpClient {
    config: HttpConfig,
    state: Arc<RwLock<HttpState>>,
    client: reqwest::Client,
}

impl HttpMcpClient {
    pub fn new(config: HttpConfig) -> Self {
        let client = rua_http::build_client_or_default(
            reqwest::Client::builder()
                .pool_idle_timeout(Some(Duration::from_secs(90)))
                .danger_accept_invalid_certs(false),
            "rua-mcp-http",
        );
        Self {
            config,
            state: Arc::new(RwLock::new(HttpState {
                connection: McpConnectionState::NotConnected,
                tools: Vec::new(),
                init_result: None,
                last_error: None,
                request_id: 0,
                session_id: None,
            })),
            client,
        }
    }

    pub async fn send_notification(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> McpResult<()> {
        let state = self.state.read().await;
        let session_id = state.session_id.clone();
        drop(state);

        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });

        let mut req = self
            .client
            .post(&self.config.url)
            .timeout(Duration::from_secs(5));
        for (k, v) in &self.config.headers {
            req = req.header(k.as_str(), v.as_str());
        }
        req = req.header("Content-Type", "application/json");
        req = req.header("Accept", "application/json, text/event-stream");
        if let Some(ref sid) = session_id {
            req = req.header("Mcp-Session-Id", sid.as_str());
        }

        let _ = req.json(&body).send().await;
        Ok(())
    }

    async fn send_request(
        &self,
        method: &str,
        params: serde_json::Value,
        timeout: Duration,
    ) -> McpResult<serde_json::Value> {
        let mut state = self.state.write().await;
        state.request_id += 1;
        let id = state.request_id;
        let session_id = state.session_id.clone();
        drop(state);

        let request_body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });

        let mut req = self.client.post(&self.config.url).timeout(timeout);

        for (k, v) in &self.config.headers {
            req = req.header(k.as_str(), v.as_str());
        }

        req = req.header("Content-Type", "application/json");

        req = req.header("Accept", "application/json, text/event-stream");

        if let Some(ref sid) = session_id {
            req = req.header("Mcp-Session-Id", sid.as_str());
        }

        let response = req.json(&request_body).send().await.map_err(|e| {
            if e.is_timeout() {
                McpError::CallTimeout {
                    server: self.config.server_key.clone(),
                    tool: method.to_string(),
                    timeout_secs: timeout.as_secs(),
                }
            } else if e.is_connect() {
                McpError::TransportClosed {
                    server: self.config.server_key.clone(),
                    message: format!("connection failed: {e}"),
                }
            } else {
                McpError::ProtocolError {
                    server: self.config.server_key.clone(),
                    message: format!("HTTP request failed: {e}"),
                }
            }
        })?;

        if let Some(sid) = response.headers().get("mcp-session-id") {
            if let Ok(sid_str) = sid.to_str() {
                let mut state = self.state.write().await;
                state.session_id = Some(sid_str.to_string());
            }
        }

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            if status.as_u16() == 401 || status.as_u16() == 403 {
                return Err(McpError::AuthFailed {
                    server: self.config.server_key.clone(),
                    status_code: status.as_u16(),
                    message: body,
                });
            }
            return Err(McpError::ProtocolError {
                server: self.config.server_key.clone(),
                message: format!("HTTP {status}: {body}"),
            });
        }

        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();
        let raw = response.text().await.map_err(|e| McpError::ProtocolError {
            server: self.config.server_key.clone(),
            message: format!("failed to read response body: {e}"),
        })?;
        let body: serde_json::Value = if content_type.contains("text/event-stream") {
            parse_sse_jsonrpc(&raw, id).ok_or_else(|| McpError::ProtocolError {
                server: self.config.server_key.clone(),
                message: format!("no JSON-RPC message for id={id} in SSE response"),
            })?
        } else {
            serde_json::from_str(&raw).map_err(|e| McpError::ProtocolError {
                server: self.config.server_key.clone(),
                message: format!("invalid JSON response: {e}"),
            })?
        };

        if let Some(error) = body.get("error") {
            let msg = error
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("unknown error");
            return Err(McpError::ProtocolError {
                server: self.config.server_key.clone(),
                message: msg.to_string(),
            });
        }

        Ok(body
            .get("result")
            .cloned()
            .unwrap_or(serde_json::Value::Null))
    }
}

#[async_trait]
impl McpClient for HttpMcpClient {
    async fn connect(&self) -> McpResult<()> {
        if !self.config.allow_private_ip && ssrf::is_private_url(&self.config.url) {
            return Err(McpError::SsrfBlocked {
                server: self.config.server_key.clone(),
                url: self.config.url.clone(),
            });
        }

        {
            let mut state = self.state.write().await;
            state.connection = McpConnectionState::Connecting;
            state.last_error = None;
        }

        let result = self
            .send_request(
                "initialize",
                serde_json::json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": {
                        "name": "rua",
                        "version": "0.1.0"
                    }
                }),
                self.config.timeout,
            )
            .await;

        let mut state = self.state.write().await;

        match result {
            Ok(result) => {
                let init = McpInitializeResult {
                    server_name: result
                        .get("serverInfo")
                        .and_then(|s| s.get("name"))
                        .and_then(|n| n.as_str())
                        .unwrap_or("unknown")
                        .to_string(),
                    server_version: result
                        .get("serverInfo")
                        .and_then(|s| s.get("version"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown")
                        .to_string(),
                    protocol_version: result
                        .get("protocolVersion")
                        .and_then(|v| v.as_str())
                        .unwrap_or("2024-11-05")
                        .to_string(),
                    capabilities: result
                        .get("capabilities")
                        .and_then(|c| {
                            serde_json::from_value::<BTreeMap<String, serde_json::Value>>(c.clone())
                                .ok()
                        })
                        .unwrap_or_default(),
                };
                state.init_result = Some(init);
                state.connection = McpConnectionState::Ready;

                drop(state);
                let _ = self
                    .send_notification("notifications/initialized", serde_json::json!({}))
                    .await;

                Ok(())
            }
            Err(e) => {
                state.connection = McpConnectionState::Failed;
                state.last_error = Some(e.to_string());
                Err(McpError::HandshakeFailed {
                    server: self.config.server_key.clone(),
                    message: e.to_string(),
                })
            }
        }
    }

    async fn initialize_result(&self) -> McpResult<McpInitializeResult> {
        let state = self.state.read().await;
        state
            .init_result
            .clone()
            .ok_or_else(|| McpError::ProtocolError {
                server: self.config.server_key.clone(),
                message: "not initialized".into(),
            })
    }

    async fn list_tools(&self) -> McpResult<Vec<McpToolInfo>> {
        {
            let state = self.state.read().await;
            if !state.tools.is_empty() {
                return Ok(state.tools.clone());
            }
        }

        let result = self
            .send_request(
                "tools/list",
                serde_json::json!({}),
                self.config.call_timeout,
            )
            .await?;

        let tools_array = result
            .get("tools")
            .and_then(|t| t.as_array())
            .cloned()
            .unwrap_or_default();

        let tools: Vec<McpToolInfo> = tools_array
            .into_iter()
            .map(|t| McpToolInfo {
                server_id: mcp_manager::types::McpServerId::new(&self.config.server_key),
                name: t
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("unknown")
                    .to_string(),
                description: t
                    .get("description")
                    .and_then(|d| d.as_str())
                    .unwrap_or("")
                    .to_string(),
                input_schema: t
                    .get("inputSchema")
                    .cloned()
                    .unwrap_or(serde_json::json!({"type": "object"})),
                raw: t,
            })
            .collect();

        let mut state = self.state.write().await;
        state.tools = tools.clone();

        Ok(tools)
    }

    async fn call_tool(
        &self,
        name: &str,
        args: serde_json::Value,
        ctx: McpCallContext,
    ) -> McpResult<McpCallResult> {
        let timeout = ctx.timeout.unwrap_or(self.config.call_timeout);

        let call_future = self.send_request(
            "tools/call",
            serde_json::json!({
                "name": name,
                "arguments": args,
            }),
            timeout,
        );

        let result = if let Some(mut cancel_rx) = ctx.cancel {
            tokio::select! {
                result = call_future => result,
                _ = async {
                    while cancel_rx.changed().await.is_ok() {
                        if *cancel_rx.borrow() {
                            break;
                        }
                    }
                } => {
                    Err(McpError::Cancelled {
                        server: self.config.server_key.clone(),
                    })
                }
            }
        } else {
            call_future.await
        }?;

        let content = result
            .get("content")
            .and_then(|c| c.as_array())
            .map(|arr| {
                arr.iter()
                    .map(|item| {
                        let item_type = item.get("type").and_then(|t| t.as_str()).unwrap_or("text");
                        match item_type {
                            "text" => McpContent::Text {
                                text: item
                                    .get("text")
                                    .and_then(|t| t.as_str())
                                    .unwrap_or("")
                                    .to_string(),
                            },
                            "image" => McpContent::Image {
                                data: item
                                    .get("data")
                                    .and_then(|d| d.as_str())
                                    .unwrap_or("")
                                    .to_string(),
                                mime_type: item
                                    .get("mimeType")
                                    .and_then(|m| m.as_str())
                                    .unwrap_or("image/png")
                                    .to_string(),
                            },
                            _ => McpContent::Text {
                                text: serde_json::to_string(item).unwrap_or_default(),
                            },
                        }
                    })
                    .collect()
            })
            .unwrap_or_else(|| {
                vec![McpContent::Text {
                    text: serde_json::to_string(&result).unwrap_or_default(),
                }]
            });

        let is_error = result
            .get("isError")
            .and_then(|e| e.as_bool())
            .unwrap_or(false);

        Ok(McpCallResult {
            content,
            structured_content: result.get("structuredContent").cloned(),
            is_error,
        })
    }

    async fn invalidate_tool_cache(&self) {
        let mut state = self.state.write().await;
        state.tools.clear();
    }

    async fn shutdown(&self) -> McpResult<()> {
        let mut state = self.state.write().await;
        state.connection = McpConnectionState::Stopping;
        state.tools.clear();
        state.session_id = None;
        state.connection = McpConnectionState::NotConnected;
        Ok(())
    }

    fn status(&self) -> McpServerStatus {
        match self.state.try_read() {
            Ok(state) => McpServerStatus {
                key: self.config.server_key.clone(),
                name: self.config.server_name.clone(),
                enabled: true,
                state: state.connection,
                transport_type: "streamable_http".to_string(),
                tools_count: state.tools.len(),
                last_error: state.last_error.clone(),
                connected_at: None,
                mobile_unavailable_reason: None,
                config_errors: Vec::new(),
            },
            Err(_) => McpServerStatus {
                key: self.config.server_key.clone(),
                name: self.config.server_name.clone(),
                enabled: true,
                state: McpConnectionState::Connecting,
                transport_type: "streamable_http".to_string(),
                tools_count: 0,
                last_error: None,
                connected_at: None,
                mobile_unavailable_reason: None,
                config_errors: Vec::new(),
            },
        }
    }
}
