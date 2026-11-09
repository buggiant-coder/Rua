use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use agent_types::TokenUsage;
use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use rua_contract::{UsageRecordMetadata, UsageRecorder};
use rua_core::{Result, RuaError};
use rusqlite::{params, Connection};
use sha2::{Digest, Sha256};

const DATABASE_FILE: &str = "token-usage.sqlite3";

#[derive(Debug, Clone)]
pub struct UsageLedger {
    database_path: PathBuf,
    legacy_cleanup: Arc<OnceLock<()>>,
}

#[derive(Debug, Clone, Default)]
pub struct TokenUsageFilter {
    pub start_date: Option<String>,
    pub end_date: Option<String>,
    pub days: Option<u32>,
    pub provider_id: Option<String>,
    pub model_name: Option<String>,
    pub agent_id: Option<String>,
    pub chat_id: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TokenUsageStats {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub call_count: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TokenUsageByModel {
    pub provider_id: String,
    pub model: String,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub call_count: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TokenUsageRecord {
    pub date: String,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub provider_id: String,
    pub model: String,
    pub call_count: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TokenUsageSummary {
    pub total_prompt_tokens: u64,
    pub total_completion_tokens: u64,
    pub total_calls: u64,
    pub by_model: HashMap<String, TokenUsageByModel>,
    pub by_provider: HashMap<String, TokenUsageStats>,
    pub by_date: HashMap<String, TokenUsageStats>,
}

#[derive(Debug, Clone)]
pub struct TokenUsageQuery {
    rua_root: PathBuf,
}

impl TokenUsageQuery {
    pub fn new(rua_root: PathBuf) -> Self {
        Self { rua_root }
    }

    pub fn query(&self, filter: &TokenUsageFilter) -> Result<TokenUsageSummary> {
        UsageLedger::open(&self.rua_root)?.query(filter)
    }

    pub fn query_details(&self, filter: &TokenUsageFilter) -> Result<Vec<TokenUsageRecord>> {
        UsageLedger::open(&self.rua_root)?.query_details(filter)
    }
}

impl UsageLedger {
    pub fn new(rua_root: impl AsRef<Path>) -> Self {
        Self {
            database_path: rua_root.as_ref().join(DATABASE_FILE),
            legacy_cleanup: Arc::new(OnceLock::new()),
        }
    }

    pub fn open(rua_root: impl AsRef<Path>) -> Result<Self> {
        let rua_root = rua_root.as_ref();
        let ledger = Self::new(rua_root);
        ledger.with_connection(|_| Ok(()))?;
        Ok(ledger)
    }

    pub fn database_path(&self) -> &Path {
        &self.database_path
    }

    pub fn query(&self, filter: &TokenUsageFilter) -> Result<TokenUsageSummary> {
        self.with_connection(|connection| query_summary(connection, filter))
    }

    pub fn query_details(&self, filter: &TokenUsageFilter) -> Result<Vec<TokenUsageRecord>> {
        self.with_connection(|connection| query_details(connection, filter))
    }

    pub fn clear(&self) -> Result<()> {
        self.with_connection(|connection| {
            connection
                .execute("DELETE FROM token_usage", [])
                .map_err(sqlite_error)?;
            Ok(())
        })
    }

    pub fn record(
        &self,
        provider_id: &str,
        model_id: &str,
        usage: &TokenUsage,
        started_at: DateTime<Utc>,
        finished_at: DateTime<Utc>,
        metadata: &UsageRecordMetadata,
    ) -> Result<()> {
        let usage_id = metadata.usage_id.clone().unwrap_or_else(|| {
            stable_usage_id(
                provider_id,
                model_id,
                usage,
                started_at,
                finished_at,
                metadata,
            )
        });
        self.with_connection(|connection| {
            connection
                .execute(
                    "INSERT OR IGNORE INTO token_usage (
                        usage_id, provider_id, model_id, prompt_tokens,
                        completion_tokens, reasoning_tokens, cache_read_tokens,
                        cache_write_tokens, started_at_ms, finished_at_ms,
                        agent_id, chat_id, session_id, turn_id, run_id, device_id, source
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
                    params![
                        usage_id,
                        provider_id,
                        model_id,
                        to_i64(usage.prompt_tokens),
                        to_i64(usage.completion_tokens),
                        to_i64(usage.reasoning_tokens.unwrap_or_default()),
                        to_i64(usage.cached_tokens.unwrap_or_default()),
                        to_i64(usage.cache_write_tokens.unwrap_or_default()),
                        started_at.timestamp_millis(),
                        finished_at.timestamp_millis(),
                        metadata.agent_id,
                        metadata.chat_id,
                        metadata.session_id,
                        metadata.turn_id,
                        metadata.run_id,
                        metadata.device_id,
                        metadata.source,
                    ],
                )
                .map_err(sqlite_error)?;
            Ok(())
        })
    }

    fn with_connection<T>(&self, operation: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        if let Some(parent) = self.database_path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| RuaError::Io {
                message: error.to_string(),
            })?;
        }
        let connection = Connection::open(&self.database_path).map_err(sqlite_error)?;
        connection
            .busy_timeout(std::time::Duration::from_secs(5))
            .map_err(sqlite_error)?;
        connection
            .execute_batch(
                "PRAGMA journal_mode = WAL;
                 PRAGMA synchronous = FULL;
                 CREATE TABLE IF NOT EXISTS token_usage (
                    usage_id TEXT PRIMARY KEY,
                    provider_id TEXT NOT NULL,
                    model_id TEXT NOT NULL,
                    prompt_tokens INTEGER NOT NULL,
                    completion_tokens INTEGER NOT NULL,
                    reasoning_tokens INTEGER NOT NULL,
                    cache_read_tokens INTEGER NOT NULL,
                    cache_write_tokens INTEGER NOT NULL,
                    started_at_ms INTEGER NOT NULL,
                    finished_at_ms INTEGER NOT NULL,
                    agent_id TEXT,
                    chat_id TEXT,
                    session_id TEXT,
                    turn_id TEXT,
                    run_id TEXT,
                    device_id TEXT,
                    source TEXT
                 );
                 CREATE INDEX IF NOT EXISTS idx_token_usage_finished_at
                    ON token_usage(finished_at_ms);
                 CREATE INDEX IF NOT EXISTS idx_token_usage_provider_model_finished
                    ON token_usage(provider_id, model_id, finished_at_ms);
                 CREATE INDEX IF NOT EXISTS idx_token_usage_agent_finished
                    ON token_usage(agent_id, finished_at_ms);",
            )
            .map_err(sqlite_error)?;
        self.legacy_cleanup.get_or_init(|| {
            if let Some(rua_root) = self.database_path.parent() {
                cleanup_legacy_ledgers(rua_root);
            }
        });
        operation(&connection)
    }
}

#[async_trait]
impl UsageRecorder for UsageLedger {
    async fn record_usage(
        &self,
        provider_id: &str,
        model_id: &str,
        usage: &TokenUsage,
        started_at: DateTime<Utc>,
        finished_at: DateTime<Utc>,
        metadata: UsageRecordMetadata,
    ) {
        if usage.is_empty() {
            return;
        }
        let ledger = self.clone();
        let provider_id = provider_id.to_owned();
        let model_id = model_id.to_owned();
        let usage = usage.clone();
        match tokio::task::spawn_blocking(move || {
            ledger.record(
                &provider_id,
                &model_id,
                &usage,
                started_at,
                finished_at,
                &metadata,
            )
        })
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(error)) => tracing::error!(%error, "failed to persist token usage"),
            Err(error) => tracing::error!(%error, "token usage persistence task failed"),
        }
    }
}

