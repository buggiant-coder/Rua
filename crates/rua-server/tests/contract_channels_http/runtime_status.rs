use super::support::{build_app, create_agent, delete_agent, read_json, request};
use async_trait::async_trait;
use axum::http::{Method, StatusCode};
use rua_channel::core::{AgentRequest, Channel, OutMessage, Process, ToHandle};
use rua_core::{Result, RuntimeEvent};
use std::sync::Arc;

struct EmptyProcess;
#[async_trait]
impl Process for EmptyProcess {
    async fn handle(&self, _: AgentRequest) -> futures::stream::BoxStream<'static, RuntimeEvent> {
        Box::pin(futures::stream::empty())
    }
}

struct TestChannel(String);
#[async_trait]
impl Channel for TestChannel {
    fn id(&self) -> &String {
        &self.0
    }
    async fn start(&self, _: Arc<dyn Process>) -> Result<()> {
        Ok(())
    }
    async fn stop(&self) -> Result<()> {
        Ok(())
    }
    async fn send(&self, _: ToHandle, _: OutMessage) -> Result<()> {
        Ok(())
    }
    fn build_request_from_native(&self, _: serde_json::Value) -> Result<AgentRequest> {
        unreachable!()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn health_uses_manager_lifecycle_not_registration_or_assumed_connectivity() {
    let app = build_app();
    let agent = "channel_status_contract";
    create_agent(&app.app, agent, "Channel status").await;
    let put = request(
        &app.app,
        Method::PUT,
        &format!("/api/agents/{agent}/config/channels/telegram"),
        Some(serde_json::json!({"enabled": true, "bot_token": ""})),
        None,
    )
    .await;
    assert_eq!(put.status(), StatusCode::OK);
    let manager = app.state.channel_manager_for(agent).await.unwrap();
    manager
        .register(Arc::new(TestChannel("telegram".into())))
        .await
        .unwrap();
    let path = format!("/api/agents/{agent}/config/channels/telegram/health");
    let registered = read_json(request(&app.app, Method::GET, &path, None, None).await).await;
    assert_eq!(registered["status"], "unhealthy");
    assert_eq!(registered["components"]["runtime"], "registered");
    assert_eq!(registered["components"]["transport"], "unknown");
    manager.set_process(Arc::new(EmptyProcess)).await;
    manager.start_all().await.unwrap();
    let running = read_json(request(&app.app, Method::GET, &path, None, None).await).await;
    assert_eq!(running["status"], "healthy");
    assert_eq!(running["components"]["runtime"], "running");
    assert_eq!(running["components"]["transport"], "unknown");
    manager.stop_all().await.unwrap();
    let stopped = read_json(request(&app.app, Method::GET, &path, None, None).await).await;
    assert_eq!(stopped["status"], "unhealthy");
    assert_eq!(stopped["components"]["runtime"], "stopped");
    delete_agent(&app.app, agent).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restart_cannot_claim_success_for_enabled_but_unconfigured_channel() {
    let app = build_app();
    let agent = "channel_restart_contract";
    create_agent(&app.app, agent, "Channel restart").await;
    let base = format!("/api/agents/{agent}/config/channels/telegram");
    let put = request(
        &app.app,
        Method::PUT,
        &base,
        Some(serde_json::json!({"enabled": true, "bot_token": ""})),
        None,
    )
    .await;
    assert_eq!(put.status(), StatusCode::OK);
    let response = request(
        &app.app,
        Method::POST,
        &format!("{base}/restart"),
        None,
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    delete_agent(&app.app, agent).await;
}
