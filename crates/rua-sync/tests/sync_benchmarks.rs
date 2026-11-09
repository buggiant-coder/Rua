use std::time::{Duration, Instant};

use bytes::Bytes;
use iroh_blobs::HashAndFormat;
use rua_sync::endpoint::SyncNode;
use rua_sync::hlc::HlcTimestamp;
use rua_sync::identity::{DeviceIdentity, NetworkSecret};
use rua_sync::inbound_inbox::InboundInbox;
use rua_sync::opaque_replica_log::{OpaqueReplicaLog, SyncChange};
use serde::Serialize;

#[derive(Serialize)]
struct ManySmallEntryReport {
    entries: usize,
    log_entries: usize,
    payload_bytes: usize,
    append_ms: u128,
    rbsr_ms: u128,
    rbsr_events_sent: usize,
    rbsr_events_received: usize,
    rbsr_connect_ms: u64,
    rbsr_process_ms: u64,
    repair_scan_ms: u128,
    repair_scanned: usize,
    repair_recorded: usize,
    read_decrypt_ms: u128,
    readable: usize,
}

#[derive(Serialize)]
struct RawBlobReport {
    bytes: usize,
    add_ms: u128,
    preconnect_ms: u128,
    download_ms: u128,
}

fn hlc(counter: u32) -> HlcTimestamp {
    HlcTimestamp {
        wall_ms: 42,
        counter,
        device_id: [7; 8],
    }
}

