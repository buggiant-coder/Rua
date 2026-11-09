use std::sync::Arc;

use async_trait::async_trait;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use rua_server::qrcode_auth::{PollResult, QRCodeAuthHandler, QRCodeHandlerRegistry, QRCodeResult};
use rua_server::qrcode_image::generate_qrcode_image;

struct FakeHandler;

#[async_trait]
impl QRCodeAuthHandler for FakeHandler {
    async fn fetch_qrcode(&self, _agent_id: &str) -> Result<QRCodeResult, rua_server::ApiError> {
        Ok(QRCodeResult {
            scan_url: "https://example.com/scan".into(),
            poll_token: "tok-123".into(),
        })
    }

    async fn poll_status(
        &self,
        token: &str,
        _agent_id: &str,
    ) -> Result<PollResult, rua_server::ApiError> {
        Ok(PollResult {
            status: if token == "tok-123" {
                "success".into()
            } else {
                "waiting".into()
            },
            credentials: serde_json::json!({"key": "val"}),
        })
    }
}

#[tokio::test]
async fn qrcode_registry_get_unregistered_returns_none() {
    let registry = QRCodeHandlerRegistry::new();

    assert!(registry.get("nonexistent").await.is_none());
}

#[tokio::test]
async fn qrcode_registry_register_and_dispatch_fetch() {
    let registry = QRCodeHandlerRegistry::new();
    registry
        .register("test_channel", Arc::new(FakeHandler))
        .await;

    let handler = registry.get("test_channel").await.expect("handler");
    let result = handler.fetch_qrcode("agent-1").await.unwrap();

    assert_eq!(result.scan_url, "https://example.com/scan");
    assert_eq!(result.poll_token, "tok-123");
}

#[tokio::test]
async fn qrcode_registry_dispatches_poll_status() {
    let registry = QRCodeHandlerRegistry::new();
    registry.register("ch", Arc::new(FakeHandler)).await;

    let handler = registry.get("ch").await.unwrap();
    let poll = handler.poll_status("tok-123", "agent-1").await.unwrap();

    assert_eq!(poll.status, "success");
    assert_eq!(poll.credentials["key"], "val");
}

#[test]
fn qrcode_image_generates_nonempty_base64_png() {
    let encoded = generate_qrcode_image("https://example.com/scan").unwrap();

    assert!(!encoded.is_empty(), "base64 output must not be empty");
    let bytes = STANDARD.decode(&encoded).expect("valid base64");
    assert!(
        bytes.starts_with(&[0x89, b'P', b'N', b'G']),
        "decoded bytes must be a valid PNG"
    );
}

#[test]
fn qrcode_image_depends_on_scan_url() {
    let a = generate_qrcode_image("https://a.example.com").unwrap();
    let b = generate_qrcode_image("https://b.example.com").unwrap();

    assert_ne!(a, b);
}
