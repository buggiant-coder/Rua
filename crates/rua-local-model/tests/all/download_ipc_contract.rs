use rua_local_model::{DownloadStatus, ErrorUpdate, IpcMessage, ProgressUpdate, StatusUpdate};
use serde_json::json;

#[test]
fn progress_message_uses_stable_snake_case_envelope() {
    let msg = IpcMessage::Progress(ProgressUpdate {
        id: "Qwen/Qwen2-0.5B-Instruct-GGUF".into(),
        bytes_downloaded: 12,
        bytes_total: 42,
        eta_secs: Some(3.5),
    });

    let encoded = serde_json::to_value(&msg).expect("progress json");

    assert_eq!(
        encoded,
        json!({
            "type": "progress",
            "id": "Qwen/Qwen2-0.5B-Instruct-GGUF",
            "bytes_downloaded": 12,
            "bytes_total": 42,
            "eta_secs": 3.5
        })
    );
    let decoded: IpcMessage = serde_json::from_value(encoded).expect("decode progress");
    assert_eq!(decoded, msg);
}

#[test]
fn progress_message_allows_missing_eta_for_older_emitters() {
    let raw = json!({
        "type": "progress",
        "id": "demo/model",
        "bytes_downloaded": 7,
        "bytes_total": 10
    });

    let decoded: IpcMessage = serde_json::from_value(raw).expect("decode progress");

    match decoded {
        IpcMessage::Progress(progress) => {
            assert_eq!(progress.id, "demo/model");
            assert_eq!(progress.bytes_downloaded, 7);
            assert_eq!(progress.bytes_total, 10);
            assert_eq!(progress.eta_secs, None);
        }
        other => panic!("expected progress, got {other:?}"),
    }
}

#[test]
fn status_message_uses_stable_status_values_and_optional_detail() {
    let started = IpcMessage::Status(StatusUpdate {
        id: "server".into(),
        status: DownloadStatus::Started,
        detail: None,
    });
    let completed = IpcMessage::Status(StatusUpdate {
        id: "server".into(),
        status: DownloadStatus::Completed,
        detail: Some("llama-server".into()),
    });
    let cancelled = IpcMessage::Status(StatusUpdate {
        id: "server".into(),
        status: DownloadStatus::Cancelled,
        detail: None,
    });

    assert_eq!(
        serde_json::to_value(&started).expect("started json"),
        json!({"type": "status", "id": "server", "status": "started"})
    );
    assert_eq!(
        serde_json::to_value(&completed).expect("completed json"),
        json!({
            "type": "status",
            "id": "server",
            "status": "completed",
            "detail": "llama-server"
        })
    );
    assert_eq!(
        serde_json::to_value(&cancelled).expect("cancelled json"),
        json!({"type": "status", "id": "server", "status": "cancelled"})
    );

    for msg in [started, completed, cancelled] {
        let encoded = serde_json::to_string(&msg).expect("status json");
        let decoded: IpcMessage = serde_json::from_str(&encoded).expect("decode status");
        assert_eq!(decoded, msg);
    }
}

#[test]
fn error_message_carries_retryability_without_requiring_it() {
    let retryable = IpcMessage::Error(ErrorUpdate {
        id: "demo/model".into(),
        error: "network timeout".into(),
        retryable: Some(true),
    });
    let terminal = IpcMessage::Error(ErrorUpdate {
        id: "demo/model".into(),
        error: "repo has no gguf".into(),
        retryable: None,
    });

    assert_eq!(
        serde_json::to_value(&retryable).expect("retryable json"),
        json!({
            "type": "error",
            "id": "demo/model",
            "error": "network timeout",
            "retryable": true
        })
    );
    assert_eq!(
        serde_json::to_value(&terminal).expect("terminal json"),
        json!({
            "type": "error",
            "id": "demo/model",
            "error": "repo has no gguf"
        })
    );
}

#[test]
fn unknown_message_type_is_rejected_at_the_ipc_boundary() {
    let err = serde_json::from_value::<IpcMessage>(json!({
        "type": "done",
        "id": "demo/model"
    }))
    .expect_err("unknown IPC message kinds must not be accepted silently");

    assert!(
        err.to_string().contains("unknown variant"),
        "unexpected serde error: {err}"
    );
}