fn query_summary(connection: &Connection, filter: &TokenUsageFilter) -> Result<TokenUsageSummary> {
    let (start, end) = resolve_range(filter);
    let start_ms = start
        .and_then(|date| date.and_hms_opt(0, 0, 0))
        .map(|value| Utc.from_utc_datetime(&value).timestamp_millis());
    let end_ms = end
        .and_then(|date| date.succ_opt())
        .and_then(|date| date.and_hms_opt(0, 0, 0))
        .map(|value| Utc.from_utc_datetime(&value).timestamp_millis());

    let mut statement = connection
        .prepare(
            "SELECT provider_id, model_id, prompt_tokens, completion_tokens, finished_at_ms
             FROM token_usage
             WHERE (?1 IS NULL OR finished_at_ms >= ?1)
               AND (?2 IS NULL OR finished_at_ms < ?2)
               AND (?3 IS NULL OR provider_id = ?3)
               AND (?4 IS NULL OR model_id = ?4)
               AND (?5 IS NULL OR agent_id = ?5)
               AND (?6 IS NULL OR chat_id = ?6)
             ORDER BY finished_at_ms DESC",
        )
        .map_err(sqlite_error)?;
    let rows = statement
        .query_map(
            params![
                start_ms,
                end_ms,
                filter.provider_id,
                filter.model_name,
                filter.agent_id,
                filter.chat_id
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, u64>(2)?,
                    row.get::<_, u64>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            },
        )
        .map_err(sqlite_error)?;

    let mut summary = TokenUsageSummary::default();
    for row in rows {
        let (provider_id, model_name, prompt_tokens, completion_tokens, finished_at_ms) =
            row.map_err(sqlite_error)?;
        summary.total_prompt_tokens += prompt_tokens;
        summary.total_completion_tokens += completion_tokens;
        summary.total_calls += 1;

        let model_key = format!("{provider_id}:{model_name}");
        let model = summary
            .by_model
            .entry(model_key)
            .or_insert_with(|| TokenUsageByModel {
                provider_id: provider_id.clone(),
                model: model_name.clone(),
                ..Default::default()
            });
        model.prompt_tokens += prompt_tokens;
        model.completion_tokens += completion_tokens;
        model.call_count += 1;

        let provider = summary.by_provider.entry(provider_id.clone()).or_default();
        add_stats(provider, prompt_tokens, completion_tokens);
        let date = Utc
            .timestamp_millis_opt(finished_at_ms)
            .single()
            .ok_or_else(|| RuaError::Validation {
                message: "invalid usage timestamp".into(),
            })?
            .date_naive()
            .to_string();
        let day = summary.by_date.entry(date.clone()).or_default();
        add_stats(day, prompt_tokens, completion_tokens);
    }
    Ok(summary)
}

