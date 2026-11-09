use std::net::SocketAddr;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

pub fn echo_server_script() -> String {
    r#"
while IFS= read -r line; do
    method=$(echo "$line" | sed -n 's/.*"method"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')
    id=$(echo "$line" | sed -n 's/.*"id"[[:space:]]*:[[:space:]]*\([0-9]*\).*/\1/p')
    case "$method" in
        initialize)
            echo "{\"jsonrpc\":\"2.0\",\"id\":${id},\"result\":{\"protocolVersion\":\"2024-11-05\",\"serverInfo\":{\"name\":\"echo-fixture\",\"version\":\"0.1.0\"},\"capabilities\":{\"tools\":{}}}}"
            ;;
        notifications/initialized)
            ;;
        tools/list)
            echo "{\"jsonrpc\":\"2.0\",\"id\":${id},\"result\":{\"tools\":[{\"name\":\"echo\",\"description\":\"Echoes back the input text\",\"inputSchema\":{\"type\":\"object\",\"properties\":{\"text\":{\"type\":\"string\",\"description\":\"Text to echo back\"}},\"required\":[\"text\"]}}]}}"
            ;;
        tools/call)
            text=$(echo "$line" | sed -n 's/.*"text"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')
            echo "{\"jsonrpc\":\"2.0\",\"id\":${id},\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"${text}\"}],\"isError\":false}}"
            ;;
        ping)
            echo "{\"jsonrpc\":\"2.0\",\"id\":${id},\"result\":{}}"
            ;;
        *)
            echo "{\"jsonrpc\":\"2.0\",\"id\":${id},\"error\":{\"code\":-32601,\"message\":\"method not found\"}}"
            ;;
    esac
done
"#
    .to_string()
}

pub struct EchoMcpServer {
    addr: SocketAddr,
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
}

impl EchoMcpServer {
    pub async fn start(port: u16) -> std::io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", port)).await?;
        let addr = listener.local_addr()?;
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();

        tokio::spawn(async move {
            Self::serve(listener, shutdown_rx).await;
        });

        Ok(Self {
            addr,
            shutdown_tx: Some(shutdown_tx),
        })
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn url(&self) -> String {
        format!("http://{}/mcp", self.addr)
    }

    pub fn shutdown(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }

    async fn serve(listener: TcpListener, mut shutdown_rx: tokio::sync::oneshot::Receiver<()>) {
        loop {
            tokio::select! {
                accept = listener.accept() => {
                    match accept {
                        Ok((stream, _)) => {
                            tokio::spawn(Self::handle_connection(stream));
                        }
                        Err(_) => break,
                    }
                }
                _ = &mut shutdown_rx => break,
            }
        }
    }

    async fn handle_connection(stream: tokio::net::TcpStream) {
        let mut reader = BufReader::new(stream);
        let mut request = String::new();
        let mut content_length: usize = 0;

        loop {
            let mut line = String::new();
            match reader.read_line(&mut line).await {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
            if line.trim().is_empty() {
                break;
            }
            if let Some(val) = line
                .strip_prefix("Content-Length:")
                .or_else(|| line.strip_prefix("content-length:"))
            {
                content_length = val.trim().parse().unwrap_or(0);
            }
        }

        if content_length > 0 {
            let mut body = vec![0u8; content_length];
            if tokio::io::AsyncReadExt::read_exact(&mut reader, &mut body)
                .await
                .is_err()
            {
                return;
            }
            request = String::from_utf8_lossy(&body).to_string();
        }

        let response_body = Self::handle_jsonrpc(&request);
        let response_bytes = response_body.as_bytes();

        let http_response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            response_bytes.len(),
            response_body,
        );

        let stream = reader.into_inner();
        let mut writer = tokio::io::BufWriter::new(stream);
        let _ = writer.write_all(http_response.as_bytes()).await;
        let _ = writer.flush().await;
    }

    pub fn handle_jsonrpc(request: &str) -> String {
        let parsed: Value = match serde_json::from_str(request) {
            Ok(v) => v,
            Err(_) => {
                return json!({
                    "jsonrpc": "2.0",
                    "id": null,
                    "error": {"code": -32700, "message": "Parse error"}
                })
                .to_string();
            }
        };

        let method = parsed.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let id = parsed.get("id").cloned().unwrap_or(Value::Null);

        match method {
            "initialize" => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": "2024-11-05",
                    "serverInfo": {"name": "echo-fixture", "version": "0.1.0"},
                    "capabilities": {"tools": {}}
                }
            })
            .to_string(),

            "notifications/initialized" => String::new(),

            "tools/list" => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "tools": [{
                        "name": "echo",
                        "description": "Echoes back the input text",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "text": {
                                    "type": "string",
                                    "description": "Text to echo back"
                                }
                            },
                            "required": ["text"]
                        }
                    }]
                }
            })
            .to_string(),

            "tools/call" => {
                let params = parsed.get("params").cloned().unwrap_or(json!({}));
                let text = params
                    .get("arguments")
                    .and_then(|a| a.get("text"))
                    .and_then(|t| t.as_str())
                    .unwrap_or("");

                json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "content": [{"type": "text", "text": text}],
                        "isError": false
                    }
                })
                .to_string()
            }

            "ping" => json!({"jsonrpc": "2.0", "id": id, "result": {}}).to_string(),

            _ => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {"code": -32601, "message": "Method not found"}
            })
            .to_string(),
        }
    }
}

impl Drop for EchoMcpServer {
    fn drop(&mut self) {
        self.shutdown();
    }
}
