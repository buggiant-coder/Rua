use super::handoff::Peer;
use super::*;
use rua_sync::identity::NetworkSecret;
use serde_json::json;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn handoff_preserves_an_empty_paused_user_queue_without_suppressing_task_results() {
    let secret = NetworkSecret::generate();
    let source = Peer::new(&secret).await;
    let target = Peer::new(&secret).await;
    source.pair(&[&target]);
    target.pair(&[&source]);
    let chat = uuid::Uuid::new_v4().to_string();
    let address = ChatAddress::new(&chat);
    source.submit(&chat, "original conversation").await;
    let placement = source.placement(&chat).await;
    let authority = &source.state.conversation_authority;
    authority
        .start_active_turn(ConversationActiveTurnStart {
            address: address.clone(),
            driver: ConversationDriverKind::Rua,
            authority_device_id: source.device_id.clone(),
            authority_epoch: placement.authority_epoch,
            driver_detail: None,
        })
        .await
        .unwrap();
    let queued = authority
        .enqueue_input(
            &address,
            ConversationInputPayload {
                message: "queued then removed".into(),
                model_override: None,
                input: None,
                messages: None,
                extra: Default::default(),
                attachments: Vec::new(),
                enqueued_at: Instant::now(),
            },
            None,
        )
        .await
        .unwrap();
    assert!(authority.pause_queued_inputs(&address).await);
    authority.finish_active_turn(&address).await;
    authority
        .delete_queued_input(&address, &queued.input_id)
        .await
        .unwrap();
    let before = authority.control_view(&address, &json!({})).await;
    assert!(before.queue.paused);
    assert!(before.queue.entries.is_empty());
    assert!(
        !source
            .store()
            .load_collaboration_wake_state(&chat)
            .await
            .unwrap()
            .suppressed
    );

    let (status, body) = source
        .request(
            Method::PUT,
            &chat,
            "/authority-placement",
            json!({"device_id":target.device_id}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let after = target
        .state
        .conversation_authority
        .control_view(&address, &json!({}))
        .await;
    assert!(after.queue.paused);
    assert!(after.queue.entries.is_empty());
    assert!(
        !target
            .store()
            .load_collaboration_wake_state(&chat)
            .await
            .unwrap()
            .suppressed,
        "pausing user inputs must not become an unrelated Task continuation stop"
    );
    let (status, body) = target
        .request(
            Method::POST,
            &chat,
            "/console/submit",
            json!({
                "message":"stay queued after handoff", "agent_id":"default",
                "submission_id":uuid::Uuid::new_v4().to_string(),
                "model":{"provider_id":"fixture","model":"available"},
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["mode"], "queued", "{body}");
    assert!(target.captured.requests.lock().await.is_empty());
    let after = target
        .state
        .conversation_authority
        .control_view(&address, &json!({}))
        .await;
    assert!(after.queue.paused);
    assert_eq!(after.queue.entries.len(), 1);
    target
        .state
        .conversation_authority
        .clear_queue(&address)
        .await;
    target.close().await;
    source.close().await;
}
