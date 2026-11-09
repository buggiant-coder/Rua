use std::convert::Infallible;
use std::sync::Arc;

use async_trait::async_trait;
use axum::http::{Method, StatusCode};
use http_body_util::BodyExt;
use rua_channel::core::console::{
    ConsoleChatFacade, ConsoleChatRequest, ConsoleSubmitOutcome, SseOutputStream,
};
use rua_core::ChatAddress;
use rua_provider::config_loader::{DefaultModel, ProviderConfig};
use serde_json::json;

use super::support::{
    build_app_with_console_facade, json as response_json, local_device_id, request,
};

struct ProjectCliFacade {
    busy: bool,
    turn_id: tokio::sync::watch::Sender<Option<String>>,
}

#[async_trait]
impl ConsoleChatFacade for ProjectCliFacade {
    async fn connect_scoped_chat_activity(
        &self,
        address: ChatAddress,
        _execution_agent_id: &str,
        _viewer_id: &str,
        _position: Option<rua_core::ConversationActivityPosition>,
    ) -> SseOutputStream {
        assert!(address.project_id().is_some());
        let mut turn_id = self.turn_id.subscribe();
        Box::pin(futures::stream::once(async move {
            let turn_id = loop {
                if let Some(turn_id) = turn_id.borrow().clone() {
                    break turn_id;
                }
                turn_id.changed().await.expect("Project CLI Turn ID sender");
            };
            Ok::<_, Infallible>(
                json!({
                    "type": "stream_replay",
                    "complete": true,
                    "events": [
                        {"type": "text_delta", "delta": "old project reply"},
                        {
                            "type": "turn_terminal",
                            "turn_id": "turn-old-project",
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
                        {"type": "text_delta", "delta": "project reply"},
                        {
                            "object": "message",
                            "type": "message",
                            "status": "completed",
                            "content": [{"type": "text", "text": "project reply"}]
                        },
                        {
                            "object": "response",
                            "status": "completed",
                            "chat_id": "00000000-0000-4000-8000-000000000403"
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
        address: ChatAddress,
        _execution_agent_id: &str,
        request: ConsoleChatRequest,
    ) -> ConsoleSubmitOutcome {
        assert!(address.project_id().is_some());
        let turn_id = request
            .extra
            .get("turn_id")
            .and_then(serde_json::Value::as_str)
            .expect("Project CLI adapter Turn ID")
            .to_string();
        self.turn_id
            .send(Some(turn_id))
            .expect("Project CLI live subscriber");
        let chat_id = request.chat_id.expect("Project CLI Chat ID");
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

async fn create_project(test: &super::support::TestApp) -> String {
    let repo = test._temp.path().join("cli-project");
    std::fs::create_dir_all(&repo).unwrap();
    let response = request(
        &test.app,
        Method::POST,
        "/api/projects",
        Some(json!({
            "resource_device_id": local_device_id(&test.state),
            "resource_root": repo,
            "display_name": "CLI Project",
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    response_json(response).await["project"]["project_id"]
        .as_str()
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn project_cli_chat_streams_one_turn_until_terminal() {
    let (turn_id, _) = tokio::sync::watch::channel(None);
    let test = build_app_with_console_facade(Some(Arc::new(ProjectCliFacade {
        busy: false,
        turn_id,
    })));
    let project_id = create_project(&test).await;
    let response = request(
        &test.app,
        Method::POST,
        &format!(
            "/api/projects/{project_id}/chats/00000000-0000-4000-8000-000000000403/console/chat"
        ),
        Some(json!({
            "agent_id": "default",
            "chat_id": "00000000-0000-4000-8000-000000000403",
            "message": "inspect the project"
        })),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers()["content-type"]
        .to_str()
        .unwrap()
        .starts_with("text/event-stream"));
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let text = std::str::from_utf8(&body).unwrap();
    assert!(!text.contains("old project reply"));
    assert!(text.contains("project reply"));
    assert!(text.contains(r#""type":"turn_terminal""#));
}

#[tokio::test]
async fn project_cli_chat_queues_non_idle_chat() {
    let (turn_id, _) = tokio::sync::watch::channel(None);
    let test = build_app_with_console_facade(Some(Arc::new(ProjectCliFacade {
        busy: true,
        turn_id,
    })));
    let project_id = create_project(&test).await;
    let response = request(
        &test.app,
        Method::POST,
        &format!(
            "/api/projects/{project_id}/chats/00000000-0000-4000-8000-000000000404/console/chat"
        ),
        Some(json!({
            "agent_id": "default",
            "chat_id": "00000000-0000-4000-8000-000000000404",
            "message": "inspect the project"
        })),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let text = std::str::from_utf8(&body).unwrap();
    assert!(text.contains(r#""type":"turn_terminal""#));
}

#[tokio::test]
async fn project_cli_chat_real_facade_has_project_runtime() {
    let test = build_app_with_console_facade(None);
    test.state
        .provider_cells
        .cell("fixture")
        .write(
            serde_json::from_value::<ProviderConfig>(json!({
                "id": "fixture",
                "name": "Fixture",
                "base_url": "https://example.invalid/v1",
                "api_key": "test-key",
                "chat_model": "OpenAIChatModel",
                "models": [{
                    "id": "project-cli-model",
                    "name": "Project CLI Model"
                }]
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    test.state
        .provider_manager
        .reload_provider("fixture")
        .await
        .unwrap();
    test.state
        .provider_manager
        .store_default_model(DefaultModel {
            provider_id: "fixture".to_string(),
            model: "project-cli-model".to_string(),
            source: None,
            extra: Default::default(),
        })
        .await;
    let project_id = create_project(&test).await;
    let response = request(
        &test.app,
        Method::POST,
        &format!(
            "/api/projects/{project_id}/chats/00000000-0000-4000-8000-000000000407/console/chat"
        ),
        Some(json!({
            "agent_id": "default",
            "chat_id": "00000000-0000-4000-8000-000000000407",
            "message": "inspect the project"
        })),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
}