fn query_details(
    connection: &Connection,
    filter: &TokenUsageFilter,
) -> Result<Vec<TokenUsageRecord>> {
    let summary = query_summary(connection, filter)?;
    let mut records = Vec::new();
    for (date, _) in summary.by_date {
        let mut statement = connection
            .prepare(
                "SELECT provider_id, model_id, SUM(prompt_tokens), SUM(completion_tokens), COUNT(*)
                 FROM token_usage
                 WHERE date(finished_at_ms / 1000, 'unixepoch') = ?1
                   AND (?2 IS NULL OR provider_id = ?2)
                   AND (?3 IS NULL OR model_id = ?3)
                   AND (?4 IS NULL OR agent_id = ?4)
                   AND (?5 IS NULL OR chat_id = ?5)
                 GROUP BY provider_id, model_id",
            )
            .map_err(sqlite_error)?;
        let rows = statement
            .query_map(
                params![
                    date,
                    filter.provider_id,
                    filter.model_name,
                    filter.agent_id,
                    filter.chat_id
                ],
                |row| {
                    Ok(TokenUsageRecord {
                        date: date.clone(),
                        provider_id: row.get(0)?,
                        model: row.get(1)?,
                        prompt_tokens: row.get(2)?,
                        completion_tokens: row.get(3)?,
                        call_count: row.get(4)?,
                    })
                },
            )
            .map_err(sqlite_error)?;
        for row in rows {
            records.push(row.map_err(sqlite_error)?);
        }
    }
    records.sort_by(|a, b| a.date.cmp(&b.date).then_with(|| a.model.cmp(&b.model)));
    Ok(records)
}

fn add_stats(stats: &mut TokenUsageStats, prompt: u64, completion: u64) {
    stats.prompt_tokens += prompt;
    stats.completion_tokens += completion;
    stats.call_count += 1;
}

fn resolve_range(filter: &TokenUsageFilter) -> (Option<NaiveDate>, Option<NaiveDate>) {
    let explicit_start = filter.start_date.as_deref().and_then(parse_date);
    let explicit_end = filter.end_date.as_deref().and_then(parse_date);
    if explicit_start.is_some() || explicit_end.is_some() {
        let end = explicit_end.unwrap_or_else(|| Utc::now().date_naive());
        let start = explicit_start.unwrap_or(end - chrono::Duration::days(30));
        return if start <= end {
            (Some(start), Some(end))
        } else {
            (Some(end), Some(start))
        };
    }
    filter.days.map_or((None, None), |days| {
        let end = Utc::now().date_naive();
        let start = end - chrono::Duration::days(i64::from(days.saturating_sub(1)));
        (Some(start), Some(end))
    })
}

fn parse_date(value: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(value, "%Y-%m-%d").ok()
}

fn stable_usage_id(
    provider_id: &str,
    model_id: &str,
    usage: &TokenUsage,
    started_at: DateTime<Utc>,
    finished_at: DateTime<Utc>,
    metadata: &UsageRecordMetadata,
) -> String {
    let mut hasher = Sha256::new();
    for part in [
        provider_id,
        model_id,
        &usage.prompt_tokens.to_string(),
        &usage.completion_tokens.to_string(),
        &started_at.timestamp_millis().to_string(),
        &finished_at.timestamp_millis().to_string(),
        metadata.agent_id.as_deref().unwrap_or_default(),
        metadata.chat_id.as_deref().unwrap_or_default(),
        metadata.turn_id.as_deref().unwrap_or_default(),
        metadata.run_id.as_deref().unwrap_or_default(),
    ] {
        hasher.update(part.as_bytes());
        hasher.update([0]);
    }
    format!("usage_{:x}", hasher.finalize())
}

fn to_i64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn sqlite_error(error: rusqlite::Error) -> RuaError {
    RuaError::Memory {
        message: format!("token usage database error: {error}"),
    }
}

fn cleanup_legacy_ledgers(rua_root: &Path) {
    let legacy_file = rua_root.join("token_usage_ledger.json");
    if let Err(error) = std::fs::remove_file(&legacy_file) {
        if error.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!(path = %legacy_file.display(), %error, "failed to remove legacy token usage ledger");
        }
    }

    let legacy_shards = rua_root.join("token_usage_ledger");
    if let Err(error) = std::fs::remove_dir_all(&legacy_shards) {
        if error.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!(path = %legacy_shards.display(), %error, "failed to remove legacy token usage shards");
        }
    }

    let Ok(entries) = std::fs::read_dir(rua_root) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !name.starts_with(".token_usage_ledger.json.tmp.") {
            continue;
        }
        if let Err(error) = std::fs::remove_file(entry.path()) {
            tracing::warn!(path = %entry.path().display(), %error, "failed to remove legacy token usage temp file");
        }
    }
}
