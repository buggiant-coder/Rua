CREATE TABLE memory_records (
    id TEXT PRIMARY KEY,
    agent_id TEXT NOT NULL,
    scope TEXT NOT NULL,
    kind TEXT NOT NULL,
    subject TEXT,
    content TEXT NOT NULL,
    status TEXT NOT NULL,
    confidence REAL NOT NULL,
    importance REAL,
    created_at TEXT NOT NULL,
    updated_at TEXT,
    last_used_at TEXT,
    expires_at TEXT,
    superseded_by TEXT,
    written_on_device_id TEXT,
    written_on_device_name_snapshot TEXT,
    written_on_host_profile TEXT,
    resource_device_id TEXT,
    resource_device_name_snapshot TEXT,
    metadata_json TEXT NOT NULL DEFAULT '{}'
);
CREATE VIRTUAL TABLE memory_records_fts USING fts5(id UNINDEXED, content, subject);
