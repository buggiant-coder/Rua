use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

use super::writer::{flush_capture, shutdown_capture_writers, stop_samplers, CaptureWriter};

// These tests include the process-wide shutdown barrier in the same module instance.
static TEST_LOCK: Mutex<()> = Mutex::new(());

fn root() -> std::path::PathBuf {
    std::env::temp_dir().join(format!("rua-capture-writer-{}", uuid::Uuid::new_v4()))
}

#[test]
fn slow_disk_does_not_block_enqueue_and_backlog_is_bounded_and_ordered() {
    let _guard = TEST_LOCK.lock().unwrap();
    let root = root();
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let written = Arc::new(Mutex::new(Vec::<Value>::new()));
    let output = written.clone();
    let writer = CaptureWriter::start_with_writer(
        root.clone(),
        Box::new(move |_, bytes, _| {
            let value: Value = serde_json::from_slice(bytes).unwrap();
            if value == 0 {
                entered_tx.send(()).unwrap();
                release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            }
            output.lock().unwrap().push(value);
            Ok(())
        }),
    )
    .unwrap();
    let producer = writer.clone();
    let path = root.join("trace.json");
    let (done_tx, done_rx) = mpsc::channel();
    let task = std::thread::spawn(move || {
        for sequence in 0..256 {
            assert!(producer.record(path.clone(), &sequence, false));
        }
        // Includes the write currently blocked on disk; do not hide it outside the bound.
        assert!(!producer.record(path, &256, false));
        done_tx.send(()).unwrap();
    });
    entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    done_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("enqueue waited for disk");
    assert_eq!(writer.status().pending_artifacts, 256);
    assert_eq!(writer.status().rejected_artifacts, 1);
    assert!(flush_capture(&root, Duration::from_millis(10)).is_err());
    release_tx.send(()).unwrap();
    task.join().unwrap();
    drop(writer);
    assert!(
        flush_capture(&root, Duration::from_secs(5)).is_err(),
        "rejection must stay visible"
    );
    assert_eq!(
        *written.lock().unwrap(),
        (0..256).map(Value::from).collect::<Vec<_>>()
    );
    let status = std::fs::read_dir(&root)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let status: Value = serde_json::from_slice(&std::fs::read(status).unwrap()).unwrap();
    assert_eq!(status["status"]["completed_artifacts"], 256);
    assert_eq!(status["status"]["rejected_artifacts"], 1);
    assert_eq!(status["status"]["closed"], true);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn background_failure_is_reported_by_flush_without_failing_enqueue() {
    let _guard = TEST_LOCK.lock().unwrap();
    let root = root();
    let writer = CaptureWriter::start_with_writer(
        root.clone(),
        Box::new(|_, _, _| Err(std::io::Error::other("injected disk error"))),
    )
    .unwrap();
    assert!(writer.record(root.join("trace.json"), &json!({"complete": true}), false));
    let error = flush_capture(&root, Duration::from_secs(5)).unwrap_err();
    assert!(error.to_string().contains("injected disk error"));
    assert_eq!(writer.status().failed_artifacts, 1);
    drop(writer);
    assert!(flush_capture(&root, Duration::from_secs(5)).is_err());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn pending_bytes_are_bounded_independently_of_artifact_count() {
    let _guard = TEST_LOCK.lock().unwrap();
    let root = root();
    let (release_tx, release_rx) = mpsc::channel();
    let writer = CaptureWriter::start_with_writer(
        root.clone(),
        Box::new(move |_, _, _| {
            release_rx.recv_timeout(Duration::from_secs(10)).unwrap();
            Ok(())
        }),
    )
    .unwrap();
    // Seven JSON strings fit; eight exceed 64 MiB even though the item count is small.
    let payload = "x".repeat(8 * 1024 * 1024);
    for _ in 0..7 {
        assert!(writer.record(root.join("large.json"), &payload, false));
    }
    assert!(!writer.record(root.join("large.json"), &payload, false));
    assert_eq!(writer.status().pending_artifacts, 7);
    assert!(writer.status().pending_bytes <= 64 * 1024 * 1024);
    for _ in 0..7 {
        release_tx.send(()).unwrap();
    }
    drop(writer);
    assert!(flush_capture(&root, Duration::from_secs(5)).is_err());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn queued_projection_is_not_overwritten_by_placeholder_and_final_snapshot_wins() {
    let _guard = TEST_LOCK.lock().unwrap();
    let root = root();
    let writer = CaptureWriter::start(root.clone()).unwrap();
    let projection = root.join("calls/call-1/projection.json");
    assert!(writer.record(projection.clone(), &json!({"status": "available"}), false));
    assert!(writer.record(projection.clone(), &json!({"status": "unavailable"}), true));
    let trace = root.join("calls/call-1/timing.json");
    for step in 0..20 {
        assert!(writer.record(trace.clone(), &step, false));
    }
    drop(writer);
    flush_capture(&root, Duration::from_secs(5)).unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&std::fs::read(projection).unwrap()).unwrap()["status"],
        "available"
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&std::fs::read(trace).unwrap()).unwrap(),
        19
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn reregister_while_finishing_reuses_the_ordered_writer() {
    let _guard = TEST_LOCK.lock().unwrap();
    let root = root();
    let writer = CaptureWriter::start(root.clone()).unwrap();
    let restored = CaptureWriter::start(root.clone()).unwrap();
    assert!(Arc::ptr_eq(&writer, &restored));
    assert!(writer.record(root.join("trace.json"), &1, false));
    drop(writer);
    assert!(restored.record(root.join("trace.json"), &2, false));
    drop(restored);
    flush_capture(&root, Duration::from_secs(5)).unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&std::fs::read(root.join("trace.json")).unwrap()).unwrap(),
        2
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn shutdown_waits_for_sampler_final_records_and_rejects_late_writes() {
    let _guard = TEST_LOCK.lock().unwrap();
    let root = root();
    let writer = CaptureWriter::start(root.clone()).unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let sampler = writer.clone();
        let path = root.join("sampler.json");
        writer.samplers.spawn(async move {
            assert!(sampler.record(path, &json!("final"), false));
        });
        stop_samplers(tokio::time::Instant::now() + Duration::from_secs(5))
            .await
            .unwrap();
    });
    // Tests below inject failures deliberately; their old closed writers need not
    // make this global drain successful, but this writer must still fully drain.
    let _ = shutdown_capture_writers(Duration::from_secs(5));
    flush_capture(&root, Duration::from_secs(5)).unwrap();
    assert_eq!(writer.status().completed_artifacts, 1);
    assert!(writer.status().closed);
    assert!(!writer.record(root.join("late.json"), &0, false));
    drop(writer);
    std::fs::remove_dir_all(root).unwrap();
}
