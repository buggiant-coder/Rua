use rua_core::ChatAddress;
use rua_session::{
    collaboration::{
        CollaborationDeferredClaim, CollaborationDeliveryClaim, CollaborationDeliveryState,
        CollaborationOutcome, CollaborationRequest,
    },
    WorkspaceSessionStore,
};

#[tokio::test]
async fn stale_request_write_cannot_erase_outcome_or_change_identity() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    let request = CollaborationRequest {
        request_id: uuid::Uuid::new_v4().to_string(),
        source: ChatAddress::agent("a", "source"),
        target: ChatAddress::agent("b", "target"),
        origin_turn_id: "turn".into(),
        origin_tool_call_id: "call".into(),
        input_digest: "digest".into(),
        created_at: "now".into(),
        input_id: None,
        turn_id: None,
        outcome: None,
        delivery: CollaborationDeliveryState::Undecided,
        extra: Default::default(),
    };
    store.save_collaboration_request(&request).await.unwrap();
    let mut completed = request.clone();
    completed.input_id = Some("accepted".into());
    completed.turn_id = Some("child-turn".into());
    completed.outcome = Some(CollaborationOutcome {
        result_id: "result".into(),
        status: "completed".into(),
        output: "done".into(),
        partial: false,
        extra: Default::default(),
    });
    store.save_collaboration_request(&completed).await.unwrap();
    store.save_collaboration_request(&request).await.unwrap();
    let restored = store
        .load_collaboration_request("target", &request.request_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(restored, completed);
    let mut invalid = request.clone();
    invalid.input_digest = "different".into();
    assert!(store.save_collaboration_request(&invalid).await.is_err());
    let mut invalid = completed.clone();
    invalid.turn_id = Some("another-turn".into());
    assert!(store.save_collaboration_request(&invalid).await.is_err());
    assert_eq!(
        store
            .load_collaboration_request("target", &request.request_id)
            .await
            .unwrap()
            .unwrap(),
        completed
    );
}

#[tokio::test]
async fn deferred_read_and_automatic_delivery_have_one_persisted_winner() {
    let dir = tempfile::tempdir().unwrap();
    let store = std::sync::Arc::new(WorkspaceSessionStore::new(dir.path()));
    let mut request = CollaborationRequest {
        request_id: uuid::Uuid::new_v4().to_string(),
        source: ChatAddress::agent("a", "source"),
        target: ChatAddress::agent("b", "target"),
        origin_turn_id: "turn".into(),
        origin_tool_call_id: "call".into(),
        input_digest: "digest".into(),
        created_at: "now".into(),
        input_id: Some("input".into()),
        turn_id: Some("child-turn".into()),
        outcome: Some(CollaborationOutcome {
            result_id: "result".into(),
            status: "completed".into(),
            output: "done".into(),
            partial: false,
            extra: Default::default(),
        }),
        delivery: CollaborationDeliveryState::Deferred,
        extra: Default::default(),
    };
    store.save_collaboration_request(&request).await.unwrap();

    let automatic_store = std::sync::Arc::clone(&store);
    let automatic_id = request.request_id.clone();
    let automatic = tokio::spawn(async move {
        automatic_store
            .claim_deferred_collaboration_delivery(
                "target",
                &automatic_id,
                CollaborationDeferredClaim::Automatic {
                    observation_id: "result".into(),
                },
            )
            .await
            .unwrap()
            .delivery
    });
    let read_store = std::sync::Arc::clone(&store);
    let read_id = request.request_id.clone();
    let read = tokio::spawn(async move {
        read_store
            .claim_deferred_collaboration_delivery(
                "target",
                &read_id,
                CollaborationDeferredClaim::Read {
                    read_id: "read-call".into(),
                },
            )
            .await
            .unwrap()
            .delivery
    });
    let (automatic_seen, read_seen) = tokio::join!(automatic, read);
    let winner = store
        .load_collaboration_request("target", &request.request_id)
        .await
        .unwrap()
        .unwrap()
        .delivery;
    assert!(matches!(
        winner,
        CollaborationDeliveryState::Automatic { .. } | CollaborationDeliveryState::Read { .. }
    ));
    assert_eq!(automatic_seen.unwrap(), winner);
    assert_eq!(read_seen.unwrap(), winner);

    request.delivery = CollaborationDeliveryState::Deferred;
    store.save_collaboration_request(&request).await.unwrap();
    assert_eq!(
        store
            .load_collaboration_request("target", &request.request_id)
            .await
            .unwrap()
            .unwrap()
            .delivery,
        winner
    );
}

