use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use futures_lite::StreamExt;
use iroh_blobs::Hash;
use iroh_docs::api::Doc;
use iroh_docs::store::Query;
use iroh_docs::AuthorId;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};

use crate::sync_state::{SyncStateConnection, SyncStateConnectionGuard};
use crate::SyncError;

const BASE_RETRY_DELAY_MS: u64 = 1_000;
const MAX_RETRY_DELAY_MS: u64 = 60 * 60 * 1_000;
const REPAIR_SCAN_BATCH_SIZE: usize = 256;
const LAST_REPAIR_SCAN_MS_KEY: &str = "last_successful_repair_scan_ms";
const LAST_REPAIR_ATTEMPT_MS_KEY: &str = "last_repair_scan_attempt_ms";
const LAST_REPAIR_DURATION_MS_KEY: &str = "last_repair_scan_duration_ms";
const LAST_REPAIR_SCANNED_KEY: &str = "last_repair_scan_scanned";
const LAST_REPAIR_RECORDED_KEY: &str = "last_repair_scan_recorded";
const LAST_REPAIR_ALREADY_APPLIED_KEY: &str = "last_repair_scan_already_applied";
const LAST_REPAIR_ERRORS_KEY: &str = "last_repair_scan_errors";

pub const DAILY_REPAIR_SCAN_INTERVAL_MS: u64 = 24 * 60 * 60 * 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InboxState {
    Pending,
    Retryable,
    Dormant,
    Discarded,
    Applied,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryExhaustion {
    Failed,
    Dormant,
}

impl RetryExhaustion {
    fn inbox_state(self) -> InboxState {
        match self {
            Self::Failed => InboxState::Failed,
            Self::Dormant => InboxState::Dormant,
        }
    }
}

impl InboxState {
    fn as_db(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Retryable => "retryable",
            Self::Dormant => "dormant",
            Self::Discarded => "discarded",
            Self::Applied => "applied",
            Self::Failed => "failed",
        }
    }

    fn from_db(value: &str) -> Result<Self, SyncError> {
        match value {
            "pending" => Ok(Self::Pending),
            "retryable" => Ok(Self::Retryable),
            "dormant" => Ok(Self::Dormant),
            "discarded" => Ok(Self::Discarded),
            "applied" => Ok(Self::Applied),
            "failed" => Ok(Self::Failed),
            value => Err(SyncError::Store(format!(
                "invalid inbound inbox state: {value}"
            ))),
        }
    }
}

