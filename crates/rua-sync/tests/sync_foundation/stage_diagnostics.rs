use rua_sync::stage_diagnostics::{
    SyncInboundStageSample, SyncPublicationStageSample, SyncStageRecorder, SyncStageStatus,
};

#[test]
fn stage_diagnostics_aggregate_recent_latency_without_business_data() {
    let recorder = SyncStageRecorder::default();
    recorder.record_publication(SyncPublicationStageSample {
        correlation_id: "opaque-op".to_string(),
        commit_observed_at_ms: 10,
        publication_started_at_ms: 11,
        publication_completed_at_ms: 12,
        commit_to_publication_ms: 1,
        publication_ms: 1,
        status: SyncStageStatus::Success,
        failure_cause: None,
    });
    for (index, latency) in [10, 20, 100].into_iter().enumerate() {
        recorder.record_inbound(SyncInboundStageSample {
            correlation_id: format!("opaque-{index}"),
            source_commit_observed_at_ms: Some(1),
            transport_arrival_at_ms: 2,
            inbox_recorded_at_ms: 3,
            content_ready_at_ms: 4,
            decrypt_read_completed_at_ms: 5,
            domain_apply_completed_at_ms: 6,
            cache_refresh_completed_at_ms: 7,
            canonical_visible_at_ms: 8,
            transport_to_inbox_ms: 1,
            inbox_wait_ms: 1,
            content_wait_ms: 1,
            content_read_ms: 1,
            decrypt_ms: 1,
            domain_apply_ms: 1,
            cache_refresh_ms: 1,
            canonical_visibility_ms: 1,
            receiver_pipeline_ms: latency,
            end_to_end_wall_ms: Some(latency + 5),
            used_peer_download: false,
            status: SyncStageStatus::Success,
        });
    }

    let snapshot = recorder.snapshot();
    let receiver = snapshot
        .receiver_pipeline
        .as_ref()
        .expect("receiver summary");
    assert_eq!(receiver.samples, 3);
    assert_eq!(receiver.p50_ms, 20);
    assert_eq!(receiver.p95_ms, 100);
    assert_eq!(receiver.max_ms, 100);
    let serialized = serde_json::to_string(&snapshot).expect("serialize diagnostics");
    for forbidden in ["record_id", "payload", "secret", "canonical_path"] {
        assert!(
            !serialized.contains(forbidden),
            "diagnostics must not expose {forbidden}"
        );
    }
}
