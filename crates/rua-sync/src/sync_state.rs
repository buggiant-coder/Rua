use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use rusqlite::{params, Connection, OptionalExtension};

use crate::SyncError;

const DATABASE_FILE: &str = "sync-state.sqlite";
const SCHEMA_VERSION: i64 = 2;

/// Bind rebuildable bookkeeping to the network whose ciphertext it describes.
/// Call only after the previous consumer has stopped, before attaching a new one.
pub(crate) fn bind_network(dir: &Path, namespace: &[u8; 32]) -> Result<(), SyncError> {
    let mut connection = open_connection(dir)?;
    let transaction = connection
        .transaction()
        .map_err(|error| sql_error("begin network binding", error))?;
    transaction
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS sync_network_binding (
            singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
            namespace BLOB NOT NULL CHECK(length(namespace) = 32)
        );",
        )
        .map_err(|error| sql_error("create network binding", error))?;
    let previous: Option<Vec<u8>> = transaction
        .query_row(
            "SELECT namespace FROM sync_network_binding WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| sql_error("read network binding", error))?;
    if previous.as_deref() != Some(namespace.as_slice()) {
        if previous.is_none() {
            // Unbound versions have no trustworthy network provenance. Keep
            // their full facts for recovery, outside the active ordering and
            // deduplication tables. Current-network replay rebuilds those.
            // Separate row ids retain every copy if binding metadata is lost
            // again; an old op id must never overwrite quarantined history.
            transaction
                .execute_batch(
                    "CREATE TABLE IF NOT EXISTS sync_legacy_unbound_applied_ops (
                         quarantine_id INTEGER PRIMARY KEY,
                         op_id TEXT NOT NULL,
                         domain TEXT NOT NULL,
                         record_id TEXT NOT NULL,
                         change_fingerprint TEXT
                     );
                     CREATE TABLE IF NOT EXISTS sync_legacy_unbound_record_cursors (
                         quarantine_id INTEGER PRIMARY KEY,
                         record_id TEXT NOT NULL,
                         hlc BLOB NOT NULL CHECK(length(hlc) = 20),
                         state TEXT NOT NULL CHECK(state IN ('live', 'tombstoned')),
                         base_blob_hash TEXT
                     );
                     INSERT INTO sync_legacy_unbound_applied_ops
                         (op_id, domain, record_id, change_fingerprint)
                         SELECT op_id, domain, record_id, change_fingerprint
                         FROM sync_applied_ops;
                     INSERT INTO sync_legacy_unbound_record_cursors
                         (record_id, hlc, state, base_blob_hash)
                         SELECT record_id, hlc, state, base_blob_hash
                         FROM sync_record_cursors;",
                )
                .map_err(|error| sql_error("quarantine unbound network versions", error))?;
        }
        transaction
            .execute_batch(
                "DELETE FROM inbound_inbox;
                 DELETE FROM sync_metadata;
                 DELETE FROM sync_applied_ops;
                 DELETE FROM sync_record_cursors;",
            )
            .map_err(|error| sql_error("reset previous network bookkeeping", error))?;
        transaction
            .execute(
                "INSERT OR REPLACE INTO sync_network_binding(singleton, namespace) VALUES(1, ?1)",
                [namespace.as_slice()],
            )
            .map_err(|error| sql_error("write network binding", error))?;
    }
    transaction
        .commit()
        .map_err(|error| sql_error("commit network binding", error))
}

/// Return only potential tombstone records; their provenance remains unproven
/// until the current namespace's own encrypted entry is read and verified.
pub(crate) fn quarantined_tombstone_records(
    dir: &Path,
) -> Result<Vec<(String, String)>, SyncError> {
    let connection = open_connection(dir)?;
    let has_legacy_tables = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table'
             AND name IN ('sync_legacy_unbound_applied_ops',
                 'sync_legacy_unbound_record_cursors')",
            [],
            |row| row.get::<_, u32>(0),
        )
        .map_err(|error| sql_error("inspect quarantined versions", error))?;
    if has_legacy_tables == 0 {
        return Ok(Vec::new());
    }
    let mut statement = connection
        .prepare(
            "SELECT DISTINCT operation.domain, operation.record_id
             FROM sync_legacy_unbound_applied_ops AS operation
             JOIN sync_legacy_unbound_record_cursors AS cursor
                 ON operation.record_id = cursor.record_id
             WHERE cursor.state = 'tombstoned'
                 AND operation.change_fingerprint IS NOT NULL",
        )
        .map_err(|error| sql_error("prepare quarantined tombstones", error))?;
    let records = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|error| sql_error("query quarantined tombstones", error))?;
    let mut candidates = Vec::new();
    for record in records {
        let (domain, qualified_record_id) =
            record.map_err(|error| sql_error("read quarantined tombstone", error))?;
        if let Some(record_id) = qualified_record_id.strip_prefix(&format!("{domain}:")) {
            candidates.push((domain, record_id.to_owned()));
        }
    }
    Ok(candidates)
}

