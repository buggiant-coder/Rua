use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::ChatTag;

const COLUMNS: &str = "id, name, user_id, channel, created_at, updated_at, \
    meta_json, status, pinned, pin_order, tag, archived";

#[derive(Debug, Clone, PartialEq)]
pub struct ChatIndexRecord {
    pub id: String,
    pub name: String,
    pub user_id: String,
    pub channel: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub meta: serde_json::Value,
    pub status: String,
    pub pinned: bool,
    pub pin_order: i64,
    pub tag: ChatTag,
    pub archived: bool,
}

#[derive(Debug, Clone, Default)]
pub struct ChatIndexFilter {
    pub user_id: Option<String>,
    pub channel: Option<String>,
    pub search: Option<String>,
    pub tag: Option<ChatTag>,
    pub conversation_view: ChatConversationView,
    pub source_chat_id: Option<String>,
    pub executor: Option<String>,
    pub status: Option<String>,
    pub sort_by: ChatSortField,
    pub sort_order: SortOrder,
    pub archived: Option<bool>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatConversationView {
    User,
    Collaboration,
    #[default]
    All,
}

impl ChatConversationView {
    fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Collaboration => "collaboration",
            Self::All => "all",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatSortField {
    Title,
    CreatedAt,
    #[default]
    UpdatedAt,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SortOrder {
    Asc,
    #[default]
    Desc,
}

impl ChatSortField {
    fn sql_column(self) -> &'static str {
        match self {
            Self::Title => "name COLLATE NOCASE",
            Self::CreatedAt => "created_at",
            Self::UpdatedAt => "updated_at",
        }
    }
}

impl SortOrder {
    fn sql_keyword(self) -> &'static str {
        match self {
            Self::Asc => "ASC",
            Self::Desc => "DESC",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ChatIndexPage {
    pub entries: Vec<ChatIndexRecord>,
    pub total: u64,
}

fn db_path(workspace_dir: &Path) -> PathBuf {
    workspace_dir.join("chat-index.sqlite")
}

fn open_connection(workspace_dir: &Path) -> Result<Connection, String> {
    std::fs::create_dir_all(workspace_dir).map_err(|error| {
        format!(
            "create Chat Index directory {}: {error}",
            workspace_dir.display()
        )
    })?;
    let path = db_path(
        &std::fs::canonicalize(workspace_dir)
            .map_err(|error| format!("resolve Chat Index directory: {error}"))?,
    );
    let initialization = initialization_lock(&path)?;
    let _guard = initialization
        .lock()
        .map_err(|_| "Chat Index initialization lock poisoned".to_string())?;
    let connection = Connection::open(path).map_err(|error| format!("open Chat Index: {error}"))?;
    connection
        .pragma_update(None, "busy_timeout", 5000_i64)
        .map_err(|error| format!("set Chat Index busy timeout: {error}"))?;
    let mode: String = connection
        .pragma_query_value(None, "journal_mode", |row| row.get(0))
        .map_err(|error| format!("read Chat Index journal mode: {error}"))?;
    if !mode.eq_ignore_ascii_case("wal") {
        connection
            .pragma_update(None, "journal_mode", "WAL")
            .map_err(|error| format!("set Chat Index WAL mode: {error}"))?;
    }
    connection
        .pragma_update(None, "synchronous", "NORMAL")
        .map_err(|error| format!("set Chat Index synchronous mode: {error}"))?;
    initialize_schema(&connection)?;
    Ok(connection)
}

fn initialization_lock(path: &Path) -> Result<Arc<Mutex<()>>, String> {
    static LOCKS: OnceLock<Mutex<HashMap<PathBuf, Weak<Mutex<()>>>>> = OnceLock::new();
    let mut locks = LOCKS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .map_err(|_| "Chat Index initialization registry poisoned".to_string())?;
    locks.retain(|_, lock| lock.strong_count() > 0);
    if let Some(lock) = locks.get(path).and_then(Weak::upgrade) {
        return Ok(lock);
    }
    let lock = Arc::new(Mutex::new(()));
    locks.insert(path.to_path_buf(), Arc::downgrade(&lock));
    Ok(lock)
}

fn initialize_schema(connection: &Connection) -> Result<(), String> {
    connection
        .execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS chat_index (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                user_id TEXT NOT NULL,
                channel TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                meta_json TEXT NOT NULL,
                status TEXT NOT NULL,
                pinned INTEGER NOT NULL DEFAULT 0,
                pin_order INTEGER NOT NULL DEFAULT 0,
                tag TEXT NOT NULL DEFAULT 'none'
            );
            "#,
        )
        .map_err(|error| format!("initialize Chat Index table: {error}"))?;

    if table_has_column(connection, "session_id")? {
        connection
            .execute_batch(
                r#"
                BEGIN IMMEDIATE;
                ALTER TABLE chat_index RENAME TO chat_index_with_session_id;
                DROP INDEX IF EXISTS idx_chat_index_listing;
                DROP INDEX IF EXISTS idx_chat_index_user_channel;
                DROP INDEX IF EXISTS idx_chat_index_session_id;
                CREATE TABLE chat_index (
                    id TEXT PRIMARY KEY,
                    name TEXT NOT NULL,
                    user_id TEXT NOT NULL,
                    channel TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    meta_json TEXT NOT NULL,
                    status TEXT NOT NULL,
                    pinned INTEGER NOT NULL DEFAULT 0,
                    pin_order INTEGER NOT NULL DEFAULT 0,
                    tag TEXT NOT NULL DEFAULT 'none'
                );
                INSERT INTO chat_index (
                    id, name, user_id, channel, created_at, updated_at,
                    meta_json, status, pinned, pin_order
                )
                SELECT
                    id, name, user_id, channel, created_at, updated_at,
                    meta_json, status, pinned, pin_order
                FROM chat_index_with_session_id;
                DROP TABLE chat_index_with_session_id;
                COMMIT;
                "#,
            )
            .map_err(|error| format!("migrate Chat Index to chat_id-only schema: {error}"))?;
    }

    if !table_has_column(connection, "tag")? {
        connection
            .execute(
                "ALTER TABLE chat_index ADD COLUMN tag TEXT NOT NULL DEFAULT 'none'",
                [],
            )
            .map_err(|error| format!("add Chat Index tag column: {error}"))?;
    }

    if !table_has_column(connection, "archived")? {
        connection
            .execute(
                "ALTER TABLE chat_index ADD COLUMN archived INTEGER NOT NULL DEFAULT 0",
                [],
            )
            .map_err(|error| format!("add Chat Index archived column: {error}"))?;
    }

    connection
        .execute_batch(
            r#"
            CREATE INDEX IF NOT EXISTS idx_chat_index_listing
                ON chat_index(pinned DESC, pin_order ASC, updated_at DESC);
            CREATE INDEX IF NOT EXISTS idx_chat_index_user_channel
                ON chat_index(user_id, channel, updated_at DESC);
            "#,
        )
        .map_err(|error| format!("initialize Chat Index indexes: {error}"))
}

fn table_has_column(connection: &Connection, column: &str) -> Result<bool, String> {
    let mut statement = connection
        .prepare("PRAGMA table_info(chat_index)")
        .map_err(|error| format!("inspect Chat Index schema: {error}"))?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|error| format!("read Chat Index schema: {error}"))?;
    for result in columns {
        if result.map_err(|error| format!("decode Chat Index schema: {error}"))? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

fn encode_json(value: &serde_json::Value) -> Result<String, String> {
    serde_json::to_string(value).map_err(|error| format!("serialize Chat Index metadata: {error}"))
}

fn parse_time(value: String) -> Result<DateTime<Utc>, rusqlite::Error> {
    DateTime::parse_from_rfc3339(&value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })
}

fn row_to_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<ChatIndexRecord> {
    let created_at: String = row.get("created_at")?;
    let updated_at: String = row.get("updated_at")?;
    let meta_json: String = row.get("meta_json")?;
    let pinned: i64 = row.get("pinned")?;
    let archived: i64 = row.get("archived")?;
    let tag: String = row.get("tag")?;
    Ok(ChatIndexRecord {
        id: row.get("id")?,
        name: row.get("name")?,
        user_id: row.get("user_id")?,
        channel: row.get("channel")?,
        created_at: parse_time(created_at)?,
        updated_at: parse_time(updated_at)?,
        meta: serde_json::from_str(&meta_json).unwrap_or(serde_json::Value::Null),
        status: row.get("status")?,
        pinned: pinned != 0,
        pin_order: row.get("pin_order")?,
        tag: tag.parse().map_err(|error: String| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                std::io::Error::new(std::io::ErrorKind::InvalidData, error).into(),
            )
        })?,
        archived: archived != 0,
    })
}

pub fn list_sync(
    workspace_dir: &Path,
    filter: ChatIndexFilter,
    offset: u64,
    limit: u64,
) -> Result<ChatIndexPage, String> {
    let connection = open_connection(workspace_dir)?;
    let selected_order = format!(
        "{} {}",
        filter.sort_by.sql_column(),
        filter.sort_order.sql_keyword()
    );
    let user_id = filter.user_id.as_deref();
    let channel = filter.channel.as_deref();
    let tag = filter.tag.map(ChatTag::as_str);
    let conversation_view = filter.conversation_view.as_str();
    let source_chat_id = filter.source_chat_id.as_deref();
    let executor = filter.executor.as_deref();
    let status = filter.status.as_deref();
    let archived = filter
        .archived
        .map(|value| if value { 1_i64 } else { 0_i64 });
    let search_pattern = filter
        .search
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(chat_index_search_pattern);
    let search = search_pattern.as_deref();
    let total = connection
        .query_row(
            r#"
            SELECT COUNT(*) FROM chat_index
            WHERE (?1 IS NULL OR user_id = ?1)
              AND (?2 IS NULL OR channel = ?2)
              AND (?3 IS NULL OR name LIKE ?3 ESCAPE '\' COLLATE NOCASE)
              AND (?4 IS NULL OR tag = ?4)
              AND (?5 = 'all'
                   OR (?5 = 'user' AND json_type(meta_json, '$.collaboration') IS NULL)
                   OR (?5 = 'collaboration' AND json_type(meta_json, '$.collaboration') IS NOT NULL))
              AND (?6 IS NULL OR json_extract(meta_json, '$.collaboration.source.chat_id') = ?6)
              AND (?7 IS NULL
                   OR json_extract(meta_json, '$.collaboration.executor.kind') = ?7
                   OR json_extract(meta_json, '$.collaboration.executor_agent_id') = ?7
                   OR json_extract(meta_json, '$.collaboration.executor.runner') = ?7)
              AND (?8 IS NULL OR status = ?8)
              AND (?9 IS NULL OR archived = ?9)
            "#,
            params![user_id, channel, search, tag, conversation_view, source_chat_id, executor, status, archived],
            |row| row.get::<_, u64>(0),
        )
        .map_err(|error| format!("count Chat Index: {error}"))?;
    let mut pinned_statement = connection
        .prepare(&format!(
            r#"
            SELECT {COLUMNS} FROM chat_index
            WHERE (?1 IS NULL OR user_id = ?1)
              AND (?2 IS NULL OR channel = ?2)
              AND (?3 IS NULL OR name LIKE ?3 ESCAPE '\' COLLATE NOCASE)
              AND (?4 IS NULL OR tag = ?4)
              AND (?5 = 'all'
                   OR (?5 = 'user' AND json_type(meta_json, '$.collaboration') IS NULL)
                   OR (?5 = 'collaboration' AND json_type(meta_json, '$.collaboration') IS NOT NULL))
              AND (?6 IS NULL OR json_extract(meta_json, '$.collaboration.source.chat_id') = ?6)
              AND (?7 IS NULL
                   OR json_extract(meta_json, '$.collaboration.executor.kind') = ?7
                   OR json_extract(meta_json, '$.collaboration.executor_agent_id') = ?7
                   OR json_extract(meta_json, '$.collaboration.executor.runner') = ?7)
              AND (?8 IS NULL OR status = ?8)
              AND (?9 IS NULL OR archived = ?9)
              AND pinned != 0
            ORDER BY pin_order ASC, {selected_order}, id ASC
            "#
        ))
        .map_err(|error| format!("prepare pinned Chat Index list: {error}"))?;
    let mut entries = pinned_statement
        .query_map(
            params![
                user_id,
                channel,
                search,
                tag,
                conversation_view,
                source_chat_id,
                executor,
                status,
                archived
            ],
            row_to_record,
        )
        .map_err(|error| format!("query pinned Chat Index: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("read pinned Chat Index: {error}"))?;
    let mut unpinned_statement = connection
        .prepare(&format!(
            r#"
            SELECT {COLUMNS} FROM chat_index
            WHERE (?1 IS NULL OR user_id = ?1)
              AND (?2 IS NULL OR channel = ?2)
              AND (?3 IS NULL OR name LIKE ?3 ESCAPE '\' COLLATE NOCASE)
              AND (?4 IS NULL OR tag = ?4)
              AND (?5 = 'all'
                   OR (?5 = 'user' AND json_type(meta_json, '$.collaboration') IS NULL)
                   OR (?5 = 'collaboration' AND json_type(meta_json, '$.collaboration') IS NOT NULL))
              AND (?6 IS NULL OR json_extract(meta_json, '$.collaboration.source.chat_id') = ?6)
              AND (?7 IS NULL
                   OR json_extract(meta_json, '$.collaboration.executor.kind') = ?7
                   OR json_extract(meta_json, '$.collaboration.executor_agent_id') = ?7
                   OR json_extract(meta_json, '$.collaboration.executor.runner') = ?7)
              AND (?8 IS NULL OR status = ?8)
              AND (?9 IS NULL OR archived = ?9)
              AND pinned = 0
            ORDER BY {selected_order}, id ASC
            LIMIT ?10 OFFSET ?11
            "#
        ))
        .map_err(|error| format!("prepare unpinned Chat Index list: {error}"))?;
    entries.extend(
        unpinned_statement
            .query_map(
                params![
                    user_id,
                    channel,
                    search,
                    tag,
                    conversation_view,
                    source_chat_id,
                    executor,
                    status,
                    archived,
                    limit.min(i64::MAX as u64),
                    offset
                ],
                row_to_record,
            )
            .map_err(|error| format!("query unpinned Chat Index: {error}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("read unpinned Chat Index: {error}"))?,
    );
    Ok(ChatIndexPage { entries, total })
}

fn chat_index_search_pattern(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('%');
    for character in value.chars() {
        if matches!(character, '\\' | '%' | '_') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped.push('%');
    escaped
}

pub fn list_all_sync(workspace_dir: &Path) -> Result<Vec<ChatIndexRecord>, String> {
    let connection = open_connection(workspace_dir)?;
    let mut statement = connection
        .prepare(&format!(
            "SELECT {COLUMNS} FROM chat_index ORDER BY pinned DESC, pin_order ASC, updated_at DESC"
        ))
        .map_err(|error| format!("prepare full Chat Index list: {error}"))?;
    let records = statement
        .query_map([], row_to_record)
        .map_err(|error| format!("query full Chat Index: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("read full Chat Index: {error}"))?;
    Ok(records)
}

/// Latest activity across all Chats in a Scope, including archived and collaboration Chats.
/// Reads only the existing projection and never materializes an empty Workspace.
pub fn latest_activity_sync(workspace_dir: &Path) -> Result<Option<DateTime<Utc>>, String> {
    let path = db_path(workspace_dir);
    if !path
        .try_exists()
        .map_err(|error| format!("inspect Chat Index: {error}"))?
    {
        return Ok(None);
    }
    let connection = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| format!("open Chat Index activity: {error}"))?;
    connection
        .busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|error| format!("set Chat Index activity busy timeout: {error}"))?;
    connection
        .query_row("SELECT MAX(updated_at) FROM chat_index", [], |row| {
            row.get::<_, Option<String>>(0)?.map(parse_time).transpose()
        })
        .map_err(|error| format!("read Chat Index latest activity: {error}"))
}

pub fn get_sync(workspace_dir: &Path, chat_id: &str) -> Result<Option<ChatIndexRecord>, String> {
    open_connection(workspace_dir)?
        .query_row(
            &format!("SELECT {COLUMNS} FROM chat_index WHERE id = ?1"),
            params![chat_id],
            row_to_record,
        )
        .optional()
        .map_err(|error| format!("read Chat Index record: {error}"))
}

pub fn upsert_sync(workspace_dir: &Path, record: &ChatIndexRecord) -> Result<(), String> {
    open_connection(workspace_dir)?
        .execute(
            r#"
            INSERT INTO chat_index (
                id, name, user_id, channel, created_at, updated_at,
                meta_json, status, pinned, pin_order, tag, archived
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
            ON CONFLICT(id) DO UPDATE SET
                name = excluded.name,
                user_id = excluded.user_id,
                channel = excluded.channel,
                created_at = excluded.created_at,
                updated_at = excluded.updated_at,
                meta_json = excluded.meta_json,
                status = excluded.status,
                pinned = excluded.pinned,
                pin_order = excluded.pin_order,
                tag = excluded.tag,
                archived = excluded.archived
            "#,
            params![
                &record.id,
                &record.name,
                &record.user_id,
                &record.channel,
                record.created_at.to_rfc3339(),
                record.updated_at.to_rfc3339(),
                encode_json(&record.meta)?,
                &record.status,
                if record.pinned { 1_i64 } else { 0_i64 },
                record.pin_order,
                record.tag.as_str(),
                if record.archived { 1_i64 } else { 0_i64 },
            ],
        )
        .map_err(|error| format!("upsert Chat Index record: {error}"))?;
    Ok(())
}

pub fn insert_sync(workspace_dir: &Path, record: &ChatIndexRecord) -> Result<(), String> {
    open_connection(workspace_dir)?
        .execute(
            r#"
            INSERT INTO chat_index (
                id, name, user_id, channel, created_at, updated_at,
                meta_json, status, pinned, pin_order, tag, archived
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
            "#,
            params![
                &record.id,
                &record.name,
                &record.user_id,
                &record.channel,
                record.created_at.to_rfc3339(),
                record.updated_at.to_rfc3339(),
                encode_json(&record.meta)?,
                &record.status,
                if record.pinned { 1_i64 } else { 0_i64 },
                record.pin_order,
                record.tag.as_str(),
                if record.archived { 1_i64 } else { 0_i64 },
            ],
        )
        .map_err(|error| format!("insert Chat Index record: {error}"))?;
    Ok(())
}

pub fn delete_sync(workspace_dir: &Path, chat_id: &str) -> Result<(), String> {
    open_connection(workspace_dir)?
        .execute("DELETE FROM chat_index WHERE id = ?1", params![chat_id])
        .map_err(|error| format!("delete Chat Index record: {error}"))?;
    Ok(())
}

pub fn delete_many_sync(workspace_dir: &Path, chat_ids: &[String]) -> Result<(), String> {
    let mut connection = open_connection(workspace_dir)?;
    let transaction = connection
        .transaction()
        .map_err(|error| format!("begin Chat Index delete: {error}"))?;
    for chat_id in chat_ids {
        transaction
            .execute("DELETE FROM chat_index WHERE id = ?1", params![chat_id])
            .map_err(|error| format!("delete Chat Index record: {error}"))?;
    }
    transaction
        .commit()
        .map_err(|error| format!("commit Chat Index delete: {error}"))
}

pub fn update_pin_order_sync(workspace_dir: &Path, items: &[(String, i64)]) -> Result<(), String> {
    let mut connection = open_connection(workspace_dir)?;
    let transaction = connection
        .transaction()
        .map_err(|error| format!("begin Chat Index pin update: {error}"))?;
    for (chat_id, pin_order) in items {
        transaction
            .execute(
                "UPDATE chat_index SET pin_order = ?1 WHERE id = ?2",
                params![pin_order, chat_id],
            )
            .map_err(|error| format!("update Chat Index pin order: {error}"))?;
    }
    transaction
        .commit()
        .map_err(|error| format!("commit Chat Index pin update: {error}"))
}

pub async fn list(
    workspace_dir: &Path,
    filter: ChatIndexFilter,
    offset: u64,
    limit: u64,
) -> Result<ChatIndexPage, String> {
    let workspace_dir = workspace_dir.to_path_buf();
    tokio::task::spawn_blocking(move || list_sync(&workspace_dir, filter, offset, limit))
        .await
        .map_err(|error| format!("Chat Index list task failed: {error}"))?
}

pub async fn get(workspace_dir: &Path, chat_id: &str) -> Result<Option<ChatIndexRecord>, String> {
    let workspace_dir = workspace_dir.to_path_buf();
    let chat_id = chat_id.to_string();
    tokio::task::spawn_blocking(move || get_sync(&workspace_dir, &chat_id))
        .await
        .map_err(|error| format!("Chat Index get task failed: {error}"))?
}

pub async fn list_all(workspace_dir: &Path) -> Result<Vec<ChatIndexRecord>, String> {
    let workspace_dir = workspace_dir.to_path_buf();
    tokio::task::spawn_blocking(move || list_all_sync(&workspace_dir))
        .await
        .map_err(|error| format!("Chat Index full-list task failed: {error}"))?
}

pub async fn latest_activity(workspace_dir: &Path) -> Result<Option<DateTime<Utc>>, String> {
    let workspace_dir = workspace_dir.to_path_buf();
    tokio::task::spawn_blocking(move || latest_activity_sync(&workspace_dir))
        .await
        .map_err(|error| format!("Chat Index latest-activity task failed: {error}"))?
}

pub async fn upsert(workspace_dir: &Path, record: ChatIndexRecord) -> Result<(), String> {
    let workspace_dir = workspace_dir.to_path_buf();
    tokio::task::spawn_blocking(move || upsert_sync(&workspace_dir, &record))
        .await
        .map_err(|error| format!("Chat Index upsert task failed: {error}"))?
}

pub async fn insert(workspace_dir: &Path, record: ChatIndexRecord) -> Result<(), String> {
    let workspace_dir = workspace_dir.to_path_buf();
    tokio::task::spawn_blocking(move || insert_sync(&workspace_dir, &record))
        .await
        .map_err(|error| format!("Chat Index insert task failed: {error}"))?
}

pub async fn delete(workspace_dir: &Path, chat_id: &str) -> Result<(), String> {
    let workspace_dir = workspace_dir.to_path_buf();
    let chat_id = chat_id.to_string();
    tokio::task::spawn_blocking(move || delete_sync(&workspace_dir, &chat_id))
        .await
        .map_err(|error| format!("Chat Index delete task failed: {error}"))?
}

pub async fn delete_many(workspace_dir: &Path, chat_ids: Vec<String>) -> Result<(), String> {
    let workspace_dir = workspace_dir.to_path_buf();
    tokio::task::spawn_blocking(move || delete_many_sync(&workspace_dir, &chat_ids))
        .await
        .map_err(|error| format!("Chat Index batch-delete task failed: {error}"))?
}

pub async fn update_pin_order(
    workspace_dir: &Path,
    items: Vec<(String, i64)>,
) -> Result<(), String> {
    let workspace_dir = workspace_dir.to_path_buf();
    tokio::task::spawn_blocking(move || update_pin_order_sync(&workspace_dir, &items))
        .await
        .map_err(|error| format!("Chat Index pin-update task failed: {error}"))?
}
