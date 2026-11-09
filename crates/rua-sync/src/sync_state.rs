use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use rusqlite::Connection;

use crate::SyncError;

const DATABASE_FILE: &str = "sync-state.sqlite";
const SCHEMA_VERSION: i64 = 2;

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
