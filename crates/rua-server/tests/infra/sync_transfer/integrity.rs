use super::*;
use rua_server::services::chat_transfer::{ChatTransferAuthorization, ChatTransferContext};
use rua_server::services::sync_transfer::{
    build_transfer_manifest_metadata, capture_transfer_digests,
};

struct TestAuthorization {
    calls: AtomicUsize,
    deny_after: usize,
}

#[async_trait::async_trait]
impl ChatTransferAuthorization for TestAuthorization {
    async fn check_receive(
        &self,
        _context: &ChatTransferContext,
        _target: &std::path::Path,
        _confirmed: bool,
    ) -> Result<(), SyncTransferPlanError> {
        if self.calls.fetch_add(1, Ordering::SeqCst) >= self.deny_after {
            Err(SyncTransferPlanError::Remote(
                "target permission revoked".into(),
            ))
        } else {
            Ok(())
        }
    }
}

fn chat_context() -> ChatTransferContext {
    ChatTransferContext {
        operation_id: "prepared-chat-copy".into(),
        extra: Default::default(),
        call: rua_server::services::device_read_check::DeviceReadCheckRequest {
            tool_name: "sync_send".into(),
            source_device_id: "origin".into(),
            target_device_id: "target".into(),
            agent_id: "agent".into(),
            address: ChatAddress::new("chat"),
            authority_epoch: ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
            configuration_epoch: "config".into(),
            turn_id: "turn".into(),
            tool_call_id: "call".into(),
            confirmation: None,
            arguments: serde_json::json!({"source_device_id":"source","target_device_id":"target"}),
        },
    }
}

fn fixture() -> (TempDir, PathBuf, PathBuf) {
    let tmp = TempDir::new().unwrap();
    let source = tmp.path().join("photo.jpg");
    let target = tmp.path().join("target");
    std::fs::write(&source, b"original photo").unwrap();
    std::fs::create_dir(&target).unwrap();
    (tmp, source, target)
}

async fn execute(
    manifest: &TransferManifest,
    plan: &rua_server::services::sync_transfer::ReceivePlan,
) -> Result<rua_server::services::sync_transfer::TransferExecutionOutcome, SyncTransferPlanError> {
    let (_cancel, rx) = tokio::sync::watch::channel(RunControl::Running);
    execute_local_transfer_plan(
        manifest,
        plan,
        TransferExecutionOptions { chunk_size: 3 },
        rx,
        |_| {},
    )
    .await
}

#[test]
fn metadata_discovery_precedes_explicit_content_capture() {
    let (_tmp, source, _target) = fixture();
    let mut manifest = build_transfer_manifest_metadata(std::slice::from_ref(&source)).unwrap();
    assert!(manifest.entries[0].sha256.is_empty());
    std::fs::write(&source, b"modified photo").unwrap();
    capture_transfer_digests(&mut manifest).unwrap();
    assert_eq!(manifest, build_transfer_manifest(&[source]).unwrap());
}

