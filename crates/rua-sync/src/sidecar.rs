use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use iroh_docs::AuthorId;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};

use crate::hlc::HlcTimestamp;
use crate::sync_state::{SyncStateConnection, SyncStateConnectionGuard};
use crate::SyncError;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictPolicy {
    #[default]
    Silent,
    Review,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConflictEntry {
    pub id: String,
    pub live_path: String,
    pub record_id: String,
    pub domain: String,
    pub winner_text: String,
    pub loser_text: String,
    pub winner_hlc: HlcTimestamp,
    pub created_at_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LwwDecision {
    Apply,
    Skip,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordState {
    Live,
    Tombstoned,
}

impl RecordState {
    fn as_db(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Tombstoned => "tombstoned",
        }
    }

    fn from_db(value: &str) -> Result<Self, SyncError> {
        match value {
            "live" => Ok(Self::Live),
            "tombstoned" => Ok(Self::Tombstoned),
            value => Err(SyncError::Store(format!(
                "invalid sync record state: {value}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SidecarEntry {
    pub hlc: HlcTimestamp,
    pub state: RecordState,

    pub base_blob_hash: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct DomainCursorCounts {
    pub live: usize,
    pub tombstoned: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SidecarDiagnostics {
    pub applied_ops_total: usize,
    pub applied_ops_by_domain: BTreeMap<String, usize>,
    pub cursors_by_domain: BTreeMap<String, DomainCursorCounts>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyCommit {
    op_id: String,
    record_id: String,
    change_fingerprint: Option<String>,
    cursors: Vec<(String, SidecarEntry)>,
}

impl ApplyCommit {
    pub fn with_cursor(
        op_id: impl Into<String>,
        record_id: impl Into<String>,
        cursor: SidecarEntry,
    ) -> Self {
        let record_id = record_id.into();
        Self {
            op_id: op_id.into(),
            record_id: record_id.clone(),
            change_fingerprint: None,
            cursors: vec![(record_id, cursor)],
        }
    }

    pub fn with_cursors(
        op_id: impl Into<String>,
        record_id: impl Into<String>,
        cursor: SidecarEntry,
        mut additional_cursors: Vec<(String, SidecarEntry)>,
    ) -> Self {
        let record_id = record_id.into();
        additional_cursors.push((record_id.clone(), cursor));
        Self {
            op_id: op_id.into(),
            record_id,
            change_fingerprint: None,
            cursors: additional_cursors,
        }
    }

    pub fn without_cursor(op_id: impl Into<String>, record_id: impl Into<String>) -> Self {
        Self {
            op_id: op_id.into(),
            record_id: record_id.into(),
            change_fingerprint: None,
            cursors: Vec::new(),
        }
    }

    pub fn with_change_fingerprint(mut self, fingerprint: impl Into<String>) -> Self {
        self.change_fingerprint = Some(fingerprint.into());
        self
    }
}

pub struct SidecarStore {
    snapshots_dir: PathBuf,
    dir: PathBuf,
    connection: SyncStateConnection,
}

impl SidecarStore {
    pub fn open(dir: &Path) -> Result<Self, SyncError> {
        std::fs::create_dir_all(dir)?;
        let snapshots_dir = dir.join("snapshots");
        std::fs::create_dir_all(&snapshots_dir)?;
        let connection = SyncStateConnection::open(dir)?;

        Ok(Self {
            snapshots_dir,
            dir: dir.to_path_buf(),
            connection,
        })
    }

    /// Persisted namespace binding, independent of live transport availability.
    /// This read does not establish Device authorization or create a binding.
    pub fn bound_network_namespace(&self) -> Result<Option<[u8; 32]>, SyncError> {
        let connection = self.connection()?;
        let exists: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master
                 WHERE type = 'table' AND name = 'sync_network_binding')",
                [],
                |row| row.get(0),
            )
            .map_err(|error| sql_error("inspect network binding", error))?;
        if !exists {
            return Ok(None);
        }
        connection
            .query_row(
                "SELECT namespace FROM sync_network_binding WHERE singleton = 1",
                [],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .optional()
            .map_err(|error| sql_error("read network binding", error))?
            .map(|namespace| {
                namespace.try_into().map_err(|_| {
                    SyncError::Store("persisted sync namespace must contain 32 bytes".into())
                })
            })
            .transpose()
    }

    pub fn get_cursor(&self, record_id: &str) -> Result<Option<SidecarEntry>, SyncError> {
        let connection = self.connection()?;
        connection
            .query_row(
                "SELECT hlc, state, base_blob_hash
                 FROM sync_record_cursors
                 WHERE record_id = ?1",
                params![record_id],
                |row| {
                    Ok((
                        row.get::<_, Vec<u8>>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| sql_error("read cursor", error))?
            .map(|(hlc, state, base_blob_hash)| {
                Ok(SidecarEntry {
                    hlc: decode_hlc(&hlc)?,
                    state: RecordState::from_db(&state)?,
                    base_blob_hash,
                })
            })
            .transpose()
    }

    pub fn set_cursor(&self, record_id: &str, entry: &SidecarEntry) -> Result<(), SyncError> {
        let connection = self.connection()?;
        upsert_cursor(&connection, record_id, entry)?;
        Ok(())
    }

    pub fn has_applied_op(&self, op_id: &str) -> Result<bool, SyncError> {
        Ok(self.applied_op_fingerprint(op_id)?.is_some())
    }

    pub fn applied_op_fingerprint(&self, op_id: &str) -> Result<Option<Option<String>>, SyncError> {
        let connection = self.connection()?;
        connection
            .query_row(
                "SELECT change_fingerprint FROM sync_applied_ops WHERE op_id = ?1",
                params![op_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()
            .map_err(|error| sql_error("read applied op", error))
    }

    pub fn mark_applied_op(&self, op_id: &str) -> Result<(), SyncError> {
        let connection = self.connection()?;
        connection
            .execute(
                "INSERT OR IGNORE INTO sync_applied_ops (
                     op_id, domain, record_id, change_fingerprint
                 ) VALUES (?1, 'unknown', '', NULL)",
                params![op_id],
            )
            .map_err(|error| sql_error("mark applied op", error))?;
        Ok(())
    }

    pub fn commit_apply(
        &self,
        op_id: &str,
        record_id: &str,
        entry: &SidecarEntry,
    ) -> Result<bool, SyncError> {
        self.commit(ApplyCommit::with_cursor(op_id, record_id, entry.clone()))
    }

    pub fn commit(&self, commit: ApplyCommit) -> Result<bool, SyncError> {
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction()
            .map_err(|error| sql_error("begin apply transaction", error))?;
        let committed = commit_one(&transaction, &commit)?;
        transaction
            .commit()
            .map_err(|error| sql_error("commit apply transaction", error))?;
        Ok(committed)
    }

    pub fn commit_inbound_apply(
        &self,
        author: AuthorId,
        key: &[u8],
        updated_at_ms: u64,
        commits: &[ApplyCommit],
    ) -> Result<usize, SyncError> {
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction()
            .map_err(|error| sql_error("begin inbound apply transaction", error))?;
        let mut committed = 0usize;
        for commit in commits {
            if commit_one(&transaction, commit)? {
                committed += 1;
            }
        }
        let updated = transaction
            .execute(
                "UPDATE inbound_inbox
                 SET state = 'applied',
                     updated_at_ms = ?3,
                     retry_after_ms = NULL,
                     last_error = NULL
                 WHERE author = ?1 AND entry_key = ?2",
                params![&author.as_bytes()[..], key, db_i64(updated_at_ms)],
            )
            .map_err(|error| sql_error("mark inbound applied", error))?;
        if updated != 1 {
            return Err(SyncError::Store(format!(
                "inbound inbox entry missing during apply commit: {}:{}",
                hex::encode(author.as_bytes()),
                hex::encode(key)
            )));
        }
        transaction
            .commit()
            .map_err(|error| sql_error("commit inbound apply transaction", error))?;
        Ok(committed)
    }

    pub fn diagnostics(&self) -> Result<SidecarDiagnostics, SyncError> {
        let Some(connection) = self.connection.try_lock()? else {
            return Ok(Default::default());
        };
        let mut diagnostics = SidecarDiagnostics::default();
        {
            let mut statement = connection
                .prepare(
                    "SELECT domain, COUNT(*)
                     FROM sync_applied_ops
                     GROUP BY domain",
                )
                .map_err(|error| sql_error("prepare applied diagnostics", error))?;
            let rows = statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
                })
                .map_err(|error| sql_error("query applied diagnostics", error))?;
            for row in rows {
                let (domain, count) =
                    row.map_err(|error| sql_error("read applied diagnostics", error))?;
                let count = usize_from_i64(count, "applied op count")?;
                diagnostics.applied_ops_total += count;
                diagnostics.applied_ops_by_domain.insert(domain, count);
            }
        }

        {
            let mut statement = connection
                .prepare("SELECT record_id, state FROM sync_record_cursors")
                .map_err(|error| sql_error("prepare cursor diagnostics", error))?;
            let rows = statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(|error| sql_error("query cursor diagnostics", error))?;
            for row in rows {
                let (record_id, state) =
                    row.map_err(|error| sql_error("read cursor diagnostics", error))?;
                let domain = cursor_domain(&record_id);
                let counts = diagnostics.cursors_by_domain.entry(domain).or_default();
                match RecordState::from_db(&state)? {
                    RecordState::Live => counts.live += 1,
                    RecordState::Tombstoned => counts.tombstoned += 1,
                }
            }
        }

        Ok(diagnostics)
    }

    pub fn mark_tombstone(
        &self,
        record_id: &str,
        tombstone_hlc: HlcTimestamp,
    ) -> Result<bool, SyncError> {
        let connection = self.connection()?;
        let transitioned = connection
            .execute(
                "UPDATE sync_record_cursors
                 SET state = 'tombstoned', hlc = ?2
                 WHERE record_id = ?1 AND state = 'live'",
                params![record_id, &tombstone_hlc.to_sortable_bytes()[..]],
            )
            .map_err(|error| sql_error("mark tombstone", error))?;
        Ok(transitioned == 1)
    }

    pub fn put_base_snapshot(&self, content: &[u8]) -> Result<String, SyncError> {
        let hash = blake3::hash(content);
        let hex = hash.to_hex();
        std::fs::create_dir_all(&self.snapshots_dir)?;
        let path = self.snapshots_dir.join(&*hex);
        if !path.exists() {
            std::fs::write(&path, content)?;
        }
        Ok(hex.to_string())
    }

    pub fn resolve_lww(
        &self,
        record_id: &str,
        incoming_hlc: HlcTimestamp,
        incoming_is_delete: bool,
    ) -> Result<LwwDecision, SyncError> {
        match self.get_cursor(record_id)? {
            None => Ok(LwwDecision::Apply),
            Some(entry) => match (entry.state, incoming_is_delete) {
                (RecordState::Tombstoned, true) => Ok(LwwDecision::Skip),
                (RecordState::Tombstoned, false) => {
                    if incoming_hlc > entry.hlc {
                        Ok(LwwDecision::Apply)
                    } else {
                        Ok(LwwDecision::Skip)
                    }
                }
                (RecordState::Live, true) => {
                    if incoming_hlc > entry.hlc {
                        Ok(LwwDecision::Apply)
                    } else {
                        Ok(LwwDecision::Skip)
                    }
                }
                (RecordState::Live, false) => Ok(LwwDecision::Apply),
            },
        }
    }

    pub fn get_base_snapshot(&self, hash_hex: &str) -> Result<Option<Vec<u8>>, SyncError> {
        let path = self.snapshots_dir.join(hash_hex);
        if path.exists() {
            Ok(Some(std::fs::read(&path)?))
        } else {
            Ok(None)
        }
    }

    fn policy_path(&self) -> PathBuf {
        self.dir.join("conflict_policy.json")
    }

    fn conflicts_path(&self) -> PathBuf {
        self.dir.join("pending_conflicts.json")
    }

    pub fn conflict_policy(&self) -> Result<ConflictPolicy, SyncError> {
        let path = self.policy_path();
        if !path.exists() {
            return Ok(ConflictPolicy::default());
        }
        let raw = std::fs::read_to_string(&path)?;
        let wrapper: serde_json::Value = serde_json::from_str(&raw)
            .map_err(|e| SyncError::Store(format!("conflict_policy parse: {e}")))?;
        let policy_str = wrapper
            .get("policy")
            .and_then(|v| v.as_str())
            .unwrap_or("silent");
        match policy_str {
            "review" => Ok(ConflictPolicy::Review),
            _ => Ok(ConflictPolicy::Silent),
        }
    }

    pub fn set_conflict_policy(&self, policy: ConflictPolicy) -> Result<(), SyncError> {
        let wrapper = serde_json::json!({ "policy": policy });
        let content =
            serde_json::to_string_pretty(&wrapper).map_err(|e| SyncError::Store(e.to_string()))?;
        std::fs::write(self.policy_path(), content)?;
        Ok(())
    }

    pub fn record_conflict(&self, entry: ConflictEntry) -> Result<String, SyncError> {
        let mut entries = self.load_conflicts()?;
        let id = entry.id.clone();
        entries.push(entry);
        self.save_conflicts(&entries)?;
        Ok(id)
    }

    pub fn list_pending_conflicts(&self) -> Result<Vec<ConflictEntry>, SyncError> {
        self.load_conflicts()
    }

    pub fn resolve_conflict(&self, id: &str, chosen: &str) -> Result<(), SyncError> {
        let entries = self.load_conflicts()?;
        let entry = entries
            .iter()
            .find(|e| e.id == id)
            .ok_or_else(|| SyncError::Store(format!("conflict not found: {id}")))?;

        let live_path = PathBuf::from(&entry.live_path);
        std::fs::write(&live_path, chosen)?;
        self.put_base_snapshot(chosen.as_bytes())?;

        let remaining: Vec<ConflictEntry> = entries.into_iter().filter(|e| e.id != id).collect();
        self.save_conflicts(&remaining)?;
        Ok(())
    }

    fn load_conflicts(&self) -> Result<Vec<ConflictEntry>, SyncError> {
        let path = self.conflicts_path();
        if !path.exists() {
            return Ok(Vec::new());
        }
        let raw = std::fs::read_to_string(&path)?;
        let entries: Vec<ConflictEntry> = serde_json::from_str(&raw)
            .map_err(|e| SyncError::Store(format!("conflicts parse: {e}")))?;
        Ok(entries)
    }

    fn save_conflicts(&self, entries: &[ConflictEntry]) -> Result<(), SyncError> {
        let content =
            serde_json::to_string_pretty(entries).map_err(|e| SyncError::Store(e.to_string()))?;
        std::fs::write(self.conflicts_path(), content)?;
        Ok(())
    }

    pub fn close(&self) -> Result<(), SyncError> {
        self.connection.close()
    }

    pub fn reopen(&self) -> Result<(), SyncError> {
        self.connection.reopen()
    }

    fn connection(&self) -> Result<SyncStateConnectionGuard<'_>, SyncError> {
        self.connection.lock()
    }
}

fn commit_one(transaction: &Transaction<'_>, commit: &ApplyCommit) -> Result<bool, SyncError> {
    let inserted = transaction
        .execute(
            "INSERT OR IGNORE INTO sync_applied_ops (
                 op_id, domain, record_id, change_fingerprint
             ) VALUES (?1, ?2, ?3, ?4)",
            params![
                commit.op_id,
                cursor_domain(&commit.record_id),
                commit.record_id,
                commit.change_fingerprint,
            ],
        )
        .map_err(|error| sql_error("insert applied op", error))?;
    if inserted == 0 {
        return Ok(false);
    }
    for (record_id, cursor) in &commit.cursors {
        upsert_cursor(transaction, record_id, cursor)?;
    }
    Ok(true)
}

fn upsert_cursor(
    connection: &Connection,
    record_id: &str,
    entry: &SidecarEntry,
) -> Result<(), SyncError> {
    connection
        .execute(
            "INSERT INTO sync_record_cursors (
                 record_id,
                 hlc,
                 state,
                 base_blob_hash
             ) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(record_id) DO UPDATE SET
                 hlc = excluded.hlc,
                 state = excluded.state,
                 base_blob_hash = excluded.base_blob_hash",
            params![
                record_id,
                &entry.hlc.to_sortable_bytes()[..],
                entry.state.as_db(),
                entry.base_blob_hash,
            ],
        )
        .map_err(|error| sql_error("upsert cursor", error))?;
    Ok(())
}

fn decode_hlc(bytes: &[u8]) -> Result<HlcTimestamp, SyncError> {
    let bytes: [u8; 20] = bytes
        .try_into()
        .map_err(|_| SyncError::Store("invalid sidecar HLC length".to_string()))?;
    Ok(HlcTimestamp::from_sortable_bytes(&bytes))
}

fn db_i64(value: u64) -> i64 {
    value.min(i64::MAX as u64) as i64
}

fn usize_from_i64(value: i64, field: &str) -> Result<usize, SyncError> {
    value
        .try_into()
        .map_err(|_| SyncError::Store(format!("invalid {field}: {value}")))
}

fn sql_error(context: &str, error: rusqlite::Error) -> SyncError {
    SyncError::Store(format!("sync sidecar {context}: {error}"))
}

fn cursor_domain(record_id: &str) -> String {
    record_id
        .split_once(':')
        .map(|(domain, _)| domain)
        .filter(|domain| !domain.is_empty())
        .unwrap_or("unknown")
        .to_string()
}
