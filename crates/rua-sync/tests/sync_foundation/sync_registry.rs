use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rua_sync::hlc::HlcTimestamp;
use rua_sync::merge_store::{live_cursor, tombstone_cursor, MergeStore};
use rua_sync::opaque_replica_log::SyncChange;
use rua_sync::sidecar::RecordState;
use rua_sync::sync_registry::{ApplyOutcome, DomainSyncAdapter, RegistryApplyStatus, SyncRegistry};
use rua_sync::{SyncError, SyncResult};

fn hlc(wall_ms: u64) -> HlcTimestamp {
    HlcTimestamp {
        wall_ms,
        counter: 0,
        device_id: [0; 8],
    }
}

fn change(op_id: &str, domain: &str, record_id: &str, kind: &str) -> SyncChange {
    SyncChange::new(
        op_id,
        domain,
        record_id,
        kind,
        "device-a",
        hlc(100),
        serde_json::json!({ "value": 1 }),
    )
}

struct StubAdapter {
    domain: String,
    outcome: ApplyOutcome,
    exports: Vec<SyncChange>,
    export_error: Option<String>,
    applied_ops: Mutex<Vec<String>>,
}

impl StubAdapter {
    fn new(domain: &str, outcome: ApplyOutcome, exports: Vec<SyncChange>) -> Self {
        Self {
            domain: domain.to_string(),
            outcome,
            exports,
            export_error: None,
            applied_ops: Mutex::new(Vec::new()),
        }
    }

    fn failing_export(domain: &str, error: &str) -> Self {
        Self {
            domain: domain.to_string(),
            outcome: ApplyOutcome::Skipped,
            exports: Vec::new(),
            export_error: Some(error.to_string()),
            applied_ops: Mutex::new(Vec::new()),
        }
    }

    fn applied_ops(&self) -> Vec<String> {
        self.applied_ops.lock().unwrap().clone()
    }
}

#[async_trait]
impl DomainSyncAdapter for StubAdapter {
    fn domain(&self) -> &str {
        &self.domain
    }

    async fn export_records(&self) -> SyncResult<Vec<SyncChange>> {
        if let Some(error) = &self.export_error {
            return Err(SyncError::Store(error.clone()));
        }
        Ok(self.exports.clone())
    }

    async fn apply_change(
        &self,
        change: &SyncChange,
        _merge_store: &MergeStore,
    ) -> SyncResult<ApplyOutcome> {
        self.applied_ops.lock().unwrap().push(change.op_id.clone());
        Ok(self.outcome.clone())
    }
}

#[tokio::test]
async fn registry_routes_apply_to_matching_domain_and_commits_cursor() {
    let dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(dir.path()).unwrap();
    let cursor = live_cursor(hlc(200), Some("base-hash".to_string()));
    let adapter = Arc::new(StubAdapter::new(
        "settings",
        ApplyOutcome::Applied {
            cursor: cursor.clone(),
        },
        Vec::new(),
    ));
    let mut registry = SyncRegistry::new();
    registry.register(adapter.clone()).unwrap();

    let result = registry
        .apply_change(
            &change("op-1", "settings", "settings", "upsert"),
            &merge_store,
        )
        .await
        .unwrap();

    assert_eq!(result.status, RegistryApplyStatus::Applied);
    assert_eq!(adapter.applied_ops(), vec!["op-1".to_string()]);
    assert!(merge_store.has_applied_op("op-1").unwrap());

    let stored = merge_store
        .get_record_cursor("settings:settings")
        .unwrap()
        .unwrap();
    assert_eq!(stored.hlc.wall_ms, cursor.hlc.wall_ms);
    assert_eq!(stored.state, RecordState::Live);
    assert_eq!(stored.base_blob_hash.as_deref(), Some("base-hash"));
}

#[tokio::test]
async fn applied_outcome_commits_additional_cursors_with_main_cursor() {
    let dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(dir.path()).unwrap();
    let adapter = Arc::new(StubAdapter::new(
        "skills",
        ApplyOutcome::AppliedWithCursors {
            cursor: live_cursor(hlc(200), None),
            additional_cursors: vec![(
                "skills:file:a.md".to_string(),
                live_cursor(hlc(201), Some("base-a".to_string())),
            )],
        },
        Vec::new(),
    ));
    let mut registry = SyncRegistry::new();
    registry.register(adapter).unwrap();

    let result = registry
        .apply_change(
            &change("op-skills", "skills", "skill-a", "upsert"),
            &merge_store,
        )
        .await
        .unwrap();

    assert_eq!(result.status, RegistryApplyStatus::Applied);
    assert!(merge_store
        .get_record_cursor("skills:skill-a")
        .unwrap()
        .is_some());
    let file_cursor = merge_store
        .get_record_cursor("skills:file:a.md")
        .unwrap()
        .unwrap();
    assert_eq!(file_cursor.hlc.wall_ms, 201);
    assert_eq!(file_cursor.base_blob_hash.as_deref(), Some("base-a"));
}

#[tokio::test]
async fn duplicate_op_is_not_routed_to_adapter_again() {
    let dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(dir.path()).unwrap();
    let adapter = Arc::new(StubAdapter::new(
        "settings",
        ApplyOutcome::Applied {
            cursor: live_cursor(hlc(200), None),
        },
        Vec::new(),
    ));
    let mut registry = SyncRegistry::new();
    registry.register(adapter.clone()).unwrap();
    let change = change("op-1", "settings", "settings", "upsert");

    let first = registry.apply_change(&change, &merge_store).await.unwrap();
    let second = registry.apply_change(&change, &merge_store).await.unwrap();

    assert_eq!(first.status, RegistryApplyStatus::Applied);
    assert_eq!(second.status, RegistryApplyStatus::AlreadyApplied);
    assert_eq!(adapter.applied_ops(), vec!["op-1".to_string()]);
}