struct DbInboxEntry {
    author: Vec<u8>,
    key: Vec<u8>,
    content_hash: Vec<u8>,
    state: String,
    attempts: i64,
    seen_at_ms: i64,
    last_error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct InboxEntry {
    pub author: AuthorId,
    pub key: Vec<u8>,
    pub content_hash: String,
    pub state: InboxState,
    pub attempts: u32,
    pub seen_at_ms: u64,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct InboxStateCounts {
    pub pending: usize,
    pub retryable: usize,
    pub dormant: usize,
    pub discarded: usize,
    pub applied: usize,
    pub failed: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InboxErrorSample {
    pub author_hex: String,
    pub key_hex: String,
    pub state: InboxState,
    pub attempts: u32,
    pub updated_at_ms: u64,
    pub last_error: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct InboxDiagnostics {
    pub counts: InboxStateCounts,
    pub recent_errors: Vec<InboxErrorSample>,
    pub repair_scan: RepairScanDiagnostics,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepairScanDiagnostics {
    pub last_successful_at_ms: Option<u64>,
    pub next_due_at_ms: Option<u64>,
    pub last_attempt_at_ms: Option<u64>,
    pub last_duration_ms: Option<u64>,
    pub last_scanned: usize,
    pub last_recorded: usize,
    pub last_already_applied: usize,
    pub last_errors: usize,
    pub last_result: Option<RepairScanResult>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RepairScanResult {
    Success,
    Failure,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepairScanReport {
    pub scanned: usize,
    pub recorded: usize,
    pub already_applied: usize,
    pub errors: Vec<String>,
    pub duration_ms: u64,
}

pub struct InboundInbox {
    connection: SyncStateConnection,
}

impl InboundInbox {
    pub fn open(dir: &Path) -> Result<Self, SyncError> {
        let connection = SyncStateConnection::open(dir)?;
        Ok(Self { connection })
    }

    pub fn record_remote_insert(
        &self,
        author: AuthorId,
        key: &[u8],
        content_hash: Hash,
    ) -> Result<(), SyncError> {
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction()
            .map_err(|error| sql_error("begin record transaction", error))?;
        record_remote_insert_transaction(&transaction, author, key, content_hash, now_ms(), false)?;
        transaction
            .commit()
            .map_err(|error| sql_error("commit record transaction", error))?;
        Ok(())
    }

    pub fn record_remote_insert_for_repair(
        &self,
        author: AuthorId,
        key: &[u8],
        content_hash: Hash,
    ) -> Result<bool, SyncError> {
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction()
            .map_err(|error| sql_error("begin record transaction", error))?;
        let recorded = record_remote_insert_transaction(
            &transaction,
            author,
            key,
            content_hash,
            now_ms(),
            true,
        )?;
        transaction
            .commit()
            .map_err(|error| sql_error("commit record transaction", error))?;
        Ok(recorded)
    }

    pub fn mark_retryable(
        &self,
        author: AuthorId,
        key: &[u8],
        error: impl Into<String>,
        retry_after_ms: u64,
    ) -> Result<(), SyncError> {
        let connection = self.connection()?;
        connection
            .execute(
                "UPDATE inbound_inbox
                 SET state = ?3,
                     attempts = MIN(attempts + 1, 4294967295),
                     updated_at_ms = ?4,
                     retry_after_ms = ?5,
                     last_error = ?6
                 WHERE author = ?1 AND entry_key = ?2
                   AND state IN ('pending', 'retryable', 'dormant')",
                params![
                    &author.as_bytes()[..],
                    key,
                    InboxState::Retryable.as_db(),
                    db_i64(now_ms()),
                    db_i64(retry_after_ms),
                    error.into(),
                ],
            )
            .map_err(|error| sql_error("mark retryable", error))?;
        Ok(())
    }

    pub fn mark_retryable_limited(
        &self,
        author: AuthorId,
        key: &[u8],
        error: impl Into<String>,
        retry_after_ms: u64,
        max_attempts: u32,
        exhaustion: RetryExhaustion,
    ) -> Result<InboxState, SyncError> {
        let max_attempts = max_attempts.max(1);
        let exhausted_state = exhaustion.inbox_state();
        let connection = self.connection()?;
        connection
            .execute(
                "UPDATE inbound_inbox
                 SET state = CASE
                         WHEN attempts + 1 >= ?3 THEN ?4
                         ELSE ?5
                     END,
                     attempts = MIN(attempts + 1, 4294967295),
                     updated_at_ms = ?6,
                     retry_after_ms = CASE
                         WHEN attempts + 1 >= ?3 THEN NULL
                         ELSE ?7
                     END,
                     last_error = ?8
                 WHERE author = ?1 AND entry_key = ?2
                   AND state IN ('pending', 'retryable', 'dormant')",
                params![
                    &author.as_bytes()[..],
                    key,
                    i64::from(max_attempts),
                    exhausted_state.as_db(),
                    InboxState::Retryable.as_db(),
                    db_i64(now_ms()),
                    db_i64(retry_after_ms),
                    error.into(),
                ],
            )
            .map_err(|error| sql_error("mark retryable with limit", error))?;

        let state = connection
            .query_row(
                "SELECT state FROM inbound_inbox WHERE author = ?1 AND entry_key = ?2",
                params![&author.as_bytes()[..], key],
                |row| row.get::<_, String>(0),
            )
            .map_err(|error| sql_error("read limited retry state", error))?;
        InboxState::from_db(&state)
    }

    pub fn retry_failed(&self) -> Result<usize, SyncError> {
        let connection = self.connection()?;
        connection
            .execute(
                "UPDATE inbound_inbox
                 SET state = ?1,
                     attempts = 0,
                     updated_at_ms = ?2,
                     retry_after_ms = NULL,
                     last_error = NULL
                 WHERE state = 'failed'",
                params![InboxState::Pending.as_db(), db_i64(now_ms())],
            )
            .map_err(|error| sql_error("retry failed inbound entries", error))
    }

    pub fn mark_failed(
        &self,
        author: AuthorId,
        key: &[u8],
        error: impl Into<String>,
    ) -> Result<(), SyncError> {
        let connection = self.connection()?;
        connection
            .execute(
                "UPDATE inbound_inbox
                 SET state = ?3,
                     attempts = MIN(attempts + 1, 4294967295),
                     updated_at_ms = ?4,
                     retry_after_ms = NULL,
                     last_error = ?5
                 WHERE author = ?1 AND entry_key = ?2
                   AND state IN ('pending', 'retryable', 'dormant')",
                params![
                    &author.as_bytes()[..],
                    key,
                    InboxState::Failed.as_db(),
                    db_i64(now_ms()),
                    error.into(),
                ],
            )
            .map_err(|error| sql_error("mark failed", error))?;
        Ok(())
    }

    pub fn reactivate_dormant_content(
        &self,
        content_hash: iroh_blobs::Hash,
        now_ms: u64,
    ) -> Result<usize, SyncError> {
        let connection = self.connection()?;
        connection
            .execute(
                "UPDATE inbound_inbox
                 SET state = ?1,
                     attempts = 0,
                     updated_at_ms = ?2,
                     retry_after_ms = NULL,
                     last_error = NULL
                 WHERE state = 'dormant' AND content_hash = ?3",
                params![
                    InboxState::Pending.as_db(),
                    db_i64(now_ms),
                    content_hash.as_bytes().as_slice(),
                ],
            )
            .map_err(|error| sql_error("reactivate dormant ready content", error))
    }

    pub fn mark_applied(&self, author: AuthorId, key: &[u8]) -> Result<(), SyncError> {
        let connection = self.connection()?;
        connection
            .execute(
                "UPDATE inbound_inbox
                 SET state = ?3,
                     updated_at_ms = ?4,
                     retry_after_ms = NULL,
                     last_error = NULL
                 WHERE author = ?1 AND entry_key = ?2
                   AND state IN ('pending', 'retryable', 'dormant')",
                params![
                    &author.as_bytes()[..],
                    key,
                    InboxState::Applied.as_db(),
                    db_i64(now_ms()),
                ],
            )
            .map_err(|error| sql_error("mark applied", error))?;
        Ok(())
    }

    pub fn mark_discarded(
        &self,
        author: AuthorId,
        key: &[u8],
        error: impl Into<String>,
    ) -> Result<(), SyncError> {
        let connection = self.connection()?;
        connection
            .execute(
                "UPDATE inbound_inbox
                 SET state = ?3,
                     attempts = MIN(attempts + 1, 4294967295),
                     updated_at_ms = ?4,
                     retry_after_ms = NULL,
                     last_error = ?5
                 WHERE author = ?1 AND entry_key = ?2
                   AND state IN ('pending', 'retryable', 'dormant')",
                params![
                    &author.as_bytes()[..],
                    key,
                    InboxState::Discarded.as_db(),
                    db_i64(now_ms()),
                    error.into(),
                ],
            )
            .map_err(|error| sql_error("mark discarded", error))?;
        Ok(())
    }

    pub fn discard(&self, author: AuthorId, key: &[u8]) -> Result<bool, SyncError> {
        let connection = self.connection()?;
        let removed = connection
            .execute(
                "DELETE FROM inbound_inbox WHERE author = ?1 AND entry_key = ?2",
                params![&author.as_bytes()[..], key],
            )
            .map_err(|error| sql_error("discard entry", error))?;
        Ok(removed > 0)
    }

    pub fn mark_author_applied(&self, author: AuthorId) -> Result<usize, SyncError> {
        let connection = self.connection()?;
        connection
            .execute(
                "UPDATE inbound_inbox
                 SET state = ?2,
                     updated_at_ms = ?3,
                     retry_after_ms = NULL,
                     last_error = NULL
                 WHERE author = ?1
                   AND state IN ('pending', 'retryable', 'dormant')",
                params![
                    &author.as_bytes()[..],
                    InboxState::Applied.as_db(),
                    db_i64(now_ms()),
                ],
            )
            .map_err(|error| sql_error("mark author applied", error))
    }

    pub fn pending_ready(&self, now_ms: u64) -> Result<Vec<InboxEntry>, SyncError> {
        self.pending_ready_limited(now_ms, usize::MAX)
    }

    /// Next automatic drain deadline; terminal states never schedule retries.
    pub fn next_drain_delay(&self, now_ms: u64) -> Result<Option<std::time::Duration>, SyncError> {
        let connection = self.connection()?;
        let due: Option<i64> = connection
            .query_row(
                "SELECT MIN(CASE state WHEN 'pending' THEN 0 ELSE COALESCE(retry_after_ms, 0) END)
                 FROM inbound_inbox WHERE state IN ('pending', 'retryable')",
                [],
                |row| row.get(0),
            )
            .map_err(|error| sql_error("read next drain deadline", error))?;
        due.map(|due| {
            u64_from_i64(due, "inbound inbox drain deadline")
                .map(|due| std::time::Duration::from_millis(due.saturating_sub(now_ms)))
        })
        .transpose()
    }

    pub fn pending_ready_limited(
        &self,
        now_ms: u64,
        limit: usize,
    ) -> Result<Vec<InboxEntry>, SyncError> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT author, entry_key, content_hash, state, attempts, seen_at_ms, last_error
                 FROM inbound_inbox
                 WHERE state = 'pending'
                    OR (
                        state = 'retryable'
                        AND COALESCE(retry_after_ms, 0) <= ?1
                    )
                 ORDER BY
                    CASE state WHEN 'pending' THEN 0 ELSE 1 END,
                    attempts,
                    updated_at_ms,
                    author,
                    entry_key
                 LIMIT ?2",
            )
            .map_err(|error| sql_error("prepare ready query", error))?;
        let rows = statement
            .query_map(params![db_i64(now_ms), limit_i64(limit)], db_entry_from_row)
            .map_err(|error| sql_error("query ready entries", error))?;
        let mut entries = Vec::new();
        for row in rows {
            entries.push(to_public(
                row.map_err(|error| sql_error("read ready entry", error))?,
            )?);
        }
        Ok(entries)
    }

    pub fn snooze_retryable_backlog(
        &self,
        now_ms: u64,
        min_attempts: u32,
        delay_ms: u64,
        limit: usize,
    ) -> Result<usize, SyncError> {
        let retry_after_ms = now_ms.saturating_add(delay_ms);
        let connection = self.connection()?;
        connection
            .execute(
                "UPDATE inbound_inbox
                 SET retry_after_ms = ?1
                 WHERE rowid IN (
                     SELECT rowid
                     FROM inbound_inbox
                     WHERE state = 'retryable'
                       AND attempts >= ?2
                       AND COALESCE(retry_after_ms, 0) <= ?3
                     ORDER BY attempts, updated_at_ms, author, entry_key
                     LIMIT ?4
                 )",
                params![
                    db_i64(retry_after_ms),
                    i64::from(min_attempts),
                    db_i64(now_ms),
                    limit_i64(limit),
                ],
            )
            .map_err(|error| sql_error("snooze retryable backlog", error))
    }

    pub fn mark_dormant_retryable_backlog(
        &self,
        now_ms: u64,
        min_attempts: u32,
        limit: usize,
    ) -> Result<usize, SyncError> {
        let connection = self.connection()?;
        connection
            .execute(
                "UPDATE inbound_inbox
                 SET state = 'dormant',
                     updated_at_ms = ?1,
                     retry_after_ms = NULL
                 WHERE rowid IN (
                     SELECT rowid
                     FROM inbound_inbox
                     WHERE state = 'retryable'
                       AND attempts >= ?2
                       AND (
                           instr(last_error, 'entry content not readable yet') > 0
                           OR instr(last_error, 'read blob') > 0
                           OR instr(last_error, 'read_blob') > 0
                           OR instr(last_error, 'read replica log blob') > 0
                           OR instr(last_error, 'download encrypted blob') > 0
                       )
                     ORDER BY attempts, updated_at_ms, author, entry_key
                     LIMIT ?3
                 )",
                params![db_i64(now_ms), i64::from(min_attempts), limit_i64(limit),],
            )
            .map_err(|error| sql_error("mark dormant backlog", error))
    }

    pub fn count_by_state(&self, state: InboxState) -> Result<usize, SyncError> {
        let connection = self.connection()?;
        let count = connection
            .query_row(
                "SELECT COUNT(*) FROM inbound_inbox WHERE state = ?1",
                params![state.as_db()],
                |row| row.get::<_, i64>(0),
            )
            .map_err(|error| sql_error("count entries by state", error))?;
        usize_from_i64(count, "inbound inbox state count")
    }

    pub fn diagnostics(&self, recent_errors_limit: usize) -> Result<InboxDiagnostics, SyncError> {
        let Some(connection) = self.connection.try_lock()? else {
            return Ok(Default::default());
        };
        let mut counts = InboxStateCounts::default();
        {
            let mut statement = connection
                .prepare("SELECT state, COUNT(*) FROM inbound_inbox GROUP BY state")
                .map_err(|error| sql_error("prepare state counts", error))?;
            let rows = statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
                })
                .map_err(|error| sql_error("query state counts", error))?;
            for row in rows {
                let (state, count) = row.map_err(|error| sql_error("read state count", error))?;
                let count = usize_from_i64(count, "inbound inbox diagnostic count")?;
                match InboxState::from_db(&state)? {
                    InboxState::Pending => counts.pending = count,
                    InboxState::Retryable => counts.retryable = count,
                    InboxState::Dormant => counts.dormant = count,
                    InboxState::Discarded => counts.discarded = count,
                    InboxState::Applied => counts.applied = count,
                    InboxState::Failed => counts.failed = count,
                }
            }
        }

        let mut recent_errors = Vec::new();
        if recent_errors_limit > 0 {
            let mut statement = connection
                .prepare(
                    "SELECT author, entry_key, state, attempts, updated_at_ms, last_error
                     FROM inbound_inbox
                     WHERE last_error IS NOT NULL
                     ORDER BY updated_at_ms DESC, author, entry_key
                     LIMIT ?1",
                )
                .map_err(|error| sql_error("prepare recent errors", error))?;
            let rows = statement
                .query_map(params![limit_i64(recent_errors_limit)], |row| {
                    Ok((
                        row.get::<_, Vec<u8>>(0)?,
                        row.get::<_, Vec<u8>>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, String>(5)?,
                    ))
                })
                .map_err(|error| sql_error("query recent errors", error))?;
            for row in rows {
                let (author, key, state, attempts, updated_at_ms, last_error) =
                    row.map_err(|error| sql_error("read recent error", error))?;
                recent_errors.push(InboxErrorSample {
                    author_hex: hex::encode(author),
                    key_hex: hex::encode(key),
                    state: InboxState::from_db(&state)?,
                    attempts: u32_from_i64(attempts, "inbound inbox attempts")?,
                    updated_at_ms: u64_from_i64(updated_at_ms, "inbound inbox updated_at_ms")?,
                    last_error,
                });
            }
        }

        Ok(InboxDiagnostics {
            counts,
            recent_errors,
            repair_scan: repair_scan_diagnostics(&connection)?,
        })
    }

    pub fn last_successful_repair_scan_ms(&self) -> Result<Option<u64>, SyncError> {
        let connection = self.connection()?;
        let value = metadata_value(&connection, LAST_REPAIR_SCAN_MS_KEY)?;
        value
            .map(|value| u64_from_i64(value, "last successful Repair Scan time"))
            .transpose()
    }

    pub fn needs_initial_repair_scan(&self) -> Result<bool, SyncError> {
        Ok(self.last_successful_repair_scan_ms()?.is_none())
    }

    pub fn repair_scan_due(&self, now_ms: u64) -> Result<bool, SyncError> {
        Ok(self
            .last_successful_repair_scan_ms()?
            .is_none_or(|last| now_ms.saturating_sub(last) >= DAILY_REPAIR_SCAN_INTERVAL_MS))
    }

    pub async fn repair_scan_doc_entries(&self, doc: &Doc) -> Result<RepairScanReport, SyncError> {
        let started = std::time::Instant::now();
        let query = Query::all().include_empty().build();
        let stream = doc
            .get_many(query)
            .await
            .map_err(|e| SyncError::Store(format!("repair scan get_many: {e}")))?;
        tokio::pin!(stream);

        let mut report = RepairScanReport::default();
        let mut batch = Vec::with_capacity(REPAIR_SCAN_BATCH_SIZE);
        while let Some(result) = stream.next().await {
            report.scanned += 1;
            let entry = match result {
                Ok(entry) => entry,
                Err(e) => {
                    report.errors.push(format!("entry read error: {e}"));
                    continue;
                }
            };
            batch.push((entry.author(), entry.key().to_vec(), entry.content_hash()));
            if batch.len() == REPAIR_SCAN_BATCH_SIZE {
                self.flush_repair_batch(&mut batch, &mut report);
            }
        }
        if !batch.is_empty() {
            self.flush_repair_batch(&mut batch, &mut report);
        }
        report.duration_ms = started.elapsed().as_millis() as u64;
        self.record_repair_scan_attempt(&report, now_ms())?;
        Ok(report)
    }

    fn flush_repair_batch(
        &self,
        batch: &mut Vec<(AuthorId, Vec<u8>, Hash)>,
        report: &mut RepairScanReport,
    ) {
        match self.record_repair_batch(batch) {
            Ok((recorded, already_applied)) => {
                report.recorded += recorded;
                report.already_applied += already_applied;
            }
            Err(error) => report.errors.push(format!("record inbox batch: {error}")),
        }
        batch.clear();
    }

    fn record_repair_batch(
        &self,
        batch: &[(AuthorId, Vec<u8>, Hash)],
    ) -> Result<(usize, usize), SyncError> {
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction()
            .map_err(|error| sql_error("begin Repair Scan batch", error))?;
        let now = now_ms();
        let mut recorded = 0usize;
        for (author, key, content_hash) in batch {
            if record_remote_insert_transaction(
                &transaction,
                *author,
                key,
                *content_hash,
                now,
                true,
            )? {
                recorded += 1;
            }
        }
        transaction
            .commit()
            .map_err(|error| sql_error("commit Repair Scan batch", error))?;
        Ok((recorded, batch.len().saturating_sub(recorded)))
    }

    fn record_repair_scan_attempt(
        &self,
        report: &RepairScanReport,
        completed_at_ms: u64,
    ) -> Result<(), SyncError> {
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction()
            .map_err(|error| sql_error("begin Repair Scan diagnostics", error))?;
        upsert_metadata(
            &transaction,
            LAST_REPAIR_ATTEMPT_MS_KEY,
            db_i64(completed_at_ms),
        )?;
        upsert_metadata(
            &transaction,
            LAST_REPAIR_DURATION_MS_KEY,
            db_i64(report.duration_ms),
        )?;
        upsert_metadata(
            &transaction,
            LAST_REPAIR_SCANNED_KEY,
            limit_i64(report.scanned),
        )?;
        upsert_metadata(
            &transaction,
            LAST_REPAIR_RECORDED_KEY,
            limit_i64(report.recorded),
        )?;
        upsert_metadata(
            &transaction,
            LAST_REPAIR_ALREADY_APPLIED_KEY,
            limit_i64(report.already_applied),
        )?;
        upsert_metadata(
            &transaction,
            LAST_REPAIR_ERRORS_KEY,
            limit_i64(report.errors.len()),
        )?;
        if report.errors.is_empty() {
            upsert_metadata(
                &transaction,
                LAST_REPAIR_SCAN_MS_KEY,
                db_i64(completed_at_ms),
            )?;
        }
        transaction
            .commit()
            .map_err(|error| sql_error("commit Repair Scan diagnostics", error))?;
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

fn record_remote_insert_transaction(
    transaction: &Transaction<'_>,
    author: AuthorId,
    key: &[u8],
    content_hash: Hash,
    now_ms: u64,
    reactivate_dormant: bool,
) -> Result<bool, SyncError> {
    let author_bytes = &author.as_bytes()[..];
    let content_hash_bytes = &content_hash.as_bytes()[..];
    if content_hash == Hash::EMPTY {
        let changed = transaction.execute(
            "INSERT INTO inbound_inbox (author, entry_key, content_hash, state, attempts, seen_at_ms, updated_at_ms, retry_after_ms, last_error)
             VALUES (?1, ?2, ?3, 'discarded', 0, ?4, ?4, NULL, NULL)
             ON CONFLICT(author, entry_key) DO UPDATE SET content_hash=excluded.content_hash,
                 state='discarded', attempts=0, updated_at_ms=excluded.updated_at_ms,
                 retry_after_ms=NULL, last_error=NULL
             WHERE inbound_inbox.content_hash != excluded.content_hash OR inbound_inbox.state != 'discarded'",
            params![author_bytes, key, content_hash_bytes, db_i64(now_ms)],
        ).map_err(|error| sql_error("record replica deletion marker", error))?;
        return Ok(changed > 0);
    }
    let existing = transaction
        .query_row(
            "SELECT content_hash, state
             FROM inbound_inbox
             WHERE author = ?1 AND entry_key = ?2",
            params![author_bytes, key],
            |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(|error| sql_error("read existing entry", error))?;

    match existing {
        Some((existing_hash, state)) if existing_hash.as_slice() == content_hash_bytes => {
            if reactivate_dormant && InboxState::from_db(&state)? == InboxState::Dormant {
                transaction
                    .execute(
                        "UPDATE inbound_inbox
                         SET state = ?3,
                             attempts = 0,
                             updated_at_ms = ?4,
                             retry_after_ms = NULL,
                             last_error = NULL
                         WHERE author = ?1 AND entry_key = ?2",
                        params![
                            author_bytes,
                            key,
                            InboxState::Pending.as_db(),
                            db_i64(now_ms),
                        ],
                    )
                    .map_err(|error| sql_error("reactivate dormant repair entry", error))?;
                Ok(true)
            } else {
                Ok(false)
            }
        }
        Some(_) => {
            transaction
                .execute(
                    "UPDATE inbound_inbox
                     SET content_hash = ?3,
                         state = ?4,
                         attempts = 0,
                         seen_at_ms = ?5,
                         updated_at_ms = ?5,
                         retry_after_ms = NULL,
                         last_error = NULL
                     WHERE author = ?1 AND entry_key = ?2",
                    params![
                        author_bytes,
                        key,
                        content_hash_bytes,
                        InboxState::Pending.as_db(),
                        db_i64(now_ms),
                    ],
                )
                .map_err(|error| sql_error("replace entry version", error))?;
            Ok(true)
        }
        None => {
            transaction
                .execute(
                    "INSERT INTO inbound_inbox (
                         author,
                         entry_key,
                         content_hash,
                         state,
                         attempts,
                         seen_at_ms,
                         updated_at_ms,
                         retry_after_ms,
                         last_error
                     ) VALUES (?1, ?2, ?3, ?4, 0, ?5, ?5, NULL, NULL)",
                    params![
                        author_bytes,
                        key,
                        content_hash_bytes,
                        InboxState::Pending.as_db(),
                        db_i64(now_ms),
                    ],
                )
                .map_err(|error| sql_error("insert entry", error))?;
            Ok(true)
        }
    }
}

fn repair_scan_diagnostics(connection: &Connection) -> Result<RepairScanDiagnostics, SyncError> {
    let last_successful_at_ms = metadata_value(connection, LAST_REPAIR_SCAN_MS_KEY)?
        .map(|value| u64_from_i64(value, "last successful Repair Scan time"))
        .transpose()?;
    let last_attempt_at_ms = metadata_value(connection, LAST_REPAIR_ATTEMPT_MS_KEY)?
        .map(|value| u64_from_i64(value, "last Repair Scan attempt time"))
        .transpose()?;
    let last_errors = metadata_value(connection, LAST_REPAIR_ERRORS_KEY)?
        .map(|value| usize_from_i64(value, "last Repair Scan error count"))
        .transpose()?
        .unwrap_or_default();
    Ok(RepairScanDiagnostics {
        last_successful_at_ms,
        next_due_at_ms: last_successful_at_ms
            .map(|last| last.saturating_add(DAILY_REPAIR_SCAN_INTERVAL_MS)),
        last_attempt_at_ms,
        last_duration_ms: metadata_value(connection, LAST_REPAIR_DURATION_MS_KEY)?
            .map(|value| u64_from_i64(value, "last Repair Scan duration"))
            .transpose()?,
        last_scanned: metadata_value(connection, LAST_REPAIR_SCANNED_KEY)?
            .map(|value| usize_from_i64(value, "last Repair Scan scanned count"))
            .transpose()?
            .unwrap_or_default(),
        last_recorded: metadata_value(connection, LAST_REPAIR_RECORDED_KEY)?
            .map(|value| usize_from_i64(value, "last Repair Scan recorded count"))
            .transpose()?
            .unwrap_or_default(),
        last_already_applied: metadata_value(connection, LAST_REPAIR_ALREADY_APPLIED_KEY)?
            .map(|value| usize_from_i64(value, "last Repair Scan already applied count"))
            .transpose()?
            .unwrap_or_default(),
        last_errors,
        last_result: last_attempt_at_ms.map(|_| {
            if last_errors == 0 {
                RepairScanResult::Success
            } else {
                RepairScanResult::Failure
            }
        }),
    })
}

fn metadata_value(connection: &Connection, key: &str) -> Result<Option<i64>, SyncError> {
    connection
        .query_row(
            "SELECT integer_value FROM sync_metadata WHERE key = ?1",
            params![key],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(|error| sql_error("read metadata", error))
}

fn upsert_metadata(transaction: &Transaction<'_>, key: &str, value: i64) -> Result<(), SyncError> {
    transaction
        .execute(
            "INSERT INTO sync_metadata (key, integer_value)
             VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET
                 integer_value = excluded.integer_value",
            params![key, value],
        )
        .map_err(|error| sql_error("write metadata", error))?;
    Ok(())
}

fn db_entry_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<DbInboxEntry> {
    Ok(DbInboxEntry {
        author: row.get(0)?,
        key: row.get(1)?,
        content_hash: row.get(2)?,
        state: row.get(3)?,
        attempts: row.get(4)?,
        seen_at_ms: row.get(5)?,
        last_error: row.get(6)?,
    })
}

fn to_public(stored: DbInboxEntry) -> Result<InboxEntry, SyncError> {
    let author_bytes = decode_32(&stored.author, "inbound inbox author")?;
    let content_hash = Hash::from_bytes(decode_32(
        &stored.content_hash,
        "inbound inbox content hash",
    )?);
    Ok(InboxEntry {
        author: AuthorId::from(&author_bytes),
        key: stored.key,
        content_hash: content_hash.to_string(),
        state: InboxState::from_db(&stored.state)?,
        attempts: u32_from_i64(stored.attempts, "inbound inbox attempts")?,
        seen_at_ms: u64_from_i64(stored.seen_at_ms, "inbound inbox seen_at_ms")?,
        last_error: stored.last_error,
    })
}

fn decode_32(bytes: &[u8], field: &str) -> Result<[u8; 32], SyncError> {
    bytes
        .try_into()
        .map_err(|_| SyncError::Store(format!("{field} length")))
}

fn db_i64(value: u64) -> i64 {
    value.min(i64::MAX as u64) as i64
}

fn limit_i64(value: usize) -> i64 {
    value.min(i64::MAX as usize) as i64
}

fn u32_from_i64(value: i64, field: &str) -> Result<u32, SyncError> {
    value
        .try_into()
        .map_err(|_| SyncError::Store(format!("invalid {field}: {value}")))
}

fn u64_from_i64(value: i64, field: &str) -> Result<u64, SyncError> {
    value
        .try_into()
        .map_err(|_| SyncError::Store(format!("invalid {field}: {value}")))
}

fn usize_from_i64(value: i64, field: &str) -> Result<usize, SyncError> {
    value
        .try_into()
        .map_err(|_| SyncError::Store(format!("invalid {field}: {value}")))
}

fn sql_error(context: &str, error: rusqlite::Error) -> SyncError {
    SyncError::Store(format!("inbound inbox {context}: {error}"))
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub fn next_retry_after_ms(attempts: u32, now_ms: u64) -> u64 {
    let exponent = attempts.min(12);
    let delay = BASE_RETRY_DELAY_MS
        .saturating_mul(1_u64 << exponent)
        .min(MAX_RETRY_DELAY_MS);
    now_ms.saturating_add(delay)
}
