use rua_core::{ChatAddress, ChatScopeStore, RunControl};
use rua_device_network::device_list::{DeviceEntry, DeviceList};
use rua_device_network::{
    BoundDeviceNetworkMembership, DeviceNetworkBootstrap, DeviceNetworkIdentity,
    DeviceNetworkPreset, DeviceNetworkRuntime, DevicePeerDirectory, DeviceSessionBulkStream,
    DeviceSessionBulkTransferHandler, DeviceSessionCommandHandler, DeviceSessionConfig,
};
use rua_server::services::chat_media_transfer::{AuthorizedMediaUpload, ChatMediaTransferRegistry};
use rua_server::services::chat_metadata_store::apply_confirmed_authority_placement;
use rua_server::services::conversation_authority_placement::{
    ConversationAuthorityEpoch, ConversationAuthorityPlacement,
};
use rua_server::services::conversation_manifest::initial_conversation_manifest_hash;
use rua_server::services::device_command_bus::{
    deliver_chat_media_prepare_via_session, deliver_sync_transfer_device_command_via_session,
    list_device_directory, ChatMediaPrepareCommand, DeviceCommandBusError, DeviceCommandHandler,
    SyncTransferConflictPolicy, SyncTransferDeviceAction, SyncTransferDeviceCommand,
    SyncTransferDeviceEntryKind, SyncTransferDeviceResponse,
};
use rua_server::services::foreground_operation::ForegroundOperationRegistry;
use rua_server::services::sync_transfer::{
    build_gap_transfer_manifest, build_transfer_inventory, build_transfer_manifest,
    command_remote_source_send_to, execute_local_transfer_plan, open_chat_media_read_stream,
    plan_receive, read_sync_blob_via_session, send_transfer_stream, ReceiveConflictPolicy,
    SyncTransferBulkHandler, SyncTransferPlanError, TransferExecutionOptions, TransferJobStatus,
    TransferJobStore, TransferManifest, TransferManifestEntry, TransferManifestEntryKind,
    TransferStreamSendRequest,
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, Mutex, RwLock};
use std::time::Duration;
use tempfile::TempDir;

struct TestPeer {
    _dir: TempDir,
    runtime: DeviceNetworkRuntime,
    device_id: String,
    addr: iroh::EndpointAddr,
}

#[derive(Debug, Default)]
struct AlwaysDropBulkHandler {
    calls: AtomicUsize,
}

#[async_trait::async_trait]
impl DeviceSessionBulkTransferHandler for AlwaysDropBulkHandler {
    async fn handle_bulk_transfer(&self, _stream: DeviceSessionBulkStream) -> Result<(), String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err("injected transfer stream failure".to_string())
    }
}

async fn build_peer(
    secret_byte: u8,
    network_secret: [u8; 32],
    peers: Vec<DeviceEntry>,
    command_handler: Option<Arc<dyn DeviceSessionCommandHandler>>,
) -> TestPeer {
    let dir = TempDir::new().expect("tempdir");
    let sync_dir = dir.path().join("sync");
    std::fs::create_dir_all(&sync_dir).expect("sync dir");
    std::fs::write(
        sync_dir.join("device_list.json"),
        serde_json::to_vec_pretty(&DeviceList { devices: peers }).expect("serialize peers"),
    )
    .expect("write peers");
    let secret = iroh::SecretKey::from_bytes(&[secret_byte; 32]);
    let identity = DeviceNetworkIdentity::new(secret);
    let device_id = hex::encode(identity.local_device_id());
    let bootstrap = DeviceNetworkBootstrap::bind(
        DeviceNetworkPreset::Local,
        identity,
        BoundDeviceNetworkMembership::new(network_secret),
        DevicePeerDirectory::from_sync_dir(&sync_dir),
    )
    .await
    .expect("bind runtime");
    let runtime = bootstrap.spawn_with_session_config(
        Vec::new(),
        command_handler,
        DeviceSessionConfig {
            connect_timeout: Duration::from_secs(2),
            handshake_timeout: Duration::from_secs(2),
            reconnect_attempts: 1,
            ..Default::default()
        },
    );
    let addr = local_endpoint_addr(runtime.endpoint());
    TestPeer {
        _dir: dir,
        runtime,
        device_id,
        addr,
    }
}

fn peer_entry(secret_byte: u8) -> DeviceEntry {
    let secret = iroh::SecretKey::from_bytes(&[secret_byte; 32]);
    let public_key = *secret.public().as_bytes();
    let mut device_id = [0; 8];
    device_id.copy_from_slice(&public_key[..8]);
    DeviceEntry {
        device_id,
        public_key,
        label: format!("device-{secret_byte}"),
        friendly_name: format!("Device {secret_byte}"),
        added_at_ms: 1,
        last_seen_ms: 1,
        direct_addrs: Vec::new(),
    }
}

fn local_endpoint_addr(endpoint: &iroh::Endpoint) -> iroh::EndpointAddr {
    let mut addr = iroh::EndpointAddr::new(endpoint.id());
    for socket_addr in endpoint.bound_sockets() {
        let dialable = if socket_addr.ip().is_unspecified() {
            match socket_addr {
                std::net::SocketAddr::V4(addr) => std::net::SocketAddr::new(
                    std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
                    addr.port(),
                ),
                std::net::SocketAddr::V6(addr) => std::net::SocketAddr::new(
                    std::net::IpAddr::V6(std::net::Ipv6Addr::LOCALHOST),
                    addr.port(),
                ),
            }
        } else {
            socket_addr
        };
        addr.addrs.insert(iroh::TransportAddr::Ip(dialable));
    }
    addr
}

fn install_connectable_peer(source: &TestPeer, target: &TestPeer) {
    let path = source._dir.path().join("sync/device_list.json");
    let mut device_list = std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<DeviceList>(&bytes).ok())
        .unwrap_or_default();
    let public_key = *target.addr.id.as_bytes();
    let mut device_id = [0; 8];
    device_id.copy_from_slice(&public_key[..8]);
    device_list
        .devices
        .retain(|entry| entry.device_id != device_id);
    device_list.add_device(DeviceEntry {
        device_id,
        public_key,
        label: target.device_id.clone(),
        friendly_name: target.device_id.clone(),
        added_at_ms: 1,
        last_seen_ms: 1,
        direct_addrs: target
            .addr
            .addrs
            .iter()
            .filter_map(|addr| match addr {
                iroh::TransportAddr::Ip(addr) => Some(addr.to_string()),
                _ => None,
            })
            .collect(),
    });
    std::fs::write(
        path,
        serde_json::to_vec_pretty(&device_list).expect("serialize connectable peer"),
    )
    .expect("write connectable peer");
}

