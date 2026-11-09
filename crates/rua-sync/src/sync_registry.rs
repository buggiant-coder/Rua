use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde::Serialize;

use crate::merge_store::MergeStore;
use crate::opaque_replica_log::SyncChange;
use crate::sidecar::{ApplyCommit, SidecarEntry};
use crate::{SyncError, SyncResult};

#[derive(Debug, Clone)]
pub enum ApplyOutcome {
    Applied {
        cursor: SidecarEntry,
    },

    AppliedWithCursors {
        cursor: SidecarEntry,
        additional_cursors: Vec<(String, SidecarEntry)>,
    },

    Skipped,

    ConflictRecorded,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryApplyResult {
    pub domain: String,
    pub record_id: String,
    pub status: RegistryApplyStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistryApplyStatus {
    Applied,
    AlreadyApplied,
    Skipped,
    ConflictRecorded,
}

pub struct PreparedRegistryApply {
    pub result: RegistryApplyResult,
    pub commit: Option<ApplyCommit>,
}

pub fn sync_cursor_record_id(domain: &str, record_id: &str) -> String {
    format!("{domain}:{record_id}")
}

#[async_trait]
pub trait DomainSyncAdapter: Send + Sync {
    fn domain(&self) -> &str;

    async fn export_records(&self) -> SyncResult<Vec<SyncChange>>;

    async fn apply_change(
        &self,
        change: &SyncChange,
        merge_store: &MergeStore,
    ) -> SyncResult<ApplyOutcome>;
}

#[derive(Default)]
pub struct SyncRegistry {
    adapters: BTreeMap<String, Arc<dyn DomainSyncAdapter>>,
}

impl SyncRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, adapter: Arc<dyn DomainSyncAdapter>) -> SyncResult<()> {
        let domain = adapter.domain().to_string();
        if domain.is_empty() {
            return Err(SyncError::Store(
                "sync adapter domain must not be empty".to_string(),
            ));
        }
        if self.adapters.contains_key(&domain) {
            return Err(SyncError::Store(format!(
                "duplicate sync adapter for domain: {domain}"
            )));
        }
        self.adapters.insert(domain, adapter);
        Ok(())
    }

    pub fn adapter(&self, domain: &str) -> SyncResult<&Arc<dyn DomainSyncAdapter>> {
        self.adapters
            .get(domain)
            .ok_or_else(|| SyncError::MissingSyncAdapter {
                domain: domain.to_string(),
            })
    }

    pub fn domains(&self) -> Vec<String> {
        self.adapters.keys().cloned().collect()
    }

    pub async fn apply_change(
        &self,
        change: &SyncChange,
        merge_store: &MergeStore,
    ) -> SyncResult<RegistryApplyResult> {
        let mut prepared = self.prepare_change(change, merge_store).await?;
        if let Some(commit) = prepared.commit.take() {
            if !merge_store.commit(commit)? {
                prepared.result.status = RegistryApplyStatus::AlreadyApplied;
            }
        }
        Ok(prepared.result)
    }

    pub async fn prepare_change(
        &self,
        change: &SyncChange,
        merge_store: &MergeStore,
    ) -> SyncResult<PreparedRegistryApply> {
        let fingerprint = change_fingerprint(change)?;
        if let Some(applied_fingerprint) = merge_store.applied_op_fingerprint(&change.op_id)? {
            if applied_fingerprint
                .as_deref()
                .is_some_and(|applied| applied != fingerprint)
            {
                return Err(SyncError::InvalidPayload(format!(
                    "divergent duplicate operation: {}",
                    change.op_id
                )));
            }
            return Ok(PreparedRegistryApply {
                result: RegistryApplyResult {
                    domain: change.domain.clone(),
                    record_id: change.record_id.clone(),
                    status: RegistryApplyStatus::AlreadyApplied,
                },
                commit: None,
            });
        }

        let adapter = self.adapter(&change.domain)?;
        let outcome = adapter.apply_change(change, merge_store).await?;
        let cursor_record_id = sync_cursor_record_id(&change.domain, &change.record_id);
        let (status, commit) = match outcome {
            ApplyOutcome::Applied { cursor } => {
                let commit = ApplyCommit::with_cursor(&change.op_id, cursor_record_id, cursor)
                    .with_change_fingerprint(&fingerprint);
                (RegistryApplyStatus::Applied, Some(commit))
            }
            ApplyOutcome::AppliedWithCursors {
                cursor,
                additional_cursors,
            } => {
                let commit = ApplyCommit::with_cursors(
                    &change.op_id,
                    cursor_record_id,
                    cursor,
                    additional_cursors,
                )
                .with_change_fingerprint(&fingerprint);
                (RegistryApplyStatus::Applied, Some(commit))
            }
            ApplyOutcome::Skipped => {
                let commit = ApplyCommit::without_cursor(&change.op_id, cursor_record_id)
                    .with_change_fingerprint(&fingerprint);
                (RegistryApplyStatus::Skipped, Some(commit))
            }
            ApplyOutcome::ConflictRecorded => {
                let commit = ApplyCommit::without_cursor(&change.op_id, cursor_record_id)
                    .with_change_fingerprint(&fingerprint);
                (RegistryApplyStatus::ConflictRecorded, Some(commit))
            }
        };

        Ok(PreparedRegistryApply {
            result: RegistryApplyResult {
                domain: change.domain.clone(),
                record_id: change.record_id.clone(),
                status,
            },
            commit,
        })
    }

    pub async fn export_all(&self) -> SyncResult<Vec<SyncChange>> {
        let mut records = Vec::new();
        for adapter in self.adapters.values() {
            records.extend(adapter.export_records().await?);
        }
        Ok(records)
    }

    pub async fn export_all_best_effort(&self) -> (Vec<SyncChange>, Vec<(String, String)>) {
        let mut records = Vec::new();
        let mut errors = Vec::new();
        for (domain, adapter) in &self.adapters {
            match adapter.export_records().await {
                Ok(exported) => records.extend(exported),
                Err(e) => errors.push((domain.clone(), e.to_string())),
            }
        }
        (records, errors)
    }
}

fn change_fingerprint(change: &SyncChange) -> SyncResult<String> {
    #[derive(Serialize)]
    struct CanonicalChange<'a> {
        schema_version: u16,
        domain: &'a str,
        record_id: &'a str,
        kind: &'a str,
        device_id: &'a str,
        hlc: crate::hlc::HlcTimestamp,
        base: &'a Option<crate::opaque_replica_log::ChangeBase>,
        payload: &'a serde_json::Value,
    }

    let canonical = CanonicalChange {
        schema_version: change.schema_version,
        domain: &change.domain,
        record_id: &change.record_id,
        kind: &change.kind,
        device_id: &change.device_id,
        hlc: change.hlc,
        base: &change.base,
        payload: &change.payload,
    };
    Ok(blake3::hash(&serde_json::to_vec(&canonical)?)
        .to_hex()
        .to_string())
}
