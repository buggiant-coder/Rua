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
    mut body: Value,
) -> AgentRequest {
    body.as_object_mut()
        .unwrap()
        .entry("agent_id")
        .or_insert(json!("default"));
    let router = test.app.clone();
    let path = format!("/chats/{}/console/{endpoint}", address.chat_id());
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
    let address = ChatAddress::new(&chat_id);
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
    let address = ChatAddress::new(uuid::Uuid::new_v4().to_string());
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
    let address = ChatAddress::new(uuid::Uuid::new_v4().to_string());
    execute(
        &test,
        &mut receive,
        &address,
        "submit",
        json!({"message": "submit reply", "authority_epoch": "forged-request-epoch"}),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn first_submit_keeps_staged_file_executor_and_reply_after_reopen() {
    use super::support::{multipart_upload_with_fields, read_json, request};
    let (test, mut receive) = app().await;
    let id = uuid::Uuid::new_v4().to_string();
    let address = ChatAddress::new(&id);
    let workspace = test.state.rua_root.join("chats");
    let upload = multipart_upload_with_fields(
        &test.app,
        &format!("/api/chats/{id}/media"),
        &[("agent_id", "default")],
        "file",
        "note.txt",
        b"fixture".to_vec(),
    )
    .await;
    let status = upload.status();
    let upload = read_json(upload).await;
    assert_eq!(status, StatusCode::CREATED, "{upload}");
    assert!(upload.get("authority_epoch").is_none());
    assert!(rua_session::WorkspaceSessionStore::new(&workspace)
        .load_chat_header(&id)
        .await
        .unwrap()
        .is_none());
    assert!(rua_session::chat_index::get(&workspace, &id)
        .await
        .unwrap()
        .is_none());
    let media_ref = upload["media_ref"].as_str().unwrap();
    let invocation = execute(&test, &mut receive, &address, "submit", json!({
        "input": [{"role":"user","content":[{"type":"text","text":"Read the note"},{"type":"file","media_ref":media_ref,"name":"note.txt"}]}]
    })).await;
    assert_eq!(invocation.agent_id.as_str(), "default");

    let state = rua_server::AppState::from_tempdir(
        test.state.rua_root.clone(),
        rua_server::ServerProfile::Test,
    );
    let reopened = rua_server::endpoints::chats::router().with_state(state);
    let response = request(
        &reopened,
        Method::GET,
        &format!("/chats/{id}?agent_id=unrelated&project_id=unrelated"),
    )
    .await;
    let status = response.status();
    let history = read_json(response).await;
    assert_eq!(status, StatusCode::OK, "{history}");
    assert_eq!(history["agent_id"], "default");
    assert_eq!(history["project_id"], Value::Null);
    assert_eq!(history["model"]["model"], "test-model");
    let turns = history["messages"].to_string();
    assert!(
        turns.contains("Read the note") && turns.contains("captured") && turns.contains(media_ref),
        "{turns}"
    );
    let saved = rua_session::WorkspaceSessionStore::new(workspace);
    let turn = saved.read_turn_range(&id, 1, 2).await.unwrap().remove(0);
    assert_eq!(
        turn.metadata[rua_chat::TURN_AGENT_ASSIGNMENT_META_KEY]["agent_id"],
        "default"
    );
}