fn device_id(secret_byte: u8) -> String {
    let secret = iroh::SecretKey::from_bytes(&[secret_byte; 32]);
    hex::encode(&secret.public().as_bytes()[..8])
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sync_blob_read_uses_the_shared_device_session() {
    let network_secret = [74; 32];
    let consumer = build_peer(74, network_secret, vec![peer_entry(75)], None).await;
    let provider = build_peer(75, network_secret, vec![peer_entry(74)], None).await;
    install_connectable_peer(&consumer, &provider);

    let blob_dir = provider._dir.path().join("sync-blobs");
    let blob_store = rua_sync::sync_blob_store::SyncBlobStore::open(
        &blob_dir,
        rua_sync::sync_blob_store::SyncBlobFetchConfig::default(),
    )
    .await
    .expect("open provider blob store");
    let api_store = blob_store.api_store();
    let expected = b"encrypted replica-log payload";
    let hash = rua_sync::opaque_replica_log::store_encrypted_blob_bytes(
        &api_store,
        bytes::Bytes::copy_from_slice(expected),
    )
    .await
    .expect("store provider Sync Blob");
    provider
        .runtime
        .session_manager()
        .set_bulk_transfer_handler(Arc::new(
            SyncTransferBulkHandler::new(provider.device_id.clone()).with_sync_blobs(api_store),
        ));

    let actual = read_sync_blob_via_session(
        consumer.runtime.session_manager(),
        &provider.device_id,
        hash,
    )
    .await
    .expect("read Sync Blob over shared Device Session");

    assert_eq!(actual, expected);
    let diagnostics = consumer.runtime.session_manager().diagnostics();
    assert_eq!(diagnostics.physical_dial_attempts, 1);
    assert_eq!(diagnostics.sessions.len(), 1);

    consumer
        .runtime
        .shutdown()
        .await
        .expect("consumer shutdown");
    provider
        .runtime
        .shutdown()
        .await
        .expect("provider shutdown");
}

#[test]
fn list_device_directory_returns_sorted_files_and_directories() {
    let tmp = TempDir::new().expect("tempdir");
    let root = tmp.path().join("files");
    std::fs::create_dir_all(root.join("Design")).expect("create dir");
    std::fs::write(root.join("report.pdf"), b"report").expect("write file");

    let listing = list_device_directory(&root).expect("list directory");

    assert_eq!(listing.path, root.canonicalize().expect("canonical root"));
    let entries = listing
        .entries
        .iter()
        .map(|entry| (entry.name.as_str(), entry.kind, entry.size))
        .collect::<Vec<_>>();
    assert_eq!(
        entries,
        vec![
            ("Design", SyncTransferDeviceEntryKind::Directory, None),
            ("report.pdf", SyncTransferDeviceEntryKind::File, Some(6)),
        ]
    );
}

#[test]
fn chat_gap_manifest_skips_identical_files_and_keeps_changed_or_missing_files() {
    let tmp = TempDir::new().expect("tempdir");
    let source_chat = tmp.path().join("source/chat-1");
    let target_chat = tmp.path().join("target/chat-1");
    std::fs::create_dir_all(source_chat.join("messages")).expect("source messages");
    std::fs::create_dir_all(target_chat.join("messages")).expect("target messages");
    std::fs::write(source_chat.join("session.json"), b"same").expect("source session");
    std::fs::write(target_chat.join("session.json"), b"same").expect("target session");
    std::fs::write(source_chat.join("turn_index.json"), b"new!").expect("source turn index");
    std::fs::write(target_chat.join("turn_index.json"), b"old!").expect("target turn index");
    std::fs::write(source_chat.join("messages/000000000000.jsonl"), b"missing")
        .expect("source message chunk");

    let target_inventory = build_transfer_inventory(&[target_chat]).expect("target inventory");
    let gap = build_gap_transfer_manifest(&[source_chat], &target_inventory).expect("gap manifest");
    let paths = gap
        .entries
        .iter()
        .map(|entry| entry.relative_path.to_string_lossy().into_owned())
        .collect::<Vec<_>>();

    assert_eq!(
        paths,
        vec![
            "chat-1/messages/000000000000.jsonl",
            "chat-1/turn_index.json"
        ]
    );
    assert_eq!(gap.total_bytes, 11);
}

#[test]
fn list_device_directory_rejects_missing_path() {
    let tmp = TempDir::new().expect("tempdir");

    let error = list_device_directory(tmp.path().join("missing")).expect_err("missing path");

    assert!(
        error.to_string().contains("No such file")
            || error.to_string().contains("not found")
            || error.to_string().contains("os error"),
        "unexpected missing path error: {error}"
    );
}

#[test]
fn sync_transfer_list_directory_command_rejects_empty_path() {
    let command = SyncTransferDeviceCommand {
        command_id: "sync-transfer-empty-path".to_string(),
        source_device_id: "source-device".to_string(),
        target_device_id: "target-device".to_string(),
        action: SyncTransferDeviceAction::ListDir {
            path: "  ".to_string(),
        },
    };

    let error = command
        .validate_for_target("target-device")
        .expect_err("empty path should reject");

    assert!(matches!(error, DeviceCommandBusError::InvalidId(_)));
}

#[test]
fn manifest_expands_files_and_directories_with_relative_paths() {
    let tmp = TempDir::new().expect("tempdir");
    let root = tmp.path().join("src");
    std::fs::create_dir_all(root.join("Design/assets")).expect("dirs");
    std::fs::write(root.join("report.pdf"), b"report").expect("report");
    std::fs::write(root.join("Design/assets/logo.png"), b"png").expect("logo");

    let manifest =
        build_transfer_manifest(&[root.join("report.pdf"), root.join("Design")]).expect("manifest");
    let entries = manifest
        .entries
        .iter()
        .map(|entry| {
            (
                entry.relative_path.to_string_lossy().to_string(),
                entry.kind,
                entry.size,
            )
        })
        .collect::<Vec<_>>();

    assert_eq!(
        entries,
        vec![
            (
                "Design/assets/logo.png".to_string(),
                TransferManifestEntryKind::File,
                3,
            ),
            ("report.pdf".to_string(), TransferManifestEntryKind::File, 6,),
        ]
    );
    assert_eq!(manifest.total_bytes, 9);
}

#[test]
fn manifest_rejects_missing_paths() {
    let tmp = TempDir::new().expect("tempdir");

    let error =
        build_transfer_manifest(&[tmp.path().join("missing.txt")]).expect_err("missing path");

    assert!(error.to_string().contains("missing"));
}

#[test]
fn failed_transfer_job_is_explicit_and_new_start_gets_a_new_operation() {
    let jobs = TransferJobStore::new();
    let (first, _first_cancel_rx) = jobs.start(
        "source".to_string(),
        "target".to_string(),
        PathBuf::from("/target"),
        10,
    );
    jobs.fail(&first.job_id, "connection lost".to_string());
    let failed = jobs.get(&first.job_id).expect("failed job");
    assert_eq!(failed.status, TransferJobStatus::Failed);
    assert_eq!(failed.error.as_deref(), Some("connection lost"));

    let (second, _second_cancel_rx) = jobs.start(
        "source".to_string(),
        "target".to_string(),
        PathBuf::from("/target"),
        10,
    );
    assert_ne!(second.job_id, first.job_id);
    assert_eq!(second.status, TransferJobStatus::Running);

    jobs.cancel(&second.job_id).expect("cancel second job");
    jobs.fail(&second.job_id, "cancelled worker returned".to_string());
    assert_eq!(
        jobs.get(&second.job_id).expect("cancelled job").status,
        TransferJobStatus::Cancelled
    );
}

#[cfg(unix)]
#[test]
fn manifest_rejects_symlink_sources() {
    let tmp = TempDir::new().expect("tempdir");
    let target = tmp.path().join("target.txt");
    let link = tmp.path().join("link.txt");
    std::fs::write(&target, b"target").expect("target");
    std::os::unix::fs::symlink(&target, &link).expect("symlink");

    let error = build_transfer_manifest(&[link]).expect_err("symlink rejected");

    assert!(error.to_string().contains("unsupported source path type"));
}

#[test]
fn receive_plan_renames_conflicting_files_by_default() {
    let tmp = TempDir::new().expect("tempdir");
    let target = tmp.path().join("target");
    std::fs::create_dir_all(&target).expect("target dir");
    std::fs::write(target.join("report.pdf"), b"old").expect("old report");
    let source = tmp.path().join("report.pdf");
    std::fs::write(&source, b"new").expect("source");
    let manifest = build_transfer_manifest(&[source]).expect("manifest");

    let plan = plan_receive(&target, &manifest, ReceiveConflictPolicy::Rename).expect("plan");

    assert_eq!(plan.entries.len(), 1);
    assert_eq!(
        plan.entries[0].target_path.parent(),
        Some(target.canonicalize().expect("canonical target").as_path())
    );
    assert_eq!(
        plan.entries[0].target_path.file_name().unwrap(),
        "report (1).pdf"
    );
}

#[test]
fn receive_plan_can_fail_on_conflict() {
    let tmp = TempDir::new().expect("tempdir");
    let target = tmp.path().join("target");
    std::fs::create_dir_all(&target).expect("target dir");
    std::fs::write(target.join("report.pdf"), b"old").expect("old report");
    let source = tmp.path().join("report.pdf");
    std::fs::write(&source, b"new").expect("source");
    let manifest = build_transfer_manifest(&[source]).expect("manifest");

    let error = plan_receive(&target, &manifest, ReceiveConflictPolicy::Fail)
        .expect_err("conflict should fail");

    assert!(error.to_string().contains("report.pdf"));
}

#[test]
fn receive_plan_rejects_relative_path_traversal() {
    let tmp = TempDir::new().expect("tempdir");
    let target = tmp.path().join("target");
    std::fs::create_dir_all(&target).expect("target dir");
    let source = tmp.path().join("source.txt");
    std::fs::write(&source, b"source").expect("source");
    let manifest = TransferManifest {
        entries: vec![TransferManifestEntry {
            source_path: source,
            relative_path: PathBuf::from("../escape.txt"),
            kind: TransferManifestEntryKind::File,
            size: 6,
        }],
        total_bytes: 6,
    };

    let error = plan_receive(&target, &manifest, ReceiveConflictPolicy::Rename)
        .expect_err("traversal should reject");

    assert!(error.to_string().contains("invalid relative transfer path"));
}

#[tokio::test]
async fn local_transfer_plan_writes_files_atomically_and_reports_progress() {
    let tmp = TempDir::new().expect("tempdir");
    let source_root = tmp.path().join("src");
    let target = tmp.path().join("target");
    std::fs::create_dir_all(&source_root).expect("source dir");
    std::fs::create_dir_all(&target).expect("target dir");
    std::fs::write(source_root.join("a.txt"), b"abcdef").expect("a");
    std::fs::write(source_root.join("b.txt"), b"ghij").expect("b");
    let manifest = build_transfer_manifest(&[source_root]).expect("manifest");
    let plan = plan_receive(&target, &manifest, ReceiveConflictPolicy::Rename).expect("plan");
    let (_cancel_tx, cancel_rx) = tokio::sync::watch::channel(RunControl::Running);
    let progress = Arc::new(Mutex::new(Vec::new()));
    let progress_for_callback = Arc::clone(&progress);

    let outcome = execute_local_transfer_plan(
        &manifest,
        &plan,
        TransferExecutionOptions { chunk_size: 2 },
        cancel_rx,
        move |event| {
            progress_for_callback
                .lock()
                .expect("progress lock")
                .push((event.bytes_transferred, event.total_bytes));
        },
    )
    .await
    .expect("execute transfer");

    assert_eq!(outcome.files_written, 2);
    assert_eq!(outcome.bytes_written, 10);
    assert_eq!(
        std::fs::read(target.join("src/a.txt")).expect("read a"),
        b"abcdef"
    );
    assert_eq!(
        std::fs::read(target.join("src/b.txt")).expect("read b"),
        b"ghij"
    );
    assert!(
        progress
            .lock()
            .expect("progress lock")
            .iter()
            .any(|(_, total)| *total == 10),
        "progress should report total bytes"
    );
    assert!(
        std::fs::read_dir(target.join("src"))
            .expect("read target src")
            .all(|entry| !entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .contains("rua-transfer-tmp")),
        "temp files should not remain after successful transfer"
    );
}

#[tokio::test]
async fn local_transfer_plan_cancellation_removes_partial_temp_file() {
    let tmp = TempDir::new().expect("tempdir");
    let target = tmp.path().join("target");
    std::fs::create_dir_all(&target).expect("target dir");
    let source = tmp.path().join("large.bin");
    std::fs::write(&source, vec![7u8; 16]).expect("source");
    let manifest = build_transfer_manifest(&[source]).expect("manifest");
    let plan = plan_receive(&target, &manifest, ReceiveConflictPolicy::Rename).expect("plan");
    let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(RunControl::Running);
    let cancel_tx = Arc::new(Mutex::new(Some(cancel_tx)));

    let error = execute_local_transfer_plan(
        &manifest,
        &plan,
        TransferExecutionOptions { chunk_size: 4 },
        cancel_rx,
        move |_event| {
            if let Some(tx) = cancel_tx.lock().expect("cancel lock").take() {
                let _ = tx.send(RunControl::Stop);
            }
        },
    )
    .await
    .expect_err("cancelled transfer");

    assert!(error.to_string().contains("cancelled"));
    assert!(
        !target.join("large.bin").exists(),
        "cancelled transfer must not expose final target file"
    );
    assert!(
        std::fs::read_dir(&target)
            .expect("read target")
            .next()
            .is_none(),
        "cancelled transfer should clean partial temp files"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bulk_transfer_sends_manifest_files_over_one_shared_session() {
    let tmp = TempDir::new().expect("tempdir");
    let source_root = tmp.path().join("src");
    let target = tmp.path().join("target");
    std::fs::create_dir_all(&source_root).expect("source dir");
    std::fs::create_dir_all(&target).expect("target dir");
    std::fs::write(source_root.join("a.txt"), b"abcdef").expect("a");
    std::fs::write(source_root.join("b.txt"), b"ghij").expect("b");
    let manifest = build_transfer_manifest(&[source_root]).expect("manifest");
    let (_cancel_tx, cancel_rx) = tokio::sync::watch::channel(RunControl::Running);

    let network_secret = [31; 32];
    let source = build_peer(31, network_secret, vec![peer_entry(32)], None).await;
    let target_handler: Arc<dyn DeviceSessionCommandHandler> = Arc::new(DeviceCommandHandler::new(
        tmp.path().join("target-commands"),
        device_id(32),
    ));
    let target_peer = build_peer(
        32,
        network_secret,
        vec![peer_entry(31)],
        Some(target_handler),
    )
    .await;
    install_connectable_peer(&source, &target_peer);
    let foreground_operations = Arc::new(ForegroundOperationRegistry::new(true));
    target_peer
        .runtime
        .session_manager()
        .set_bulk_transfer_handler(Arc::new(
            SyncTransferBulkHandler::new(target_peer.device_id.clone())
                .with_foreground_operations(Arc::clone(&foreground_operations)),
        ));
    let listing = deliver_sync_transfer_device_command_via_session(
        source.runtime.session_manager(),
        &SyncTransferDeviceCommand {
            command_id: "list-before-transfer".to_string(),
            source_device_id: source.device_id.clone(),
            target_device_id: target_peer.device_id.clone(),
            action: SyncTransferDeviceAction::ListDir {
                path: target.to_string_lossy().to_string(),
            },
        },
        Duration::from_secs(5),
    )
    .await
    .expect("directory listing over shared session");
    assert!(matches!(
        listing,
        SyncTransferDeviceResponse::DirEntries { .. }
    ));
    let generation = source.runtime.session_manager().diagnostics().sessions[0]
        .generation
        .expect("Command session generation");

    let observed_receive_lease = Arc::new(AtomicBool::new(false));
    let outcome = send_transfer_stream(
        source.runtime.session_manager(),
        TransferStreamSendRequest {
            operation_id: "direct-transfer".to_string(),
            source_device_id: source.device_id.clone(),
            target_device_id: target_peer.device_id.clone(),
            manifest,
            target_dir: target.clone(),
            conflict_policy: ReceiveConflictPolicy::Rename,
            options: TransferExecutionOptions { chunk_size: 2 },
        },
        cancel_rx,
        {
            let foreground_operations = Arc::clone(&foreground_operations);
            let observed_receive_lease = Arc::clone(&observed_receive_lease);
            move |_| {
                observed_receive_lease.store(foreground_operations.has_active(), Ordering::SeqCst);
            }
        },
    )
    .await
    .expect("stream transfer");

    assert_eq!(outcome.files_written, 2);
    assert_eq!(outcome.bytes_written, 10);
    assert!(observed_receive_lease.load(Ordering::SeqCst));
    assert!(!foreground_operations.has_active());
    assert_eq!(
        std::fs::read(target.join("src/a.txt")).expect("read a"),
        b"abcdef"
    );
    assert_eq!(
        std::fs::read(target.join("src/b.txt")).expect("read b"),
        b"ghij"
    );
    let diagnostics = source.runtime.session_manager().diagnostics();
    assert_eq!(diagnostics.physical_dial_attempts, 1);
    assert_eq!(diagnostics.sessions.len(), 1);
    assert_eq!(diagnostics.sessions[0].generation, Some(generation));
    assert_eq!(diagnostics.sessions[0].active_logical_streams, 0);

    source.runtime.shutdown().await.expect("source shutdown");
    target_peer
        .runtime
        .shutdown()
        .await
        .expect("target shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_chat_media_upload_and_read_use_authority_workspace() {
    media_upload_over_device_session(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn first_submission_media_transfer_does_not_materialize_chat() {
    media_upload_over_device_session(true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prepared_first_input_transfers_staged_media_before_target_acceptance() {
    let tmp = TempDir::new().unwrap();
    let source_root = tmp.path().join("source");
    let target_root = tmp.path().join("target");
    let state =
        rua_server::AppState::from_tempdir(target_root.clone(), rua_server::ServerProfile::Test);
    state.wait_platform_initialization().await.unwrap();
    let identity: rua_sync::identity::DeviceIdentity =
        serde_json::from_value(serde_json::json!({"secret_bytes": vec![72_u8;32]})).unwrap();
    *state.sync_device.write().unwrap() = Some(Arc::new(identity));
    let registry = Arc::new(ChatMediaTransferRegistry::default());
    let handler = Arc::new(
        DeviceCommandHandler::new(target_root.join("device_commands"), device_id(72))
            .with_acp_chat_registry(
                Arc::clone(&state.acp_chat_registry),
                Arc::clone(&state.registry),
            )
            .with_conversation_input_intake(state.conversation_input_intake.clone())
            .with_conversation_authority(Arc::clone(&state.conversation_authority))
            .with_remote_chat_control_streams(Arc::clone(&state.streams))
            .with_chat_media_transfers(Arc::clone(&registry)),
    );
    let target = build_peer(72, [71; 32], vec![peer_entry(71)], Some(handler)).await;
    let bulk: Arc<dyn DeviceSessionBulkTransferHandler> = Arc::new(
        SyncTransferBulkHandler::new(target.device_id.clone())
            .with_media_transfers(registry)
            .with_chat_media_root(target_root.clone()),
    );
    target
        .runtime
        .session_manager()
        .set_bulk_transfer_handler(bulk);
    let source = build_peer(71, [71; 32], vec![peer_entry(72)], None).await;
    install_connectable_peer(&source, &target);
    let address = ChatAddress::agent("default", "00000000-0000-4000-8000-000000000115");
    let source_scope = ChatScopeStore::resolve(&source_root, address.clone());
    let media = rua_server::services::chat_media::ChatMediaService
        .store(
            &source_root,
            address.clone(),
            b"first input bytes".to_vec(),
            Some("text/plain".into()),
            None,
        )
        .await
        .unwrap();
    let source_device = source.device_id.clone();
    let target_device = target.device_id.clone();
    let delivery =
        rua_server::services::remote_conversation_input::RemoteConversationInputDelivery::new(
            Arc::new(tokio::sync::Mutex::new(None)),
            Arc::new(rua_server::services::device_session_access::DeviceNetworkAccess::new(Some(source.runtime), Arc::new(rua_server::services::device_network_availability::DeviceNetworkAvailability::new()))),
            source_root,
        );
    let input = rua_channel::core::console::ConsoleChatRequest {
        message: "inspect staged media".into(),
        chat_id: Some(address.chat_id().to_string()),
        model: None,
        is_reconnect: false,
        attachments: vec![],
        input: Some(vec![
            serde_json::json!({"role":"user","content":[{"type":"file","media_ref":media.media_ref} ]}),
        ]),
        messages: None,
        extra: Default::default(),
        channel_kind: None,
    };
    let request = delivery
        .build_remote_conversation_input_request(
            &rua_server::services::remote_conversation_input::RemoteConversationInputParams {
                address: &address,
                execution_agent_id: "default",
                request: &input,
                source_device_id: &source_device,
                target_device_id: &target_device,
                authority_epoch: ConversationAuthorityEpoch::next_for_device(&target_device, None),
                base_manifest_hash: &initial_conversation_manifest_hash(&address).unwrap(),
                workspace_dir: Some(source_scope.root()),
                materialize_conversation: true,
            },
        )
        .await
        .unwrap();
    let outcome = delivery
        .deliver_prepared_conversation_input(&request, "first staged input test")
        .await
        .unwrap();
    assert!(outcome.accepted);
    let repeated = delivery
        .deliver_prepared_conversation_input(&request, "repeat first staged input test")
        .await
        .unwrap();
    assert!(repeated.accepted);
    assert_eq!(repeated.input_id, outcome.input_id);
    assert!(rua_session::WorkspaceSessionStore::new(source_scope.root())
        .load_chat_header(address.chat_id().as_str())
        .await
        .unwrap()
        .is_none());
    let target_scope = ChatScopeStore::resolve(&target_root, address.clone());
    assert_eq!(
        std::fs::read(target_scope.root().join("media").join(&media.media_id)).unwrap(),
        b"first input bytes"
    );
    let header = rua_session::WorkspaceSessionStore::new(target_scope.root())
        .load_chat_header(address.chat_id().as_str())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(header.turn_index.latest_turn, 1);
    assert!(!target_scope
        .root()
        .join("media")
        .join(format!(".staged-{}.json", media.media_id))
        .exists());
}

async fn media_upload_over_device_session(first_submission: bool) {
    let tmp = TempDir::new().expect("tempdir");
    let authority_root = tmp.path().join("authority-rua-root");
    let source_root = tmp.path().join("controller-temp");
    std::fs::create_dir_all(&source_root).expect("source dir");

    let network_secret = [71; 32];
    let controller = build_peer(71, network_secret, vec![peer_entry(72)], None).await;
    let registry = Arc::new(ChatMediaTransferRegistry::default());
    let authority_handler: Arc<dyn DeviceSessionCommandHandler> = Arc::new(
        DeviceCommandHandler::new(authority_root.join("device_commands"), device_id(72))
            .with_chat_media_transfers(Arc::clone(&registry)),
    );
    let authority = build_peer(
        72,
        network_secret,
        vec![peer_entry(71)],
        Some(authority_handler),
    )
    .await;
    install_connectable_peer(&controller, &authority);
    authority
        .runtime
        .session_manager()
        .set_bulk_transfer_handler(Arc::new(
            SyncTransferBulkHandler::new(authority.device_id.clone())
                .with_media_transfers(registry)
                .with_chat_media_root(authority_root.clone()),
        ));

    let address = ChatAddress::agent("default", "00000000-0000-4000-8000-000000000113");
    let scope = ChatScopeStore::resolve(&authority_root, address.clone());
    let placement = ConversationAuthorityPlacement {
        authority_epoch: ConversationAuthorityEpoch::next_for_device(&authority.device_id, None),
        device_id: authority.device_id.clone(),
        base_manifest_hash: initial_conversation_manifest_hash(&address).expect("manifest hash"),
    };
    if first_submission {
        std::fs::create_dir_all(scope.root()).unwrap();
        rua_config::atomic_write_json(&authority_root.join("config.json"), &serde_json::json!({"agents":{"active_agent":"default","profiles":{"default":{"id":"default","name":"Default","workspace_dir":scope.root()}}}})).unwrap();
        rua_config::atomic_write_json(
            &scope.root().join("agent.json"),
            &serde_json::json!({"id":"default","name":"Default","workspace_dir":scope.root()}),
        )
        .unwrap();
    } else {
        apply_confirmed_authority_placement(scope.root(), &address, placement.clone())
            .await
            .expect("materialize placement");
    }

    let media_id = uuid::Uuid::new_v4().to_string();
    let operation_id = format!("chat-media-{}", uuid::Uuid::new_v4());
    let payload = b"remote authority media".to_vec();
    let source_path = source_root.join(&media_id);
    std::fs::write(&source_path, &payload).expect("source media");

    let (_unauthorized_cancel_tx, unauthorized_cancel_rx) =
        tokio::sync::watch::channel(RunControl::Running);
    let unauthorized = send_transfer_stream(
        controller.runtime.session_manager(),
        TransferStreamSendRequest {
            operation_id: format!("chat-media-{}", uuid::Uuid::new_v4()),
            source_device_id: controller.device_id.clone(),
            target_device_id: authority.device_id.clone(),
            manifest: TransferManifest {
                entries: vec![TransferManifestEntry {
                    source_path: source_path.clone(),
                    relative_path: PathBuf::from(&media_id),
                    kind: TransferManifestEntryKind::File,
                    size: payload.len() as u64,
                }],
                total_bytes: payload.len() as u64,
            },
            target_dir: tmp.path().join("unauthorized-target"),
            conflict_policy: ReceiveConflictPolicy::Fail,
            options: TransferExecutionOptions { chunk_size: 4 },
        },
        unauthorized_cancel_rx,
        |_| {},
    )
    .await;
    assert!(
        unauthorized.is_err(),
        "Media Bulk stream requires prepare authorization"
    );
    assert!(!tmp.path().join("unauthorized-target").exists());

    let stale_epoch = ConversationAuthorityEpoch::next_for_device(
        &authority.device_id,
        Some(placement.authority_epoch),
    );
    let stale = deliver_chat_media_prepare_via_session(
        controller.runtime.session_manager(),
        &ChatMediaPrepareCommand {
            sha256: None,
            first_submission: false,
            command_id: uuid::Uuid::new_v4().to_string(),
            operation_id: format!("chat-media-{}", uuid::Uuid::new_v4()),
            address: address.clone(),
            source_device_id: controller.device_id.clone(),
            target_device_id: authority.device_id.clone(),
            authority_epoch: stale_epoch,
            media_id: media_id.clone(),
            byte_size: payload.len() as u64,
            mime_type: "image/png".to_string(),
        },
        Duration::from_secs(5),
    )
    .await;
    assert!(stale.is_err(), "stale Authority Epoch must be rejected");

    deliver_chat_media_prepare_via_session(
        controller.runtime.session_manager(),
        &ChatMediaPrepareCommand {
            sha256: None,
            first_submission,
            command_id: uuid::Uuid::new_v4().to_string(),
            operation_id: operation_id.clone(),
            address: address.clone(),
            source_device_id: controller.device_id.clone(),
            target_device_id: authority.device_id.clone(),
            authority_epoch: placement.authority_epoch,
            media_id: media_id.clone(),
            byte_size: payload.len() as u64,
            mime_type: "image/png".to_string(),
        },
        Duration::from_secs(5),
    )
    .await
    .expect("prepare Media upload");
    let (_cancel_tx, cancel_rx) = tokio::sync::watch::channel(RunControl::Running);
    send_transfer_stream(
        controller.runtime.session_manager(),
        TransferStreamSendRequest {
            operation_id,
            source_device_id: controller.device_id.clone(),
            target_device_id: authority.device_id.clone(),
            manifest: TransferManifest {
                entries: vec![TransferManifestEntry {
                    source_path,
                    relative_path: PathBuf::from(&media_id),
                    kind: TransferManifestEntryKind::File,
                    size: payload.len() as u64,
                }],
                total_bytes: payload.len() as u64,
            },
            target_dir: tmp.path().join("must-not-be-used"),
            conflict_policy: ReceiveConflictPolicy::Fail,
            options: TransferExecutionOptions { chunk_size: 4 },
        },
        cancel_rx,
        |_| {},
    )
    .await
    .expect("upload Media");

    let authority_media = authority_root
        .join("workspaces/default/media")
        .join(&media_id);
    assert_eq!(
        std::fs::read(&authority_media).expect("Authority Media"),
        payload
    );
    assert!(!tmp.path().join("must-not-be-used").exists());
    assert!(authority_media
        .parent()
        .expect("media dir")
        .join(format!(".{media_id}.media.json"))
        .is_file());

    if first_submission {
        assert!(rua_session::WorkspaceSessionStore::new(scope.root())
            .load_chat_header(address.chat_id().as_str())
            .await
            .unwrap()
            .is_none());
        assert!(rua_server::services::conversation_authority_target::load_optional_chat_authority_placement(scope.root(), address.chat_id().as_str()).await.unwrap().is_none());
        return;
    }
    let (mime_type, byte_size, mut reader) = open_chat_media_read_stream(
        controller.runtime.session_manager(),
        &authority.device_id,
        address,
        placement.authority_epoch,
        media_id,
    )
    .await
    .expect("open remote Media read");
    let mut downloaded = Vec::new();
    tokio::io::AsyncReadExt::read_to_end(&mut reader, &mut downloaded)
        .await
        .expect("read remote Media");
    assert_eq!(mime_type, "image/png");
    assert_eq!(byte_size, payload.len() as u64);
    assert_eq!(downloaded, payload);
}

#[test]
fn expired_chat_media_authorization_cannot_open_a_bulk_transfer() {
    let tmp = TempDir::new().expect("tempdir");
    let registry = ChatMediaTransferRegistry::default();
    let media_id = "00000000-0000-4000-8000-000000000401";
    registry
        .authorize_upload(AuthorizedMediaUpload {
            operation_id: "chat-media-expired".to_string(),
            source_device_id: "source".to_string(),
            target_device_id: "target".to_string(),
            media_dir: tmp.path().join("media"),
            media_id: media_id.to_string(),
            byte_size: 4,
            mime_type: "text/plain".to_string(),
            authorized_at: std::time::Instant::now()
                - ChatMediaTransferRegistry::AUTHORIZATION_TTL
                - Duration::from_secs(1),
        })
        .expect("authorize expired fixture");
    let error = registry
        .validate_upload(
            "chat-media-expired",
            "source",
            "target",
            std::path::Path::new(media_id),
            4,
        )
        .expect_err("expired authorization must fail");
    assert!(error.contains("expired"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelled_bulk_transfer_removes_partial_temp_and_never_exposes_final_file() {
    let tmp = TempDir::new().expect("tempdir");
    let source_root = tmp.path().join("cancel-source");
    let target_root = tmp.path().join("cancel-target");
    std::fs::create_dir_all(&source_root).expect("source root");
    std::fs::create_dir_all(&target_root).expect("target root");
    std::fs::write(source_root.join("large.bin"), vec![9u8; 512 * 1024]).expect("source");
    let manifest = build_transfer_manifest(&[source_root]).expect("manifest");

    let network_secret = [38; 32];
    let source = build_peer(38, network_secret, vec![peer_entry(39)], None).await;
    let target = build_peer(39, network_secret, vec![peer_entry(38)], None).await;
    install_connectable_peer(&source, &target);
    let foreground_operations = Arc::new(ForegroundOperationRegistry::new(true));
    target
        .runtime
        .session_manager()
        .set_bulk_transfer_handler(Arc::new(
            SyncTransferBulkHandler::new(target.device_id.clone())
                .with_foreground_operations(Arc::clone(&foreground_operations)),
        ));
    let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(RunControl::Running);
    let cancel_once = Arc::new(AtomicBool::new(false));
    let observed_receive_lease = Arc::new(AtomicBool::new(false));
    let error = send_transfer_stream(
        source.runtime.session_manager(),
        TransferStreamSendRequest {
            operation_id: "cancel-transfer".to_string(),
            source_device_id: source.device_id.clone(),
            target_device_id: target.device_id.clone(),
            manifest,
            target_dir: target_root.clone(),
            conflict_policy: ReceiveConflictPolicy::Rename,
            options: TransferExecutionOptions {
                chunk_size: 32 * 1024,
            },
        },
        cancel_rx,
        {
            let cancel_once = Arc::clone(&cancel_once);
            let foreground_operations = Arc::clone(&foreground_operations);
            let observed_receive_lease = Arc::clone(&observed_receive_lease);
            move |progress| {
                observed_receive_lease.store(foreground_operations.has_active(), Ordering::SeqCst);
                if progress.bytes_transferred > 0 && !cancel_once.swap(true, Ordering::SeqCst) {
                    let _ = cancel_tx.send(RunControl::Stop);
                }
            }
        },
    )
    .await
    .expect_err("cancelled transfer");
    assert!(matches!(error, SyncTransferPlanError::Cancelled));
    assert!(observed_receive_lease.load(Ordering::SeqCst));
    assert!(!foreground_operations.has_active());
    assert!(!target_root.join("cancel-source/large.bin").exists());
    assert!(
        walkdir::WalkDir::new(&target_root)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_file())
            .all(|entry| !entry
                .file_name()
                .to_string_lossy()
                .contains("rua-transfer-tmp")),
        "cancelled transfer must remove partial temp files"
    );

    source.runtime.shutdown().await.expect("source shutdown");
    target.runtime.shutdown().await.expect("target shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn repeated_bulk_stream_failure_exhausts_the_current_operation_without_redialing_session() {
    let tmp = TempDir::new().expect("tempdir");
    let source_path = tmp.path().join("payload.bin");
    let target_root = tmp.path().join("target");
    std::fs::write(&source_path, vec![5u8; 64 * 1024]).expect("source");
    std::fs::create_dir_all(&target_root).expect("target root");
    let manifest = build_transfer_manifest(&[source_path]).expect("manifest");

    let network_secret = [40; 32];
    let source = build_peer(40, network_secret, vec![peer_entry(41)], None).await;
    let target = build_peer(41, network_secret, vec![peer_entry(40)], None).await;
    install_connectable_peer(&source, &target);
    let handler = Arc::new(AlwaysDropBulkHandler::default());
    target
        .runtime
        .session_manager()
        .set_bulk_transfer_handler(handler.clone());
    let (_cancel_tx, cancel_rx) = tokio::sync::watch::channel(RunControl::Running);

    let error = send_transfer_stream(
        source.runtime.session_manager(),
        TransferStreamSendRequest {
            operation_id: "bounded-failure".to_string(),
            source_device_id: source.device_id.clone(),
            target_device_id: target.device_id.clone(),
            manifest,
            target_dir: target_root,
            conflict_policy: ReceiveConflictPolicy::Rename,
            options: TransferExecutionOptions::default(),
        },
        cancel_rx,
        |_| {},
    )
    .await
    .expect_err("operation should exhaust retries");

    assert!(matches!(error, SyncTransferPlanError::RecoveryExhausted(_)));
    assert!(
        handler.calls.load(Ordering::SeqCst) >= 4,
        "initial attempt plus three recoveries should reach the Bulk handler"
    );
    assert_eq!(
        source
            .runtime
            .session_manager()
            .diagnostics()
            .physical_dial_attempts,
        1,
        "logical stream retries must reuse the shared Device Session"
    );

    source.runtime.shutdown().await.expect("source shutdown");
    target.runtime.shutdown().await.expect("target shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn brief_disconnect_resumes_multi_file_transfer_from_acknowledged_cursor() {
    let tmp = TempDir::new().expect("tempdir");
    let source_root = tmp.path().join("resume-source");
    let target_root = tmp.path().join("resume-target");
    std::fs::create_dir_all(&source_root).expect("source root");
    std::fs::create_dir_all(&target_root).expect("target root");
    std::fs::write(source_root.join("a-first.bin"), vec![1u8; 32 * 1024]).expect("first");
    std::fs::write(source_root.join("b-second.bin"), vec![2u8; 2 * 1024 * 1024]).expect("second");
    let manifest = build_transfer_manifest(std::slice::from_ref(&source_root)).expect("manifest");
    let first_size = 32 * 1024_u64;
    let total_bytes = manifest.total_bytes;

    let network_secret = [33; 32];
    let source = build_peer(33, network_secret, vec![peer_entry(34)], None).await;
    let target = build_peer(34, network_secret, vec![peer_entry(33)], None).await;
    install_connectable_peer(&source, &target);
    let foreground_operations = Arc::new(ForegroundOperationRegistry::new(true));
    target
        .runtime
        .session_manager()
        .set_bulk_transfer_handler(Arc::new(
            SyncTransferBulkHandler::new(target.device_id.clone())
                .with_foreground_operations(Arc::clone(&foreground_operations)),
        ));
    let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(RunControl::Running);
    let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel();
    let break_once = Arc::new(AtomicBool::new(false));
    let break_barrier = Arc::new(Barrier::new(2));
    let source_manager = source.runtime.session_manager().clone();
    let target_device_id = target.device_id.clone();
    let source_device_id = source.device_id.clone();
    let transfer_break_barrier = Arc::clone(&break_barrier);
    let transfer = tokio::spawn(async move {
        send_transfer_stream(
            &source_manager,
            TransferStreamSendRequest {
                operation_id: "resume-transfer".to_string(),
                source_device_id,
                target_device_id,
                manifest,
                target_dir: target_root,
                conflict_policy: ReceiveConflictPolicy::Rename,
                options: TransferExecutionOptions {
                    chunk_size: 32 * 1024,
                },
            },
            cancel_rx,
            {
                let break_once = Arc::clone(&break_once);
                let break_barrier = Arc::clone(&transfer_break_barrier);
                move |progress| {
                    if progress.bytes_transferred > first_size
                        && progress.bytes_transferred < total_bytes
                        && !break_once.swap(true, Ordering::SeqCst)
                    {
                        let _ = progress_tx.send(progress.bytes_transferred);
                        break_barrier.wait();
                    }
                }
            },
        )
        .await
    });

    let acknowledged_before_break = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let acknowledged = progress_rx.recv().await.expect("progress");
            if acknowledged > first_size && acknowledged < total_bytes {
                break acknowledged;
            }
        }
    })
    .await
    .expect("partial progress before disconnect");
    assert!(
        foreground_operations.has_active(),
        "receiver lease must remain active before the shared session disconnects"
    );
    source
        .runtime
        .session_manager()
        .disconnect_peer(&target.device_id)
        .await
        .expect("disconnect shared session");
    break_barrier.wait();
    let outcome = transfer
        .await
        .expect("transfer task")
        .expect("resumed transfer");
    drop(cancel_tx);

    assert_eq!(outcome.files_written, 2);
    assert_eq!(outcome.bytes_written, total_bytes);
    assert!(acknowledged_before_break > first_size);
    assert!(!foreground_operations.has_active());
    assert_eq!(
        std::fs::read(source_root.join("a-first.bin")).expect("source first"),
        std::fs::read(tmp.path().join("resume-target/resume-source/a-first.bin"))
            .expect("target first")
    );
    assert_eq!(
        std::fs::read(source_root.join("b-second.bin")).expect("source second"),
        std::fs::read(tmp.path().join("resume-target/resume-source/b-second.bin"))
            .expect("target second")
    );
    let diagnostics = source.runtime.session_manager().diagnostics();
    assert_eq!(diagnostics.physical_dial_attempts, 2);

    source.runtime.shutdown().await.expect("source shutdown");
    target.runtime.shutdown().await.expect("target shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn remote_source_uses_bounded_operation_commands_and_bulk_session() {
    let tmp = TempDir::new().expect("tempdir");
    let source_root = tmp.path().join("source");
    let target_root = tmp.path().join("target");
    let inbox_root = tmp.path().join("inbox");
    std::fs::create_dir_all(&source_root).expect("source root");
    std::fs::create_dir_all(&target_root).expect("target root");
    std::fs::write(source_root.join("payload.txt"), b"direct-bytes").expect("payload");

    let network_secret = [35; 32];
    let coordinator_id = device_id(35);
    let source_id = device_id(36);
    let target_id = device_id(37);
    let source_session_manager = Arc::new(RwLock::new(None));
    let source_handler = Arc::new(
        DeviceCommandHandler::new(inbox_root, source_id.clone())
            .with_sync_transfer_session_manager(Arc::clone(&source_session_manager)),
    );
    let source_command_handler: Arc<dyn DeviceSessionCommandHandler> = source_handler;
    let coordinator = build_peer(35, network_secret, vec![peer_entry(36)], None).await;
    let source = build_peer(
        36,
        network_secret,
        vec![peer_entry(35), peer_entry(37)],
        Some(source_command_handler),
    )
    .await;
    let target = build_peer(37, network_secret, vec![peer_entry(36)], None).await;
    install_connectable_peer(&coordinator, &source);
    install_connectable_peer(&source, &target);
    *source_session_manager
        .write()
        .expect("source session manager lock") = Some(source.runtime.session_manager().clone());
    target
        .runtime
        .session_manager()
        .set_bulk_transfer_handler(Arc::new(SyncTransferBulkHandler::new(target_id.clone())));
    let (_cancel_tx, cancel_rx) = tokio::sync::watch::channel(RunControl::Running);
    let mut totals = Vec::new();
    let mut progresses = Vec::new();

    let outcome = command_remote_source_send_to(
        coordinator.runtime.session_manager(),
        SyncTransferDeviceCommand {
            command_id: "direct-send".to_string(),
            source_device_id: coordinator_id,
            target_device_id: source_id,
            action: SyncTransferDeviceAction::StartSend {
                operation_id: "remote-source-operation".to_string(),
                source_paths: vec![source_root.to_string_lossy().to_string()],
                transfer_target_device_id: target_id,
                target_dir: target_root.to_string_lossy().to_string(),
                conflict: SyncTransferConflictPolicy::Rename,
            },
        },
        cancel_rx,
        |total| totals.push(total),
        |progress| progresses.push(progress.bytes_transferred),
    )
    .await
    .expect("remote direct send");

    assert_eq!(outcome.files_written, 1);
    assert_eq!(outcome.bytes_written, 12);
    assert!(totals.contains(&12));
    assert!(progresses.contains(&12));
    assert_eq!(
        std::fs::read(target_root.join("source/payload.txt")).expect("target payload"),
        b"direct-bytes"
    );
    assert_eq!(
        coordinator
            .runtime
            .session_manager()
            .diagnostics()
            .physical_dial_attempts,
        1
    );
    assert_eq!(
        source
            .runtime
            .session_manager()
            .diagnostics()
            .physical_dial_attempts,
        1
    );

    coordinator
        .runtime
        .shutdown()
        .await
        .expect("coordinator shutdown");
    source.runtime.shutdown().await.expect("source shutdown");
    target.runtime.shutdown().await.expect("target shutdown");
}
