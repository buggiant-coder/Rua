use std::convert::Infallible;
use std::sync::Arc;

use async_trait::async_trait;
use axum::http::{Method, StatusCode};
use http_body_util::BodyExt;
use rua_channel::core::console::{
    ConsoleChatFacade, ConsoleChatRequest, ConsoleSubmitOutcome, SseOutputStream,
};
use rua_core::ChatAddress;
use rua_provider::config_loader::DefaultModel;
use serde_json::json;

use super::support::{build_app_with_console_facade, request_json};

struct CliChatFacade {
    busy: bool,
    turn_id: tokio::sync::watch::Sender<Option<String>>,
}

#[async_trait]
impl ConsoleChatFacade for CliChatFacade {
    async fn connect_scoped_chat_activity(
        &self,
        _address: ChatAddress,
        _execution_agent_id: &str,
        _viewer_id: &str,
        _position: Option<rua_core::ConversationActivityPosition>,
    ) -> SseOutputStream {
        let mut turn_id = self.turn_id.subscribe();
        Box::pin(futures::stream::once(async move {
            let turn_id = loop {
                if let Some(turn_id) = turn_id.borrow().clone() {
                    break turn_id;
                }
                turn_id.changed().await.expect("CLI Turn ID sender");
            };
            Ok::<_, Infallible>(
                json!({
                    "type": "stream_replay",
                    "complete": true,
                    "events": [
                        {"type": "text_delta", "delta": "old reply"},
                        {
                            "type": "turn_terminal",
                            "turn_id": "turn-old",
                            "status": "completed"
                        },
                        {
                            "type": "conversation_control",
                            "control": {
                                "active_turn": {
                                    "turn_id": turn_id
                                }
                            }
                        },
                        {"type": "text_delta", "delta": "hello"},
                        {
                            "object": "message",
                            "type": "message",
                            "status": "completed",
                            "content": [{"type": "text", "text": "hello"}]
                        },
                        {
                            "object": "response",
                            "status": "completed",
                            "chat_id": "00000000-0000-4000-8000-000000000401"
                        },
                        {
                            "type": "turn_terminal",
                            "turn_id": turn_id,
                            "status": "completed"
                        }
                    ]
                })
                .to_string(),
            )
        }))
    }

    async fn submit_scoped_chat(
        &self,
        _address: ChatAddress,
        _execution_agent_id: &str,
        request: ConsoleChatRequest,
    ) -> ConsoleSubmitOutcome {
        let turn_id = request
            .extra
            .get("turn_id")
            .and_then(serde_json::Value::as_str)
            .expect("CLI adapter Turn ID")
            .to_string();
        self.turn_id
            .send(Some(turn_id))
            .expect("CLI live subscriber");
        let chat_id = request.chat_id.expect("CLI Chat ID");
        if self.busy {
            ConsoleSubmitOutcome::queued(chat_id, Some("input-queued".to_string()))
        } else {
            ConsoleSubmitOutcome::started(chat_id)
        }
    }

    async fn stop_chat(&self, _agent_id: &str, _chat_id: &str) -> bool {
        false
    }
}

fn build_cli_app(busy: bool) -> super::support::TestApp {
    let (turn_id, _) = tokio::sync::watch::channel(None);
    build_app_with_console_facade(Arc::new(CliChatFacade { busy, turn_id }))
}

async fn configure_default_model(test: &super::support::TestApp) {
    test.state
        .provider_manager
        .store_default_model(DefaultModel {
            provider_id: "dashscope".into(),
            model: "qwen3.7-plus".into(),
            source: None,
            extra: Default::default(),
        })
        .await;
}

#[tokio::test]
async fn agent_cli_chat_streams_one_turn_until_terminal() {
    let test = build_cli_app(false);
    configure_default_model(&test).await;
    let response = request_json(
        &test.app,
        Method::POST,
        "/agents/default/chats/00000000-0000-4000-8000-000000000401/console/chat",
        json!({
            "chat_id": "00000000-0000-4000-8000-000000000401",
            "message": "hello"
        }),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers()["content-type"]
        .to_str()
        .unwrap()
        .starts_with("text/event-stream"));
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let text = std::str::from_utf8(&body).unwrap();
    assert!(!text.contains("old reply"));
    assert!(text.contains(r#""type":"text_delta""#));
    assert!(text.contains(r#""type":"turn_terminal""#));
}

#[tokio::test]
async fn agent_cli_chat_queues_non_idle_chat() {
    let test = build_cli_app(true);
    configure_default_model(&test).await;
    let response = request_json(
        &test.app,
        Method::POST,
        "/agents/default/chats/00000000-0000-4000-8000-000000000402/console/chat",
        json!({
            "chat_id": "00000000-0000-4000-8000-000000000402",
            "message": "hello"
        }),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let text = std::str::from_utf8(&body).unwrap();
    assert!(text.contains(r#""type":"turn_terminal""#));
}