#[tokio::test]
async fn duplicate_op_with_divergent_content_is_rejected_without_moving_cursor() {
    let dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(dir.path()).unwrap();
    let adapter = Arc::new(StubAdapter::new(
        "settings",
        ApplyOutcome::Applied {
            cursor: live_cursor(hlc(200), None),
        },
        Vec::new(),
    ));
    let mut registry = SyncRegistry::new();
    registry.register(adapter.clone()).unwrap();
    let original = change("op-1", "settings", "settings", "upsert");
    registry
        .apply_change(&original, &merge_store)
        .await
        .unwrap();
    let original_cursor = merge_store
        .get_record_cursor("settings:settings")
        .unwrap()
        .unwrap();
    drop(merge_store);
    let merge_store = MergeStore::open(dir.path()).unwrap();
    let mut divergent = original;
    divergent.payload = serde_json::json!({ "value": 2 });

    let error = registry
        .apply_change(&divergent, &merge_store)
        .await
        .expect_err("same operation identity with different content must fail");

    assert!(matches!(
        error,
        SyncError::InvalidPayload(ref detail) if detail.contains("divergent duplicate operation")
    ));
    assert_eq!(adapter.applied_ops(), vec!["op-1".to_string()]);
    assert_eq!(
        merge_store
            .get_record_cursor("settings:settings")
            .unwrap()
            .unwrap(),
        original_cursor
    );
}

#[tokio::test]
async fn missing_adapter_is_structured_error() {
    let dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(dir.path()).unwrap();
    let registry = SyncRegistry::new();

    let err = registry
        .apply_change(
            &change("op-1", "unknown", "record-1", "upsert"),
            &merge_store,
        )
        .await
        .unwrap_err();

    assert!(matches!(
        err,
        SyncError::MissingSyncAdapter { domain } if domain == "unknown"
    ));
}

#[tokio::test]
async fn export_all_asks_registered_adapters() {
    let mut registry = SyncRegistry::new();
    let settings_export = change("op-settings", "settings", "settings", "snapshot");
    let prompts_export = change("op-prompt", "prompts", "prompt-1", "snapshot");

    registry
        .register(Arc::new(StubAdapter::new(
            "settings",
            ApplyOutcome::Skipped,
            vec![settings_export],
        )))
        .unwrap();
    registry
        .register(Arc::new(StubAdapter::new(
            "prompts",
            ApplyOutcome::Skipped,
            vec![prompts_export],
        )))
        .unwrap();

    let exported = registry.export_all().await.unwrap();
    let domains: Vec<_> = exported
        .iter()
        .map(|change| change.domain.as_str())
        .collect();

    assert_eq!(domains, vec!["prompts", "settings"]);
}

#[tokio::test]
async fn export_all_best_effort_keeps_good_domains_when_one_adapter_fails() {
    let mut registry = SyncRegistry::new();
    let settings_export = change("op-settings", "settings", "settings", "snapshot");

    registry
        .register(Arc::new(StubAdapter::new(
            "settings",
            ApplyOutcome::Skipped,
            vec![settings_export],
        )))
        .unwrap();
    registry
        .register(Arc::new(StubAdapter::failing_export(
            "broken",
            "cannot decrypt local record",
        )))
        .unwrap();

    let (exported, errors) = registry.export_all_best_effort().await;

    assert_eq!(exported.len(), 1);
    assert_eq!(exported[0].domain, "settings");
    assert_eq!(
        errors,
        vec![(
            "broken".to_string(),
            "store: cannot decrypt local record".to_string()
        )]
    );
}

#[tokio::test]
async fn skipped_and_conflict_outcomes_still_mark_op_applied() {
    let dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(dir.path()).unwrap();
    let mut registry = SyncRegistry::new();
    registry
        .register(Arc::new(StubAdapter::new(
            "settings",
            ApplyOutcome::Skipped,
            Vec::new(),
        )))
        .unwrap();
    registry
        .register(Arc::new(StubAdapter::new(
            "prompts",
            ApplyOutcome::ConflictRecorded,
            Vec::new(),
        )))
        .unwrap();

    let skipped = registry
        .apply_change(
            &change("op-skip", "settings", "settings", "upsert"),
            &merge_store,
        )
        .await
        .unwrap();
    let conflict = registry
        .apply_change(
            &change("op-conflict", "prompts", "prompt-1", "upsert"),
            &merge_store,
        )
        .await
        .unwrap();

    assert_eq!(skipped.status, RegistryApplyStatus::Skipped);
    assert_eq!(conflict.status, RegistryApplyStatus::ConflictRecorded);
    assert!(merge_store.has_applied_op("op-skip").unwrap());
    assert!(merge_store.has_applied_op("op-conflict").unwrap());
    assert!(merge_store
        .get_record_cursor("settings:settings")
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn delete_change_can_commit_tombstone_cursor() {
    let dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(dir.path()).unwrap();
    let mut registry = SyncRegistry::new();
    registry
        .register(Arc::new(StubAdapter::new(
            "settings",
            ApplyOutcome::Applied {
                cursor: tombstone_cursor(hlc(300), None),
            },
            Vec::new(),
        )))
        .unwrap();

    let result = registry
        .apply_change(
            &change("delete-op", "settings", "settings", "delete"),
            &merge_store,
        )
        .await
        .unwrap();

    assert_eq!(result.status, RegistryApplyStatus::Applied);
    let cursor = merge_store
        .get_record_cursor("settings:settings")
        .unwrap()
        .unwrap();
    assert_eq!(cursor.state, RecordState::Tombstoned);
    assert_eq!(cursor.hlc.wall_ms, 300);
}
