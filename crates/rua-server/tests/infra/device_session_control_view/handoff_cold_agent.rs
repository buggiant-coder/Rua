use super::handoff::{Peer, PersistingCaptureRuntime};
use super::*;
use rua_sync::identity::NetworkSecret;
use serde_json::json;

async fn submit(peer: &Peer, chat: &str, agent: &str, text: &str) {
    let (status, body) = peer
        .request(
            Method::POST,
            chat,
            "/console/submit",
            json!({
                "message": text,
                "agent_id": agent,
                "submission_id": uuid::Uuid::new_v4().to_string(),
                "execution_directory": {"device_id": peer.device_id},
                "model": {"provider_id": "fixture", "model": "available"},
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["accepted"], true, "{body}");
    peer.idle(chat).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn handoff_initializes_a_cold_target_agent_before_taking_the_chat_transition_gate() {
    let secret = NetworkSecret::generate();
    let source = Peer::new(&secret).await;
    let target = Peer::new(&secret).await;
    source.pair(&[&target]);
    target.pair(&[&source]);
    let agent = "handoff-cold-agent";
    let response = rua_server::router::build_app(source.state.clone(), None)
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/agents")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"id": agent, "name": "Cold Handoff Agent"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    source
        .state
        .registry
        .get_or_create(agent)
        .await
        .unwrap()
        .set_react_runtime(Arc::new(PersistingCaptureRuntime(source.captured.clone())))
        .await;

    let chat = uuid::Uuid::new_v4().to_string();
    submit(&source, &chat, agent, "before cold handoff").await;
    target.synchronize(&source).await;
    assert!(target
        .state
        .agent_service
        .get_agent(agent)
        .await
        .unwrap()
        .is_some());
    let target_header = target
        .store()
        .load_chat_header(&chat)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(target_header.agent_id, agent);
    assert_eq!(
        target
            .store()
            .read_turn_range(&chat, 1, 10)
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(
        target.state.registry.get(agent).await.is_none(),
        "the target has the Agent definition and Chat history, but no initialized runtime"
    );
    let old = source.placement(&chat).await;

    let outcome = tokio::time::timeout(
        Duration::from_secs(30),
        source.request(
            Method::PUT,
            &chat,
            "/authority-placement",
            json!({"device_id": target.device_id}),
        ),
    )
    .await;
    let (status, body) = match outcome {
        Ok(response) => response,
        Err(error) => {
            let source_journal = source.store().load_chat_handoff(&chat).await.unwrap();
            let target_journal = target.store().load_chat_handoff(&chat).await.unwrap();
            let cold = target.state.registry.get(agent).await.is_none();
            let _ = tokio::time::timeout(Duration::from_secs(5), async {
                source.close().await;
                target.close().await;
            })
            .await;
            panic!(
                "cold Agent handoff did not complete: {error}; source phase={:?}, target phase={:?}, target runtime still cold={cold}",
                source_journal.map(|record| record.phase),
                target_journal.map(|record| record.phase),
            );
        }
    };
    assert_eq!(status, StatusCode::OK, "{body}");
    let new = target.placement(&chat).await;
    assert_eq!(new.device_id, target.device_id);
    assert!(new.authority_epoch > old.authority_epoch);
    assert_eq!(source.placement(&chat).await, new);
    target
        .state
        .registry
        .get(agent)
        .await
        .expect("handoff must finish initializing the selected Agent")
        .set_react_runtime(Arc::new(PersistingCaptureRuntime(target.captured.clone())))
        .await;
    submit(&target, &chat, agent, "first Turn after cold handoff").await;
    let requests = target.captured.requests.lock().await;
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].agent_id.as_str(), agent);
    assert_eq!(requests[0].session_id.as_str(), chat);
    drop(requests);
    assert_eq!(source.captured.requests.lock().await.len(), 1);
    assert_eq!(
        target
            .store()
            .read_turn_range(&chat, 1, 10)
            .await
            .unwrap()
            .len(),
        2
    );
    source.close().await;
    target.close().await;
}
