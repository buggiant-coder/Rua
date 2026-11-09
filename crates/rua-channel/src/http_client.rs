use reqwest::Client;

pub(crate) fn default_http_client() -> Client {
    rua_http::default_client_or_default("rua-channel")
}

pub(crate) fn build_http_client(builder: reqwest::ClientBuilder) -> Client {
    rua_http::build_client_or_default(builder, "rua-channel")
}

#[cfg(any(feature = "slack", feature = "discord"))]
pub(crate) async fn connect_websocket(
    client: &reqwest::Client,
    url: &str,
) -> rua_core::Result<tokio_tungstenite::WebSocketStream<reqwest::Upgraded>> {
    use tokio_tungstenite::tungstenite::{handshake::derive_accept_key, protocol::Role};
    let http_url = if let Some(rest) = url.strip_prefix("wss://") {
        format!("https://{rest}")
    } else if let Some(rest) = url.strip_prefix("ws://") {
        format!("http://{rest}")
    } else {
        return Err(rua_core::RuaError::Validation {
            message: "WebSocket URL must use ws or wss".into(),
        });
    };
    let key = tokio_tungstenite::tungstenite::handshake::client::generate_key();
    let response = client
        .get(http_url)
        .version(reqwest::Version::HTTP_11)
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Version", "13")
        .header("Sec-WebSocket-Key", &key)
        .send()
        .await
        .map_err(|error| rua_core::RuaError::Runtime {
            message: format!("WebSocket handshake: {error}"),
        })?;
    if response.status() != reqwest::StatusCode::SWITCHING_PROTOCOLS
        || response
            .headers()
            .get("Sec-WebSocket-Accept")
            .and_then(|v| v.to_str().ok())
            != Some(derive_accept_key(key.as_bytes()).as_str())
    {
        return Err(rua_core::RuaError::Runtime {
            message: "WebSocket upgrade rejected".into(),
        });
    }
    let stream = response
        .upgrade()
        .await
        .map_err(|error| rua_core::RuaError::Runtime {
            message: format!("WebSocket upgrade: {error}"),
        })?;
    Ok(tokio_tungstenite::WebSocketStream::from_raw_socket(stream, Role::Client, None).await)
}
