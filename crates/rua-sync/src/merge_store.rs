use std::path::Path;
use std::sync::Arc;

use iroh_docs::AuthorId;

use crate::hlc::HlcTimestamp;
use crate::sidecar::{
    ApplyCommit, ConflictEntry, LwwDecision, RecordState, SidecarDiagnostics, SidecarEntry,
    SidecarStore,
};
use crate::SyncError;

pub struct MergeStore {
    sidecar: Arc<SidecarStore>,
}

impl MergeStore {
    pub fn open(dir: &Path) -> Result<Self, SyncError> {
        Ok(Self {
            sidecar: Arc::new(SidecarStore::open(dir)?),
        })
    }

    pub fn from_shared(sidecar: Arc<SidecarStore>) -> Self {
        Self { sidecar }
    }

    pub fn has_applied_op(&self, op_id: &str) -> Result<bool, SyncError> {
        self.sidecar.has_applied_op(op_id)
    }

    pub fn applied_op_fingerprint(&self, op_id: &str) -> Result<Option<Option<String>>, SyncError> {
        self.sidecar.applied_op_fingerprint(op_id)
    }

    pub fn mark_applied_op(&self, op_id: &str) -> Result<(), SyncError> {
        self.sidecar.mark_applied_op(op_id)
    }

    pub fn get_record_cursor(&self, record_id: &str) -> Result<Option<SidecarEntry>, SyncError> {
        self.sidecar.get_cursor(record_id)
    }

    pub fn set_record_cursor(
        &self,
        record_id: &str,
        entry: &SidecarEntry,
    ) -> Result<(), SyncError> {
        self.sidecar.set_cursor(record_id, entry)
    }

    pub fn mark_tombstone(
        &self,
        record_id: &str,
        tombstone_hlc: HlcTimestamp,
    ) -> Result<bool, SyncError> {
        self.sidecar.mark_tombstone(record_id, tombstone_hlc)
    }

    pub fn resolve_lww(
        &self,
        record_id: &str,
        incoming_hlc: HlcTimestamp,
        incoming_is_delete: bool,
    ) -> Result<LwwDecision, SyncError> {
        self.sidecar
            .resolve_lww(record_id, incoming_hlc, incoming_is_delete)
    }

    pub fn put_base_snapshot(&self, content: &[u8]) -> Result<String, SyncError> {
        self.sidecar.put_base_snapshot(content)
    }

    pub fn get_base_snapshot(&self, hash_hex: &str) -> Result<Option<Vec<u8>>, SyncError> {
        self.sidecar.get_base_snapshot(hash_hex)
    }

    pub fn record_conflict(&self, entry: ConflictEntry) -> Result<String, SyncError> {
        self.sidecar.record_conflict(entry)
    }

    pub fn list_pending_conflicts(&self) -> Result<Vec<ConflictEntry>, SyncError> {
        self.sidecar.list_pending_conflicts()
    }

    pub fn commit_apply(
        &self,
        op_id: &str,
        record_id: &str,
        entry: &SidecarEntry,
    ) -> Result<bool, SyncError> {
        self.sidecar.commit_apply(op_id, record_id, entry)
    }

    pub fn commit(&self, commit: ApplyCommit) -> Result<bool, SyncError> {
        self.sidecar.commit(commit)
    }

    pub fn commit_inbound_apply(
        &self,
        author: AuthorId,
        key: &[u8],
        updated_at_ms: u64,
        commits: &[ApplyCommit],
    ) -> Result<usize, SyncError> {
        self.sidecar
            .commit_inbound_apply(author, key, updated_at_ms, commits)
    }

    pub fn diagnostics(&self) -> Result<SidecarDiagnostics, SyncError> {
        self.sidecar.diagnostics()
    }
}

pub fn live_cursor(hlc: HlcTimestamp, base_blob_hash: Option<String>) -> SidecarEntry {
    SidecarEntry {
        hlc,
        state: RecordState::Live,
        base_blob_hash,
    }
}

pub fn tombstone_cursor(hlc: HlcTimestamp, base_blob_hash: Option<String>) -> SidecarEntry {
    SidecarEntry {
        hlc,
        state: RecordState::Tombstoned,
        base_blob_hash,
    }
}
