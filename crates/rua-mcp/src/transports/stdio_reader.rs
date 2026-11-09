use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncBufRead, AsyncBufReadExt};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use mcp_manager::error::{McpError, McpResult};

pub type PendingMap = Arc<Mutex<HashMap<u64, oneshot::Sender<McpResult<serde_json::Value>>>>>;

pub struct ReaderTask {
    server_key: String,
    pending: PendingMap,
}

impl ReaderTask {
    pub fn spawn<R>(stdout: R, pending: PendingMap, server_key: String) -> JoinHandle<()>
    where
        R: AsyncBufRead + Unpin + Send + 'static,
    {
        let task = ReaderTask {
            server_key,
            pending,
        };
        tokio::spawn(async move {
            task.run(stdout).await;
        })
    }

    async fn run<R>(self, mut stdout: R)
    where
        R: AsyncBufRead + Unpin,
    {
        let mut line = String::new();
        let exit_error = loop {
            line.clear();
            match stdout.read_line(&mut line).await {
                Ok(0) => {
                    break McpError::TransportClosed {
                        server: self.server_key.clone(),
                        message: "stdout closed (EOF)".into(),
                    };
                }
                Ok(_) => self.handle_line(line.trim()).await,
                Err(e) => {
                    tracing::warn!(
                        server = %self.server_key,
                        error = %e,
                        "stdout read error",
                    );
                    break McpError::TransportClosed {
                        server: self.server_key.clone(),
                        message: format!("stdout read error: {e}"),
                    };
                }
            }
        };
        self.fail_pending(exit_error).await;
    }

    async fn handle_line(&self, line: &str) {
        if line.is_empty() {
            return;
        }
        let value: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(
                    server = %self.server_key,
                    error = %e,
                    line_len = line.len(),
                    "invalid JSON from stdout (dropped)",
                );
                return;
            }
        };

        match value.get("id").and_then(|v| v.as_u64()) {
            Some(id) => self.dispatch_response(id, value).await,
            None => {
                let method = value.get("method").and_then(|v| v.as_str()).unwrap_or("?");
                tracing::debug!(
                    server = %self.server_key,
                    method = %method,
                    "received notification (dropped)",
                );
            }
        }
    }

    async fn dispatch_response(&self, id: u64, value: serde_json::Value) {
        let sender = self.pending.lock().expect("pending MCP calls").remove(&id);
        match sender {
            Some(tx) => {
                let result = if let Some(error) = value.get("error") {
                    let msg = error
                        .get("message")
                        .and_then(|m| m.as_str())
                        .unwrap_or("unknown error");
                    Err(McpError::ProtocolError {
                        server: self.server_key.clone(),
                        message: msg.to_string(),
                    })
                } else {
                    Ok(value
                        .get("result")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null))
                };

                let _ = tx.send(result);
            }
            None => {
                tracing::warn!(
                    server = %self.server_key,
                    id,
                    "response for unknown id (already cancelled or timed out)",
                );
            }
        }
    }

    async fn fail_pending(&self, error: McpError) {
        let mut pending = self.pending.lock().expect("pending MCP calls");
        for (_id, tx) in pending.drain() {
            let _ = tx.send(Err(error.clone()));
        }
    }
}

impl Drop for ReaderTask {
    fn drop(&mut self) {
        for (_, sender) in self.pending.lock().expect("pending MCP calls").drain() {
            let _ = sender.send(Err(McpError::TransportClosed {
                server: self.server_key.clone(),
                message: "MCP reader stopped".into(),
            }));
        }
    }
}

/// A dropped caller must not leave a sender in the connection for an unbounded reply.
pub(super) struct PendingRequest {
    pub pending: PendingMap,
    pub id: u64,
    pub cancel: Option<(
        Arc<super::stdio::StdinWriter>,
        tokio_util::sync::CancellationToken,
    )>,
}
impl Drop for PendingRequest {
    fn drop(&mut self) {
        let unfinished = self
            .pending
            .lock()
            .expect("pending MCP calls")
            .remove(&self.id)
            .is_some();
        if unfinished {
            if let Some((stdin, stop)) = self.cancel.take() {
                stdin.cancel_request(self.id, stop);
            }
        }
    }
}
