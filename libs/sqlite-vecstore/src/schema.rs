use rusqlite::Connection;

use crate::error::Result;

pub const SCHEMA_VERSION: u32 = 1;

pub fn init_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        -- Schema version tracking
        CREATE TABLE IF NOT EXISTS vec_schema_version (
            version INTEGER NOT NULL
        );

        -- Main documents table
        CREATE TABLE IF NOT EXISTS vec_documents (
            id TEXT PRIMARY KEY,
            content TEXT NOT NULL,
            metadata TEXT NOT NULL DEFAULT '{}',
            embedding BLOB,
            dimension INTEGER,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        -- FTS5 full-text search index
        CREATE VIRTUAL TABLE IF NOT EXISTS vec_documents_fts USING fts5(
            id UNINDEXED,
            content,
            tokenize = 'unicode61'
        );

        -- Metadata index for filtered queries
        CREATE INDEX IF NOT EXISTS idx_vec_documents_created
            ON vec_documents(created_at);
        ",
    )?;

    let count: i64 = conn.query_row("SELECT COUNT(*) FROM vec_schema_version", [], |row| {
        row.get(0)
    })?;
    if count == 0 {
        conn.execute(
            "INSERT INTO vec_schema_version (version) VALUES (?1)",
            [SCHEMA_VERSION],
        )?;
    }

    Ok(())
}

pub fn get_schema_version(conn: &Connection) -> Result<u32> {
    let version: u32 = conn.query_row(
        "SELECT version FROM vec_schema_version LIMIT 1",
        [],
        |row| row.get(0),
    )?;
    Ok(version)
}