/// The caller must authenticate this change as an own-author entry in the
/// currently bound namespace. Restore ordering only when both that envelope
/// and the quarantined apply facts prove the same explicit removal.
pub(crate) fn restore_verified_removal(
    dir: &Path,
    namespace: &[u8; 32],
    change: &crate::opaque_replica_log::SyncChange,
) -> Result<bool, SyncError> {
    if !crate::sync_registry::is_explicit_removal(change) {
        return Ok(false);
    }
    let fingerprint = crate::sync_registry::change_fingerprint(change)?;
    let record_id = crate::sync_registry::sync_cursor_record_id(&change.domain, &change.record_id);
    let mut connection = open_connection(dir)?;
    let transaction = connection
        .transaction()
        .map_err(|error| sql_error("begin proven removal recovery", error))?;
    let bound: Vec<u8> = transaction
        .query_row(
            "SELECT namespace FROM sync_network_binding WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .map_err(|error| sql_error("read removal recovery binding", error))?;
    if bound.as_slice() != namespace.as_slice() {
        return Err(SyncError::Store(
            "sync namespace changed during proven removal recovery".into(),
        ));
    }
    let proven: bool = transaction
        .query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM sync_legacy_unbound_applied_ops AS operation
                 JOIN sync_legacy_unbound_record_cursors AS cursor
                     ON operation.record_id = cursor.record_id
                 WHERE operation.op_id = ?1 AND operation.domain = ?2
                     AND operation.record_id = ?3 AND operation.change_fingerprint = ?4
                     AND cursor.hlc = ?5 AND cursor.state = 'tombstoned'
             )",
            params![
                &change.op_id,
                &change.domain,
                &record_id,
                &fingerprint,
                &change.hlc.to_sortable_bytes()[..]
            ],
            |row| row.get(0),
        )
        .map_err(|error| sql_error("verify quarantined removal", error))?;
    if !proven {
        return Ok(false);
    }
    let applied: Option<Option<String>> = transaction
        .query_row(
            "SELECT change_fingerprint FROM sync_applied_ops WHERE op_id = ?1",
            [&change.op_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| sql_error("read recovered operation", error))?;
    if applied
        .flatten()
        .is_some_and(|existing| existing != fingerprint)
    {
        return Err(SyncError::InvalidPayload(format!(
            "divergent duplicate operation: {}",
            change.op_id
        )));
    }
    transaction
        .execute(
            "INSERT OR IGNORE INTO sync_applied_ops
                 (op_id, domain, record_id, change_fingerprint) VALUES (?1, ?2, ?3, ?4)",
            params![&change.op_id, &change.domain, &record_id, &fingerprint],
        )
        .map_err(|error| sql_error("restore proven removal operation", error))?;
    transaction
        .execute(
            "INSERT INTO sync_record_cursors(record_id, hlc, state, base_blob_hash)
                 VALUES (?1, ?2, 'tombstoned', NULL)
             ON CONFLICT(record_id) DO UPDATE
                 SET hlc = excluded.hlc, state = 'tombstoned', base_blob_hash = NULL
                 WHERE sync_record_cursors.hlc <= excluded.hlc",
            params![&record_id, &change.hlc.to_sortable_bytes()[..]],
        )
        .map_err(|error| sql_error("restore proven removal cursor", error))?;
    transaction
        .commit()
        .map_err(|error| sql_error("commit proven removal recovery", error))?;
    Ok(true)
}

// Store handles may outlive an enabled Sync runtime. Closing releases the file;
// only an explicit reopen may bind the handle to the current directory again.
pub(crate) struct SyncStateConnection {
    dir: PathBuf,
    connection: Mutex<Option<Connection>>,
}

impl SyncStateConnection {
    pub(crate) fn open(dir: &Path) -> Result<Self, SyncError> {
        Ok(Self {
            dir: dir.to_path_buf(),
            connection: Mutex::new(Some(open_connection(dir)?)),
        })
    }

    pub(crate) fn close(&self) -> Result<(), SyncError> {
        self.guard()?.take();
        Ok(())
    }

    pub(crate) fn reopen(&self) -> Result<(), SyncError> {
        let mut guard = self.guard()?;
        guard.take();
        *guard = Some(open_connection(&self.dir)?);
        Ok(())
    }

    fn guard(&self) -> Result<MutexGuard<'_, Option<Connection>>, SyncError> {
        self.connection
            .lock()
            .map_err(|_| SyncError::Store("sync state lock poisoned".into()))
    }

    pub(crate) fn try_lock(&self) -> Result<Option<SyncStateConnectionGuard<'_>>, SyncError> {
        let guard = self.guard()?;
        if guard.is_none() {
            return Ok(None);
        }
        Ok(Some(SyncStateConnectionGuard(guard)))
    }

    pub(crate) fn lock(&self) -> Result<SyncStateConnectionGuard<'_>, SyncError> {
        self.try_lock()?
            .ok_or_else(|| SyncError::Store("sync state database is closed".into()))
    }
}