#[tokio::test]
async fn native_directory_manifest_uses_portable_slashes_and_preserves_hierarchy() {
    let tmp = TempDir::new().unwrap();
    let album = tmp.path().join("album");
    let nested = album.join("trip");
    let target = tmp.path().join("target");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::create_dir(&target).unwrap();
    std::fs::write(nested.join("photo.jpg"), b"photo").unwrap();
    let manifest = build_transfer_manifest(&[album]).unwrap();
    // On Windows the input above uses native backslashes, but the wire value
    // must still describe the same three path components on Unix receivers.
    let wire = serde_json::to_value(&manifest).unwrap();
    assert_eq!(wire["entries"][0]["relative_path"], "album/trip/photo.jpg");
    let decoded = serde_json::from_value(wire).unwrap();
    let plan = plan_receive(&target, &decoded, ReceiveConflictPolicy::Fail).unwrap();
    execute(&decoded, &plan).await.unwrap();
    assert_eq!(
        std::fs::read(target.join("album/trip/photo.jpg")).unwrap(),
        b"photo"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn portable_manifest_preserves_literal_unix_backslash_filename() {
    let tmp = TempDir::new().unwrap();
    let source = tmp.path().join(r"album\photo.jpg");
    let target = tmp.path().join("target");
    std::fs::create_dir(&target).unwrap();
    std::fs::write(&source, b"literal filename").unwrap();
    let manifest = build_transfer_manifest(&[source]).unwrap();
    assert_eq!(
        manifest.entries[0].relative_path,
        PathBuf::from(r"album\photo.jpg")
    );
    let plan = plan_receive(&target, &manifest, ReceiveConflictPolicy::Fail).unwrap();
    execute(&manifest, &plan).await.unwrap();
    assert_eq!(
        std::fs::read(target.join(r"album\photo.jpg")).unwrap(),
        b"literal filename"
    );
    assert!(!target.join("album").exists());
}

#[cfg(windows)]
#[test]
fn windows_receiver_rejects_literal_backslash_wire_filename() {
    let (_tmp, source, target) = fixture();
    let mut manifest = build_transfer_manifest(&[source]).unwrap();
    manifest.entries[0].relative_path = PathBuf::from(r"album\photo.jpg");
    assert!(matches!(
        plan_receive(&target, &manifest, ReceiveConflictPolicy::Fail),
        Err(SyncTransferPlanError::InvalidRelativePath(_))
    ));
    assert!(!target.join("album").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn transfer_reads_and_writes_below_search_only_ancestors() {
    use std::os::unix::fs::PermissionsExt;
    struct SearchOnly(PathBuf);
    impl Drop for SearchOnly {
        fn drop(&mut self) {
            let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o700));
        }
    }
    let tmp = TempDir::new().unwrap();
    let source_ancestor = tmp.path().join("source-search-only");
    let source_child = source_ancestor.join("readable");
    let target_root = tmp.path().join("target");
    let target_ancestor = target_root.join("search-only");
    let target_child = target_ancestor.join("writable");
    std::fs::create_dir_all(&source_child).unwrap();
    std::fs::create_dir_all(&target_child).unwrap();
    let source = source_child.join("photo.jpg");
    std::fs::write(&source, b"photo through searchable parent").unwrap();
    let source_guard = SearchOnly(source_ancestor);
    let target_guard = SearchOnly(target_ancestor);
    for ancestor in [&source_guard.0, &target_guard.0] {
        std::fs::set_permissions(ancestor, std::fs::Permissions::from_mode(0o100)).unwrap();
        if unsafe { libc::geteuid() } != 0 {
            assert!(std::fs::read_dir(ancestor).is_err());
        }
    }
    // As on Android /data, full file access is valid although enumerating an
    // ancestor is forbidden. Both digest capture and later copy must work.
    let mut manifest = build_transfer_manifest(&[source]).unwrap();
    manifest.entries[0].relative_path = PathBuf::from("search-only/writable/photo.jpg");
    let plan = plan_receive(&target_root, &manifest, ReceiveConflictPolicy::Fail).unwrap();
    let outcome = execute(&manifest, &plan).await.unwrap();
    assert_eq!(outcome.files_written, 1);
    assert_eq!(
        std::fs::read(target_child.join("photo.jpg")).unwrap(),
        b"photo through searchable parent"
    );
}

#[test]
fn cancel_arriving_before_source_start_prevents_late_start() {
    let store = rua_server::services::sync_transfer::SourceTransferOperationStore::new();
    let cancelled = store.cancel("cancel-first").unwrap();
    assert_eq!(cancelled.status, TransferJobStatus::Cancelled);
    let (snapshot, execution) = store
        .start("cancel-first".into(), 100, "source".into(), "target".into())
        .unwrap();
    assert_eq!(snapshot.status, TransferJobStatus::Cancelled);
    assert!(execution.is_none());
}

#[cfg(unix)]
#[test]
fn source_symlink_substitution_after_metadata_is_rejected_before_hashing() {
    let (tmp, source, _target) = fixture();
    let mut manifest = build_transfer_manifest_metadata(std::slice::from_ref(&source)).unwrap();
    let other = tmp.path().join("not-requested.jpg");
    std::fs::write(&other, b"unselected file").unwrap();
    std::fs::remove_file(&source).unwrap();
    std::os::unix::fs::symlink(other, source).unwrap();
    assert!(capture_transfer_digests(&mut manifest).is_err());
    assert!(manifest.entries[0].sha256.is_empty());
}

#[tokio::test]
async fn mutated_source_never_replaces_existing_target() {
    let (_tmp, source, target) = fixture();
    std::fs::write(target.join("photo.jpg"), b"keep target").unwrap();
    let manifest = build_transfer_manifest(std::slice::from_ref(&source)).unwrap();
    let plan = plan_receive(&target, &manifest, ReceiveConflictPolicy::Overwrite).unwrap();
    std::fs::write(&source, b"modified photo").unwrap();
    assert!(matches!(
        execute(&manifest, &plan).await,
        Err(SyncTransferPlanError::IntegrityMismatch(_))
    ));
    assert_eq!(
        std::fs::read(target.join("photo.jpg")).unwrap(),
        b"keep target"
    );
    assert!(!plan.entries[0].temp_path.exists());
    assert_eq!(std::fs::read(source).unwrap(), b"modified photo");
}

#[tokio::test]
async fn target_created_after_planning_is_not_overwritten() {
    let (_tmp, source, target) = fixture();
    let manifest = build_transfer_manifest(&[source]).unwrap();
    let plan = plan_receive(&target, &manifest, ReceiveConflictPolicy::Fail).unwrap();
    std::fs::write(target.join("photo.jpg"), b"concurrent file").unwrap();
    assert!(matches!(
        execute(&manifest, &plan).await,
        Err(SyncTransferPlanError::TargetChanged(_))
    ));
    assert_eq!(
        std::fs::read(target.join("photo.jpg")).unwrap(),
        b"concurrent file"
    );
    assert!(!plan.entries[0].temp_path.exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_overwrites_have_separate_temps_and_only_one_commit() {
    let (_tmp, source, target) = fixture();
    std::fs::write(target.join("photo.jpg"), b"old target").unwrap();
    let manifest = build_transfer_manifest(&[source]).unwrap();
    let first = plan_receive(&target, &manifest, ReceiveConflictPolicy::Overwrite).unwrap();
    let second = plan_receive(&target, &manifest, ReceiveConflictPolicy::Overwrite).unwrap();
    assert_ne!(first.entries[0].temp_path, second.entries[0].temp_path);
    let (a, b) = tokio::join!(execute(&manifest, &first), execute(&manifest, &second));
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    let outcome = a.or(b).unwrap();
    assert!(outcome.files[0].overwritten);
    assert_eq!(
        std::fs::read(target.join("photo.jpg")).unwrap(),
        b"original photo"
    );
    assert!(!first.entries[0].temp_path.exists());
    assert!(!second.entries[0].temp_path.exists());
}

#[tokio::test]
async fn successful_receipt_reports_actual_renamed_path_and_verified_digest() {
    let (_tmp, source, target) = fixture();
    std::fs::write(target.join("photo.jpg"), b"old target").unwrap();
    let manifest = build_transfer_manifest(&[source]).unwrap();
    let plan = plan_receive(&target, &manifest, ReceiveConflictPolicy::Rename).unwrap();
    let outcome = execute(&manifest, &plan).await.unwrap();
    assert_eq!(outcome.files.len(), 1);
    assert_eq!(
        outcome.files[0].target_path,
        target.canonicalize().unwrap().join("photo (1).jpg")
    );
    assert_eq!(outcome.files[0].sha256, manifest.entries[0].sha256);
    assert_eq!(outcome.files[0].bytes, manifest.total_bytes);
    assert!(!outcome.files[0].overwritten);
}

#[tokio::test]
async fn chat_preparation_keeps_fixed_target_plan_without_creating_directories_or_temps() {
    let (_tmp, source, target) = fixture();
    let mut manifest = build_transfer_manifest_metadata(&[source]).unwrap();
    manifest.entries[0].relative_path = PathBuf::from("album/photo.jpg");
    let context = chat_context();
    let handler = SyncTransferBulkHandler::new("target".into()).with_chat_authorization(Arc::new(
        TestAuthorization {
            calls: AtomicUsize::new(0),
            deny_after: usize::MAX,
        },
    ));
    let request = || TransferStreamSendRequest {
        operation_id: context.operation_id.clone(),
        source_device_id: "source".into(),
        target_device_id: "target".into(),
        manifest: manifest.clone(),
        target_dir: target.clone(),
        conflict_policy: ReceiveConflictPolicy::Rename,
        options: TransferExecutionOptions::default(),
        chat_context: Some(context.clone()),
    };
    let first = handler
        .prepare_chat_receive(request(), false)
        .await
        .unwrap();
    assert_eq!(std::fs::read_dir(&target).unwrap().count(), 0);
    std::fs::create_dir(target.join("album")).unwrap();
    std::fs::write(target.join("album/photo.jpg"), b"concurrent file").unwrap();
    let second = handler
        .prepare_chat_receive(request(), false)
        .await
        .unwrap();
    assert_eq!(
        first, second,
        "the Agent reviewed this exact destination plan"
    );
    assert!(manifest.entries[0].sha256.is_empty());
}

#[tokio::test]
async fn local_chat_revocation_after_copy_prevents_final_commit() {
    use rua_server::services::sync_transfer::{
        execute_local_chat_transfer_plan, LocalTransferAuthorization,
    };
    let (_tmp, source, target) = fixture();
    let manifest = build_transfer_manifest(&[source]).unwrap();
    let plan = plan_receive(&target, &manifest, ReceiveConflictPolicy::Fail).unwrap();
    let context = chat_context();
    let authorization = TestAuthorization {
        calls: AtomicUsize::new(0),
        deny_after: 1,
    };
    let (_cancel, rx) = tokio::sync::watch::channel(RunControl::Running);
    let result = execute_local_chat_transfer_plan(
        &manifest,
        &plan,
        TransferExecutionOptions::default(),
        rx,
        |_| {},
        &LocalTransferAuthorization {
            context: &context,
            confirmed_invocation: false,
            authorization: &authorization,
        },
    )
    .await;
    assert!(result.unwrap_err().to_string().contains("revoked"));
    assert_eq!(authorization.calls.load(Ordering::SeqCst), 2);
    assert_eq!(std::fs::read_dir(target).unwrap().count(), 0);
}

#[tokio::test]
async fn failed_second_file_preserves_first_file_completion_receipt() {
    let (tmp, _source, target) = fixture();
    let source = tmp.path().join("selected");
    std::fs::create_dir(&source).unwrap();
    std::fs::write(source.join("a.jpg"), b"first photo").unwrap();
    std::fs::write(source.join("b.jpg"), b"second photo").unwrap();
    let manifest = build_transfer_manifest(std::slice::from_ref(&source)).unwrap();
    let plan = plan_receive(&target, &manifest, ReceiveConflictPolicy::Fail).unwrap();
    std::fs::write(source.join("b.jpg"), b"edited photo").unwrap();
    let store = rua_server::services::sync_transfer::SourceTransferOperationStore::new();
    let (_, cancel) = store
        .start(
            "partial".into(),
            manifest.total_bytes,
            "source".into(),
            "target".into(),
        )
        .unwrap();
    let error = execute_local_transfer_plan(
        &manifest,
        &plan,
        TransferExecutionOptions::default(),
        cancel.unwrap(),
        |progress| store.update_progress_with_files("partial", &progress),
    )
    .await
    .unwrap_err();
    store.fail("partial", error.to_string());
    let snapshot = store.get("partial").unwrap();
    assert_eq!(snapshot.status, TransferJobStatus::Failed);
    assert_eq!(snapshot.files_written, 1);
    assert_eq!(snapshot.files.len(), 1);
    assert_eq!(
        snapshot.files[0].target_path,
        target.canonicalize().unwrap().join("selected/a.jpg")
    );
    assert_eq!(snapshot.files[0].sha256, manifest.entries[0].sha256);
    assert_eq!(
        std::fs::read(&snapshot.files[0].target_path).unwrap(),
        b"first photo"
    );
    assert!(!target.join("selected/b.jpg").exists());
}

#[test]
fn running_source_cancellation_waits_for_worker_completion_evidence() {
    let store = rua_server::services::sync_transfer::SourceTransferOperationStore::new();
    let (_, cancel) = store
        .start(
            "cancel-running".into(),
            100,
            "source".into(),
            "target".into(),
        )
        .unwrap();
    assert_eq!(
        store.cancel("cancel-running").unwrap().status,
        TransferJobStatus::Running
    );
    assert!(cancel.unwrap().borrow().is_terminal());
    store.mark_unknown(
        "cancel-running",
        "destination completion receipt lost".into(),
    );
    assert_eq!(
        store.get("cancel-running").unwrap().status,
        TransferJobStatus::Unknown
    );
}

#[cfg(unix)]
#[tokio::test]
async fn swapped_parent_symlink_cannot_redirect_commit_or_temp_cleanup() {
    let (tmp, source, target) = fixture();
    let nested = target.join("album");
    let displaced = target.join("displaced");
    let outside = tmp.path().join("outside");
    std::fs::create_dir(&nested).unwrap();
    std::fs::create_dir(&outside).unwrap();
    let mut manifest = build_transfer_manifest(&[source]).unwrap();
    manifest.entries[0].relative_path = PathBuf::from("album/photo.jpg");
    let plan = plan_receive(&target, &manifest, ReceiveConflictPolicy::Fail).unwrap();
    let (_cancel, rx) = tokio::sync::watch::channel(RunControl::Running);
    let mut swapped = false;
    let result = execute_local_transfer_plan(
        &manifest,
        &plan,
        TransferExecutionOptions { chunk_size: 3 },
        rx,
        |_| {
            if !swapped {
                std::fs::rename(&nested, &displaced).unwrap();
                std::os::unix::fs::symlink(&outside, &nested).unwrap();
                swapped = true;
            }
        },
    )
    .await;
    assert!(result.is_err());
    assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
    assert_eq!(std::fs::read_dir(&displaced).unwrap().count(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bulk_digest_mismatch_does_not_publish_changed_source() {
    let (_tmp, source_path, target_dir) = fixture();
    let manifest = build_transfer_manifest(std::slice::from_ref(&source_path)).unwrap();
    std::fs::write(&source_path, b"modified photo").unwrap();
    let source = build_peer(81, [81; 32], vec![peer_entry(82)], None).await;
    let target = build_peer(82, [81; 32], vec![peer_entry(81)], None).await;
    install_connectable_peer(&source, &target);
    target
        .runtime
        .session_manager()
        .set_bulk_transfer_handler(Arc::new(SyncTransferBulkHandler::new(
            target.device_id.clone(),
        )));
    let (_cancel, rx) = tokio::sync::watch::channel(RunControl::Running);
    let result = send_transfer_stream(
        source.runtime.session_manager(),
        TransferStreamSendRequest {
            chat_context: None,
            operation_id: "digest-mismatch".into(),
            source_device_id: source.device_id.clone(),
            target_device_id: target.device_id.clone(),
            manifest,
            target_dir: target_dir.clone(),
            conflict_policy: ReceiveConflictPolicy::Fail,
            options: TransferExecutionOptions { chunk_size: 3 },
        },
        rx,
        |_| {},
    )
    .await;
    assert!(result.unwrap_err().to_string().contains("SHA-256 mismatch"));
    assert!(!target_dir.join("photo.jpg").exists());
    assert_eq!(std::fs::read_dir(&target_dir).unwrap().count(), 0);
    assert_eq!(std::fs::read(source_path).unwrap(), b"modified photo");
    source.runtime.shutdown().await.unwrap();
    target.runtime.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn receiver_losing_committed_operation_returns_unknown_without_replaying_overwrite() {
    let (_tmp, source_path, target_dir) = fixture();
    std::fs::write(target_dir.join("photo.jpg"), b"old target").unwrap();
    let manifest = build_transfer_manifest(&[source_path]).unwrap();
    let source = build_peer(83, [83; 32], vec![peer_entry(84)], None).await;
    let target = build_peer(84, [83; 32], vec![peer_entry(83)], None).await;
    install_connectable_peer(&source, &target);
    target
        .runtime
        .session_manager()
        .set_bulk_transfer_handler(Arc::new(SyncTransferBulkHandler::new(
            target.device_id.clone(),
        )));
    let (_cancel, rx) = tokio::sync::watch::channel(RunControl::Running);
    let (done_tx, mut done_rx) = tokio::sync::mpsc::unbounded_channel();
    let barrier = Arc::new(Barrier::new(2));
    let barrier_callback = Arc::clone(&barrier);
    let manager = source.runtime.session_manager().clone();
    let target_id = target.device_id.clone();
    let source_id = source.device_id.clone();
    let target_path = target_dir.join("photo.jpg");
    let task = tokio::spawn(async move {
        let mut notified = false;
        send_transfer_stream(
            &manager,
            TransferStreamSendRequest {
                chat_context: None,
                operation_id: "lost-committed-receipt".into(),
                source_device_id: source_id,
                target_device_id: target_id,
                manifest,
                target_dir,
                conflict_policy: ReceiveConflictPolicy::Overwrite,
                options: TransferExecutionOptions { chunk_size: 64 },
            },
            rx,
            |progress| {
                if progress.bytes_transferred == progress.total_bytes && !notified {
                    notified = true;
                    done_tx.send(()).unwrap();
                    barrier_callback.wait();
                }
            },
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(10), done_rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(std::fs::read(&target_path).unwrap(), b"original photo");
    std::fs::write(&target_path, b"subsequent user edit").unwrap();
    target
        .runtime
        .session_manager()
        .set_bulk_transfer_handler(Arc::new(SyncTransferBulkHandler::new(
            target.device_id.clone(),
        )));
    source
        .runtime
        .session_manager()
        .disconnect_peer(&target.device_id)
        .await
        .unwrap();
    barrier.wait();
    let error = task.await.unwrap().unwrap_err();
    assert!(
        matches!(error, SyncTransferPlanError::OutcomeUnknown { .. }),
        "{error}"
    );
    assert_eq!(std::fs::read(target_path).unwrap(), b"subsequent user edit");
    source.runtime.shutdown().await.unwrap();
    target.runtime.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn retry_of_completed_operation_returns_receipt_without_another_overwrite() {
    let (_tmp, source_path, target_dir) = fixture();
    std::fs::write(target_dir.join("photo.jpg"), b"old target").unwrap();
    let manifest = build_transfer_manifest(&[source_path]).unwrap();
    let source = build_peer(85, [85; 32], vec![peer_entry(86)], None).await;
    let target = build_peer(86, [85; 32], vec![peer_entry(85)], None).await;
    install_connectable_peer(&source, &target);
    target
        .runtime
        .session_manager()
        .set_bulk_transfer_handler(Arc::new(SyncTransferBulkHandler::new(
            target.device_id.clone(),
        )));
    let request = || TransferStreamSendRequest {
        operation_id: "completed-receipt".into(),
        source_device_id: source.device_id.clone(),
        target_device_id: target.device_id.clone(),
        manifest: manifest.clone(),
        target_dir: target_dir.clone(),
        conflict_policy: ReceiveConflictPolicy::Overwrite,
        options: TransferExecutionOptions::default(),
        chat_context: None,
    };
    let (_cancel, rx) = tokio::sync::watch::channel(RunControl::Running);
    let first = send_transfer_stream(
        source.runtime.session_manager(),
        request(),
        rx.clone(),
        |_| {},
    )
    .await
    .unwrap();
    std::fs::write(target_dir.join("photo.jpg"), b"later user edit").unwrap();
    let repeated = send_transfer_stream(source.runtime.session_manager(), request(), rx, |_| {})
        .await
        .unwrap();
    assert_eq!(first, repeated);
    assert_eq!(
        std::fs::read(target_dir.join("photo.jpg")).unwrap(),
        b"later user edit"
    );
    source.runtime.shutdown().await.unwrap();
    target.runtime.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resume_reports_committed_empty_file_before_later_failure() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    async fn frame(
        stream: &mut DeviceSessionBulkStream,
        request: serde_json::Value,
    ) -> serde_json::Value {
        let bytes = serde_json::to_vec(&request).unwrap();
        stream
            .send_mut()
            .write_u32(bytes.len() as u32)
            .await
            .unwrap();
        stream.send_mut().write_all(&bytes).await.unwrap();
        let len = stream.recv_mut().read_u32().await.unwrap();
        let mut bytes = vec![0; len as usize];
        stream.recv_mut().read_exact(&mut bytes).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }
    let (tmp, _source_path, target_dir) = fixture();
    let selected = tmp.path().join("selected");
    std::fs::create_dir(&selected).unwrap();
    std::fs::write(selected.join("a-empty.jpg"), []).unwrap();
    std::fs::write(selected.join("b.jpg"), b"second").unwrap();
    let manifest = build_transfer_manifest(std::slice::from_ref(&selected)).unwrap();
    let source = build_peer(87, [87; 32], vec![peer_entry(88)], None).await;
    let target = build_peer(88, [87; 32], vec![peer_entry(87)], None).await;
    install_connectable_peer(&source, &target);
    target
        .runtime
        .session_manager()
        .set_bulk_transfer_handler(Arc::new(SyncTransferBulkHandler::new(
            target.device_id.clone(),
        )));
    let mut stream = source
        .runtime
        .session_manager()
        .open_bulk_transfer_to(&target.device_id, Duration::from_secs(5))
        .await
        .unwrap();
    frame(&mut stream, serde_json::json!({"kind":"open","header":{
        "operation_id":"empty-resume","source_device_id":source.device_id,"target_device_id":target.device_id,"target_dir":target_dir,
        "conflict_policy":"fail","entries":manifest.entries.iter().map(|entry| serde_json::json!({"relative_path":entry.relative_path,"size":entry.size,"sha256":entry.sha256})).collect::<Vec<_>>()
    }})).await;
    // Discard the original acknowledgement, as if its source process lost it.
    let ack = frame(&mut stream, serde_json::json!({"kind":"chunk","operation_id":"empty-resume","manifest_index":0,"offset":0,"len":0})).await;
    assert_eq!(ack["bytes_written"], 0);
    assert_eq!(ack["complete"], false);
    drop(stream);
    std::fs::write(selected.join("b.jpg"), b"edited").unwrap();
    let store = rua_server::services::sync_transfer::SourceTransferOperationStore::new();
    let (_, cancel) = store
        .start(
            "empty-resume".into(),
            manifest.total_bytes,
            source.device_id.clone(),
            target.device_id.clone(),
        )
        .unwrap();
    let error = send_transfer_stream(
        source.runtime.session_manager(),
        TransferStreamSendRequest {
            operation_id: "empty-resume".into(),
            source_device_id: source.device_id.clone(),
            target_device_id: target.device_id.clone(),
            manifest,
            target_dir: target_dir.clone(),
            conflict_policy: ReceiveConflictPolicy::Fail,
            options: TransferExecutionOptions::default(),
            chat_context: None,
        },
        cancel.unwrap(),
        |progress| store.update_progress_with_files("empty-resume", &progress),
    )
    .await
    .unwrap_err();
    store.fail("empty-resume", error.to_string());
    let failed = store.get("empty-resume").unwrap();
    assert_eq!(failed.files_written, 1);
    assert_eq!(failed.files[0].bytes, 0);
    assert_eq!(
        failed.files[0].target_path,
        target_dir
            .canonicalize()
            .unwrap()
            .join("selected/a-empty.jpg")
    );
    assert!(failed.files[0].target_path.exists());
    assert!(!target_dir.join("selected/b.jpg").exists());
    source.runtime.shutdown().await.unwrap();
    target.runtime.shutdown().await.unwrap();
}
