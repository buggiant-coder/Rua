use super::handoff::Peer;
use super::*;
use rua_device_network::{DeviceSessionBulkStream, DeviceSessionBulkTransferHandler};
use rua_sync::identity::NetworkSecret;
use serde_json::json;
use sha2::{Digest, Sha256};

struct CountingBulkTransfers {
    inner: Arc<dyn DeviceSessionBulkTransferHandler>,
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl DeviceSessionBulkTransferHandler for CountingBulkTransfers {
    async fn handle_bulk_transfer(&self, stream: DeviceSessionBulkStream) -> Result<(), String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        // Preserve the real handler and stream, including its unread request frame.
        self.inner.handle_bulk_transfer(stream).await
    }
}

async fn count_bulk_transfers(peer: &Peer) -> Arc<AtomicUsize> {
    let node = peer.state.sync_node.lock().await.clone().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let inner = Arc::new(
        peer.state
            .sync_transfer_bulk_handler
            .as_ref()
            .clone()
            .with_sync_blobs(node.blobs_store().clone()),
    );
    node.network_runtime()
        .session_manager()
        .set_bulk_transfer_handler(Arc::new(CountingBulkTransfers {
            inner,
            calls: calls.clone(),
        }));
    calls
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn handoff_opens_no_bulk_stream_until_the_original_media_is_requested() {
    let secret = NetworkSecret::generate();
    let source = Peer::new(&secret).await;
    let target = Peer::new(&secret).await;
    source.pair(&[&target]);
    target.pair(&[&source]);
    let chat = uuid::Uuid::new_v4().to_string();
    let address = ChatAddress::new(&chat);
    source
        .submit(&chat, "keep my original media on its Device")
        .await;
    let bytes = vec![0x71; 128 * 1024];
    let media_id = uuid::Uuid::new_v4();
    let media = rua_session::chat_media::ChatMediaManifest::new(
        media_id,
        &chat,
        &source.device_id,
        "image/png".into(),
        &bytes,
    )
    .unwrap();
    let source_media = source.state.chat_workspace().as_path().join("media");
    let target_media = target.state.chat_workspace().as_path().join("media");
    media.store_copy(&source_media, &bytes).unwrap();
    media
        .record_reference(&source.store().chat_dir(&chat), &chat)
        .unwrap();
    source
        .state
        .security
        .approval_authority()
        .ensure_chat_rules_snapshot(&address)
        .await
        .unwrap();
    let publication = rua_server::sync_publication::publish_sync_fact(
        &source.state,
        rua_server::sync_publication::SyncPublicationFact::StableConversationState {
            scope_root: source.state.chat_workspace().as_path().to_path_buf(),
            address: address.clone(),
        },
    )
    .await;
    assert!(publication.errors.is_empty(), "{:?}", publication.errors);
    target.synchronize(&source).await;
    let original_header = source
        .store()
        .load_chat_header(&chat)
        .await
        .unwrap()
        .unwrap();
    let mirrored_header = target
        .store()
        .load_chat_header(&chat)
        .await
        .unwrap()
        .unwrap();
    assert!(
        original_header.extra["hook_configuration"] == mirrored_header.extra["hook_configuration"]
    );
    let mirrored = rua_session::chat_media::ChatMediaManifest::load_reference(
        &target.store().chat_dir(&chat),
        media_id,
    )
    .unwrap()
    .unwrap();
    assert_eq!(mirrored.sha256, media.sha256);
    assert_eq!(mirrored.device_ids, media.device_ids);
    assert!(!target_media.join(media_id.to_string()).exists());
    let expected_history =
        load_conversation_manifest(source.state.chat_workspace().as_path(), &address)
            .await
            .unwrap()
            .unwrap();
    let actual_history =
        load_conversation_manifest(target.state.chat_workspace().as_path(), &address)
            .await
            .unwrap()
            .unwrap();
    assert!(
        actual_history.compare(&expected_history).matches,
        "all required Chat state is applied before the counted handoff"
    );

    // Count both incoming directions after ordinary metadata synchronization.
    // Zero bulk streams is stronger than checking that no copy survived on disk.
    let source_bulk = count_bulk_transfers(&source).await;
    let target_bulk = count_bulk_transfers(&target).await;
    let (status, moved) = source
        .request(
            Method::PUT,
            &chat,
            "/authority-placement",
            json!({"device_id":target.device_id}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{moved}");
    assert_eq!(target.placement(&chat).await.device_id, target.device_id);
    assert_eq!(
        source_bulk.load(Ordering::SeqCst),
        0,
        "handoff must not fetch bytes from the original Device"
    );
    assert_eq!(
        target_bulk.load(Ordering::SeqCst),
        0,
        "handoff must not push bytes to the new Device"
    );
    assert!(!target_media.join(media_id.to_string()).exists());
    media.verify_copy(&source_media).unwrap();

    // Positive control: the same real bulk handler is reached by an explicit
    // media GET after handoff, and the returned content has the original hash.
    let response = tokio::time::timeout(
        Duration::from_secs(20),
        rua_server::router::build_app(target.state.clone(), None).oneshot(
            Request::builder()
                .uri(format!("/api/chats/{chat}/media/{media_id}"))
                .body(Body::empty())
                .unwrap(),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    let status = response.status();
    let received = axum::body::to_bytes(response.into_body(), bytes.len() + 4096)
        .await
        .unwrap();
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&received)
    );
    assert_eq!(received.len(), bytes.len());
    assert_eq!(Sha256::digest(&received), Sha256::digest(&bytes));
    assert!(
        source_bulk.load(Ordering::SeqCst) > 0,
        "the positive control must reach the original Device's bulk handler"
    );
    source.close().await;
    target.close().await;
}