pub(crate) struct SyncStateConnectionGuard<'a>(MutexGuard<'a, Option<Connection>>);

impl Deref for SyncStateConnectionGuard<'_> {
    type Target = Connection;

    fn deref(&self) -> &Connection {
        self.0
            .as_ref()
            .expect("connection guard holds an open connection")
    }
}

impl DerefMut for SyncStateConnectionGuard<'_> {
    fn deref_mut(&mut self) -> &mut Connection {
        self.0
            .as_mut()
            .expect("connection guard holds an open connection")
    }
}

pub(crate) fn open_connection(dir: &Path) -> Result<Connection, SyncError> {
    std::fs::create_dir_all(dir)?;
    let connection = Connection::open(dir.join(DATABASE_FILE))
        .map_err(|error| sql_error("open database", error))?;
    connection
        .execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = FULL;
             PRAGMA foreign_keys = ON;
             PRAGMA busy_timeout = 5000;",
        )
        .map_err(|error| sql_error("configure database", error))?;
    initialize_schema(&connection)?;
    Ok(connection)
}

fn initialize_schema(connection: &Connection) -> Result<(), SyncError> {
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS inbound_inbox (
                 author BLOB NOT NULL CHECK(length(author) = 32),
                 entry_key BLOB NOT NULL,
                 content_hash BLOB NOT NULL CHECK(length(content_hash) = 32),
                 state TEXT NOT NULL CHECK(
                     state IN (
                         'pending',
                         'retryable',
                         'dormant',
                         'discarded',
                         'applied',
                         'failed'
                     )
                 ),
                 attempts INTEGER NOT NULL,
                 seen_at_ms INTEGER NOT NULL,
                 updated_at_ms INTEGER NOT NULL,
                 retry_after_ms INTEGER,
                 last_error TEXT,
                 PRIMARY KEY (author, entry_key)
             );
             CREATE INDEX IF NOT EXISTS inbound_inbox_ready
                 ON inbound_inbox(state, retry_after_ms, attempts, updated_at_ms);
             CREATE INDEX IF NOT EXISTS inbound_inbox_author_state
                 ON inbound_inbox(author, state);
             CREATE INDEX IF NOT EXISTS inbound_inbox_recent_errors
                 ON inbound_inbox(updated_at_ms DESC)
                 WHERE last_error IS NOT NULL;

             CREATE TABLE IF NOT EXISTS sync_applied_ops (
                 op_id TEXT PRIMARY KEY,
                 domain TEXT NOT NULL,
                 record_id TEXT NOT NULL,
                 change_fingerprint TEXT
             ) WITHOUT ROWID;
             CREATE INDEX IF NOT EXISTS sync_applied_ops_domain
                 ON sync_applied_ops(domain);

             CREATE TABLE IF NOT EXISTS sync_record_cursors (
                 record_id TEXT PRIMARY KEY,
                 hlc BLOB NOT NULL CHECK(length(hlc) = 20),
                 state TEXT NOT NULL CHECK(state IN ('live', 'tombstoned')),
                 base_blob_hash TEXT
             ) WITHOUT ROWID;

             CREATE TABLE IF NOT EXISTS sync_metadata (
                 key TEXT PRIMARY KEY,
                 integer_value INTEGER
             ) WITHOUT ROWID;",
        )
        .map_err(|error| sql_error("initialize schema", error))?;
    let version = connection
        .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
        .map_err(|error| sql_error("read schema version", error))?;
    if version < 2 {
        let has_change_fingerprint = {
            let mut statement = connection
                .prepare("PRAGMA table_info(sync_applied_ops)")
                .map_err(|error| sql_error("inspect applied op schema", error))?;
            let columns = statement
                .query_map([], |row| row.get::<_, String>(1))
                .map_err(|error| sql_error("read applied op schema", error))?;
            let mut found = false;
            for column in columns {
                if column.map_err(|error| sql_error("read applied op column", error))?
                    == "change_fingerprint"
                {
                    found = true;
                    break;
                }
            }
            found
        };
        if !has_change_fingerprint {
            connection
                .execute(
                    "ALTER TABLE sync_applied_ops ADD COLUMN change_fingerprint TEXT",
                    [],
                )
                .map_err(|error| sql_error("add applied op fingerprint", error))?;
        }
        connection
            .pragma_update(None, "user_version", SCHEMA_VERSION)
            .map_err(|error| sql_error("write schema version", error))?;
    }
    Ok(())
}

fn sql_error(context: &str, error: rusqlite::Error) -> SyncError {
    SyncError::Store(format!("sync state {context}: {error}"))
}