#[tokio::test]
async fn delivery_claim_has_one_persisted_winner_and_cannot_regress() {
    let dir = tempfile::tempdir().unwrap();
    let store = std::sync::Arc::new(WorkspaceSessionStore::new(dir.path()));
    let request = CollaborationRequest {
        request_id: uuid::Uuid::new_v4().to_string(),
        source: ChatAddress::agent("a", "source"),
        target: ChatAddress::agent("b", "target"),
        origin_turn_id: "turn".into(),
        origin_tool_call_id: "call".into(),
        input_digest: "digest".into(),
        created_at: "now".into(),
        input_id: Some("input".into()),
        turn_id: Some("child-turn".into()),
        outcome: None,
        delivery: CollaborationDeliveryState::Undecided,
        extra: Default::default(),
    };
    store.save_collaboration_request(&request).await.unwrap();

    let direct_store = std::sync::Arc::clone(&store);
    let direct_chat = request.target.chat_id().to_string();
    let direct_id = request.request_id.clone();
    let direct = tokio::spawn(async move {
        direct_store
            .claim_collaboration_delivery(
                &direct_chat,
                &direct_id,
                CollaborationDeliveryClaim::Direct,
            )
            .await
            .unwrap()
            .delivery
    });
    let deferred_store = std::sync::Arc::clone(&store);
    let deferred_chat = request.target.chat_id().to_string();
    let deferred_id = request.request_id.clone();
    let deferred = tokio::spawn(async move {
        deferred_store
            .claim_collaboration_delivery(
                &deferred_chat,
                &deferred_id,
                CollaborationDeliveryClaim::Deferred,
            )
            .await
            .unwrap()
            .delivery
    });
    let (direct_seen, deferred_seen) = tokio::join!(direct, deferred);
    let winner = store
        .load_collaboration_request(request.target.chat_id().as_str(), &request.request_id)
        .await
        .unwrap()
        .unwrap()
        .delivery;
    assert!(matches!(
        winner,
        CollaborationDeliveryState::Direct | CollaborationDeliveryState::Deferred
    ));
    assert_eq!(direct_seen.unwrap(), winner);
    assert_eq!(deferred_seen.unwrap(), winner);

    store.save_collaboration_request(&request).await.unwrap();
    assert_eq!(
        store
            .load_collaboration_request(request.target.chat_id().as_str(), &request.request_id)
            .await
            .unwrap()
            .unwrap()
            .delivery,
        winner
    );
}

#[tokio::test]
async fn collaboration_wake_suppression_round_trips_independently() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    assert!(
        !store
            .load_collaboration_wake_state("source")
            .await
            .unwrap()
            .suppressed
    );
    let stopped = store
        .set_collaboration_wake_suppressed("source", true)
        .await
        .unwrap();
    assert!(stopped.suppressed);
    assert!(stopped.stopped_at.is_some());
    assert!(
        store
            .load_collaboration_wake_state("source")
            .await
            .unwrap()
            .suppressed
    );
    let resumed = store
        .set_collaboration_wake_suppressed("source", false)
        .await
        .unwrap();
    assert!(!resumed.suppressed);
    assert!(resumed.stopped_at.is_none());
}

#[tokio::test]
async fn failed_delivery_retry_preserves_observation_identity() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    let mut request = CollaborationRequest {
        request_id: uuid::Uuid::new_v4().to_string(),
        source: ChatAddress::agent("a", "source"),
        target: ChatAddress::agent("b", "target"),
        origin_turn_id: "turn".into(),
        origin_tool_call_id: "call".into(),
        input_digest: "digest".into(),
        created_at: "now".into(),
        input_id: Some("input".into()),
        turn_id: Some("target-turn".into()),
        outcome: Some(CollaborationOutcome {
            result_id: "result".into(),
            status: "completed".into(),
            output: "done".into(),
            partial: false,
            extra: Default::default(),
        }),
        delivery: CollaborationDeliveryState::Automatic {
            observation_id: "observation".into(),
        },
        extra: Default::default(),
    };
    store.save_collaboration_request(&request).await.unwrap();
    request = store
        .mark_collaboration_delivery_failed(
            request.target.chat_id().as_str(),
            &request.request_id,
            "observation".into(),
            "source unavailable".into(),
        )
        .await
        .unwrap();
    assert!(matches!(
        request.delivery,
        CollaborationDeliveryState::DeliveryFailed { ref observation_id, .. }
            if observation_id == "observation"
    ));
    let retried = store
        .retry_failed_collaboration_delivery(request.target.chat_id().as_str(), &request.request_id)
        .await
        .unwrap();
    assert_eq!(
        retried.delivery,
        CollaborationDeliveryState::Automatic {
            observation_id: "observation".into(),
        }
    );
}