fn payload_bytes(size: usize, seed: u8) -> String {
    let mut bytes = Vec::with_capacity(size);
    for i in 0..size {
        bytes.push(seed.wrapping_add((i % 251) as u8));
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

async fn spawn_node(
    network: &NetworkSecret,
) -> (
    tempfile::TempDir,
    DeviceIdentity,
    SyncNode,
    OpaqueReplicaLog,
) {
    let dir = tempfile::tempdir().expect("tempdir");
    let device = DeviceIdentity::generate();
    let node = SyncNode::bind_local(dir.path(), network.namespace_secret(), &device)
        .await
        .expect("bind local sync node");
    let log = OpaqueReplicaLog::new(
        node.doc().clone(),
        device.to_iroh_author(),
        network.derive_sync_key(),
        node.blobs_store().clone(),
    );
    (dir, device, node, log)
}

async fn bench_many_small_v4_entries(entries: usize, payload_size: usize) -> ManySmallEntryReport {
    let network = NetworkSecret::generate();
    let (_dir_a, _device_a, node_a, log_a) = spawn_node(&network).await;
    let (_dir_b, _device_b, node_b, log_b) = spawn_node(&network).await;
    let inbox_dir = tempfile::tempdir().expect("inbox tempdir");
    let inbox = InboundInbox::open(inbox_dir.path()).expect("open inbox");

    let append_start = Instant::now();
    for idx in 0..entries {
        let change = SyncChange::new(
            format!("bench-op-{idx}"),
            "bench_domain",
            format!("bench-record-{idx}"),
            "upsert",
            "device-a",
            hlc(idx as u32),
            serde_json::json!({
                "idx": idx,
                "content": payload_bytes(payload_size, idx as u8),
            }),
        );
        log_a
            .append_change(&change)
            .await
            .expect("append benchmark change");
    }
    let append_ms = append_start.elapsed().as_millis();

    let rbsr_start = Instant::now();
    let rbsr =
        rua_sync::transport::sync_now(&node_b, node_a.local_peer_addr(), Duration::from_secs(30))
            .await
            .expect("local RBSR sync");
    let rbsr_ms = rbsr_start.elapsed().as_millis();

    let repair_start = Instant::now();
    let repair = inbox
        .repair_scan_doc_entries(node_b.doc())
        .await
        .expect("repair scan");
    let repair_scan_ms = repair_start.elapsed().as_millis();

    let read_start = Instant::now();
    let mut readable = 0usize;
    for entry in inbox.pending_ready(u64::MAX).expect("pending entries") {
        let change = log_b
            .read_change_with_peer_download(
                entry.author,
                &entry.key,
                node_b.blob_downloader(),
                vec![node_a.node_id()],
            )
            .await
            .expect("read remote change");
        if change.is_some() {
            readable += 1;
        }
    }
    let read_decrypt_ms = read_start.elapsed().as_millis();

    node_a.shutdown().await;
    node_b.shutdown().await;

    ManySmallEntryReport {
        entries,
        log_entries: entries,
        payload_bytes: payload_size,
        append_ms,
        rbsr_ms,
        rbsr_events_sent: rbsr.events_sent,
        rbsr_events_received: rbsr.events_received,
        rbsr_connect_ms: rbsr.connect_ms,
        rbsr_process_ms: rbsr.process_ms,
        repair_scan_ms,
        repair_scanned: repair.scanned,
        repair_recorded: repair.recorded,
        read_decrypt_ms,
        readable,
    }
}

async fn bench_many_small_v4_grouped_entries(
    entries: usize,
    payload_size: usize,
    group_size: usize,
) -> ManySmallEntryReport {
    let network = NetworkSecret::generate();
    let (_dir_a, _device_a, node_a, log_a) = spawn_node(&network).await;
    let (_dir_b, _device_b, node_b, log_b) = spawn_node(&network).await;
    let inbox_dir = tempfile::tempdir().expect("inbox tempdir");
    let inbox = InboundInbox::open(inbox_dir.path()).expect("open inbox");

    let changes = (0..entries)
        .map(|idx| {
            SyncChange::new(
                format!("bench-batch-op-{idx}"),
                "bench_domain",
                format!("bench-batch-record-{idx}"),
                "upsert",
                "device-a",
                hlc(idx as u32),
                serde_json::json!({
                    "idx": idx,
                    "content": payload_bytes(payload_size, idx as u8),
                }),
            )
        })
        .collect::<Vec<_>>();

    let append_start = Instant::now();
    let mut log_entries = 0usize;
    for (idx, group) in changes.chunks(group_size).enumerate() {
        log_a
            .append_change_batch(&format!("bench-group-{idx}"), Some("bulk"), group)
            .await
            .expect("append benchmark change group");
        log_entries += group.len();
    }
    let append_ms = append_start.elapsed().as_millis();

    let rbsr_start = Instant::now();
    let rbsr =
        rua_sync::transport::sync_now(&node_b, node_a.local_peer_addr(), Duration::from_secs(30))
            .await
            .expect("local RBSR sync");
    let rbsr_ms = rbsr_start.elapsed().as_millis();

    let repair_start = Instant::now();
    let repair = inbox
        .repair_scan_doc_entries(node_b.doc())
        .await
        .expect("repair scan");
    let repair_scan_ms = repair_start.elapsed().as_millis();

    let read_start = Instant::now();
    let mut readable = 0usize;
    for entry in inbox.pending_ready(u64::MAX).expect("pending entries") {
        let frame = log_b
            .read_frame_with_peer_download(
                entry.author,
                &entry.key,
                node_b.blob_downloader(),
                vec![node_a.node_id()],
            )
            .await
            .expect("read remote frame")
            .expect("frame exists");
        readable += frame.into_changes().len();
    }
    let read_decrypt_ms = read_start.elapsed().as_millis();

    node_a.shutdown().await;
    node_b.shutdown().await;

    ManySmallEntryReport {
        entries,
        log_entries,
        payload_bytes: payload_size,
        append_ms,
        rbsr_ms,
        rbsr_events_sent: rbsr.events_sent,
        rbsr_events_received: rbsr.events_received,
        rbsr_connect_ms: rbsr.connect_ms,
        rbsr_process_ms: rbsr.process_ms,
        repair_scan_ms,
        repair_scanned: repair.scanned,
        repair_recorded: repair.recorded,
        read_decrypt_ms,
        readable,
    }
}

async fn bench_raw_blob_download(bytes_len: usize) -> RawBlobReport {
    let network = NetworkSecret::generate();
    let (_dir_a, _device_a, node_a, _log_a) = spawn_node(&network).await;
    let (_dir_b, _device_b, node_b, _log_b) = spawn_node(&network).await;

    let content = deterministic_blob(bytes_len);
    let add_start = Instant::now();
    let tag = node_a
        .blobs_store()
        .add_bytes(Bytes::from(content))
        .await
        .expect("add source blob");
    let add_ms = add_start.elapsed().as_millis();

    let preconnect_start = Instant::now();
    let _connection = node_b
        .endpoint()
        .connect(node_a.local_peer_addr(), iroh_blobs::ALPN)
        .await
        .expect("preconnect blob protocol");
    let preconnect_ms = preconnect_start.elapsed().as_millis();

    let download_start = Instant::now();
    node_b
        .blobs_store()
        .downloader(node_b.endpoint())
        .download(HashAndFormat::raw(tag.hash), vec![node_a.node_id()])
        .await
        .expect("download blob");
    let download_ms = download_start.elapsed().as_millis();

    node_a.shutdown().await;
    node_b.shutdown().await;

    RawBlobReport {
        bytes: bytes_len,
        add_ms,
        preconnect_ms,
        download_ms,
    }
}

fn deterministic_blob(bytes_len: usize) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(bytes_len);
    for i in 0..bytes_len {
        bytes.push(((i * 31) % 251) as u8);
    }
    bytes
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "operational benchmark; run explicitly with --ignored --nocapture"]
async fn bench_many_small_v4_entry_sync_500() {
    let report = bench_many_small_v4_entries(500, 512).await;
    println!("{}", serde_json::to_string_pretty(&report).unwrap());
    assert_eq!(report.repair_recorded, 500);
    assert_eq!(report.readable, 500);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "operational benchmark; run explicitly with --ignored --nocapture"]
async fn bench_many_small_v4_entry_sync_1000() {
    let report = bench_many_small_v4_entries(1_000, 512).await;
    println!("{}", serde_json::to_string_pretty(&report).unwrap());
    assert_eq!(report.repair_recorded, 1_000);
    assert_eq!(report.readable, 1_000);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "operational benchmark; run explicitly with --ignored --nocapture"]
async fn bench_many_small_v4_grouped_entry_sync_1000_by_100() {
    let report = bench_many_small_v4_grouped_entries(1_000, 512, 100).await;
    println!("{}", serde_json::to_string_pretty(&report).unwrap());
    assert_eq!(report.log_entries, 1_000);
    assert_eq!(report.repair_recorded, 1_000);
    assert_eq!(report.readable, 1_000);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "operational benchmark; run explicitly with --ignored --nocapture"]
async fn bench_raw_iroh_blob_download_10mb() {
    let report = bench_raw_blob_download(10 * 1024 * 1024).await;
    println!("{}", serde_json::to_string_pretty(&report).unwrap());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "operational benchmark; run explicitly with --ignored --nocapture"]
async fn bench_raw_iroh_blob_download_100mb() {
    let report = bench_raw_blob_download(100 * 1024 * 1024).await;
    println!("{}", serde_json::to_string_pretty(&report).unwrap());
}
