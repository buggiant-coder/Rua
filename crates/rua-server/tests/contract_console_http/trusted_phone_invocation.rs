use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use axum::http::{Method, StatusCode};
use futures::Stream;
use http_body_util::BodyExt;
use rua_contract::agent::{AgentRequest, AgentRuntime, RuntimeWarmupContext};
use rua_core::{ChatAddress, RuaError, RuntimeEvent};
use rua_provider::config_loader::DefaultModel;
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot};

use super::support::{build_app, request_json, TestApp};

struct CapturedRequest {
    request: AgentRequest,
    resume: oneshot::Sender<()>,
}

struct CapturingRuntime(mpsc::UnboundedSender<CapturedRequest>);

#[async_trait::async_trait]
impl AgentRuntime for CapturingRuntime {
    async fn stream(
        &self,
        request: AgentRequest,
    ) -> rua_core::Result<Pin<Box<dyn Stream<Item = Result<RuntimeEvent, RuaError>> + Send>>> {
        let (resume, wait) = oneshot::channel();
        self.0
            .send(CapturedRequest {
                request: request.clone(),
                resume,
            })
            .expect("capture runtime request");
        let _ = wait.await;
        let events = futures::stream::iter([
            Ok(RuntimeEvent::Start {
                session_id: request.session_id.clone(),
                agent_id: Some(request.agent_id.clone()),
            }),
            Ok(RuntimeEvent::TextDelta {
                delta: "captured".into(),
                message_id: Some(rua_core::MessageId(uuid::Uuid::new_v4().to_string())),
            }),
            Ok(RuntimeEvent::End {
                finish_reason: Some("stop".into()),
            }),
        ]);
        Ok(rua_agent::turn_persistence::persist_runtime_stream(
            &request,
            Box::pin(events),
        ))
    }

    async fn cancel(&self, _: &rua_core::SessionId) -> rua_core::Result<()> {
        Ok(())
    }

    async fn warmup(&self, _: RuntimeWarmupContext) -> rua_core::Result<()> {
        Ok(())
    }

    async fn shutdown(&self) -> rua_core::Result<()> {
        Ok(())
    }
}

async fn app() -> (TestApp, mpsc::UnboundedReceiver<CapturedRequest>) {
    let test = build_app();
    test.state
        .provider_manager
        .store_default_model(DefaultModel {
            provider_id: "test-provider".into(),
            model: "test-model".into(),
            source: None,
            extra: Default::default(),
        })
        .await;
    let (send, receive) = mpsc::unbounded_channel();
    test.state
        .registry
        .get_or_create("default")
        .await
        .unwrap()
        .set_react_runtime(Arc::new(CapturingRuntime(send)))
        .await;
    (test, receive)
}

async fn execute(
    test: &TestApp,
    receive: &mut mpsc::UnboundedReceiver<CapturedRequest>,
    address: &ChatAddress,
    endpoint: &str,
    body: Value,
) -> AgentRequest {
    let router = test.app.clone();
    let path = format!(
        "/agents/default/chats/{}/console/{endpoint}",
        address.chat_id()
    );
    let response = tokio::spawn(async move {
        let response = request_json(&router, Method::POST, &path, body).await;
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, bytes)
    });
    let captured = tokio::time::timeout(Duration::from_secs(10), receive.recv())
        .await
        .expect("HTTP request reaches Agent runtime")
        .expect("captured request");
    let active = test
        .state
        .conversation_authority
        .active_turn(address)
        .await
        .expect("runtime is executing an Authority-claimed Turn");
    let expected_epoch = json!(active.authority_epoch);
    let expected_turn = active.turn_id;
    let expected_device = active.authority_device_id;
    captured.resume.send(()).unwrap();
    let (status, response_body) = tokio::time::timeout(Duration::from_secs(10), response)
        .await
        .expect("HTTP Turn completes")
        .unwrap();
    assert_eq!(status, StatusCode::OK, "{response_body:?}");
    tokio::time::timeout(Duration::from_secs(10), async {
        while test
            .state
            .conversation_authority
            .active_turn(address)
            .await
            .is_some()
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("Authority finishes the captured Turn");
    assert_eq!(captured.request.metadata["turn_id"], expected_turn);
    assert_eq!(
        captured.request.metadata["authority_device_id"],
        expected_device
    );
    assert_eq!(
        captured.request.metadata.get("authority_epoch"),
        Some(&expected_epoch),
        "Phone Use needs the current Authority claim on every runtime invocation"
    );
    let context = rua_core::phone_use::PhoneSessionContext::from_metadata(
        "default",
        &captured.request.metadata,
    )
    .expect("trusted HTTP metadata is accepted by Phone Use");
    assert_eq!(context.chat_address, *address);
    assert_eq!(context.turn_id, expected_turn);
    assert_eq!(json!(context.authority_epoch), expected_epoch);
    captured.request
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn regenerated_http_turn_has_the_current_trusted_phone_authority() {
    let (test, mut receive) = app().await;
    let chat_id = uuid::Uuid::new_v4().to_string();
    let address = ChatAddress::agent("default", &chat_id);
    let first = execute(
        &test,
        &mut receive,
        &address,
        "chat",
        json!({"message": "first reply"}),
    )
    .await;
    let regenerated = execute(
        &test,
        &mut receive,
        &address,
        "submit",
        json!({
            "message": "regenerate",
            "regenerate_turn_id": first.metadata["turn_id"],
            "authority_epoch": "forged-request-epoch"
        }),
    )
    .await;
    assert_eq!(regenerated.metadata["turn_id"], first.metadata["turn_id"]);
    assert!(!regenerated
        .metadata
        .contains_key(rua_agent::turn_persistence::LOADED_MESSAGE_COUNT_METADATA_KEY));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn streaming_http_turn_has_the_current_trusted_phone_authority() {
    let (test, mut receive) = app().await;
    let address = ChatAddress::agent("default", uuid::Uuid::new_v4().to_string());
    execute(
        &test,
        &mut receive,
        &address,
        "chat",
        json!({"message": "stream reply", "authority_epoch": "forged-request-epoch"}),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn submitted_http_turn_has_the_current_trusted_phone_authority() {
    let (test, mut receive) = app().await;
    let address = ChatAddress::agent("default", uuid::Uuid::new_v4().to_string());
    execute(
        &test,
        &mut receive,
        &address,
        "submit",
        json!({"message": "submit reply", "authority_epoch": "forged-request-epoch"}),
    )
    .await;
}
