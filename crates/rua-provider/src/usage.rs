use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::{self, Read as _, Seek, SeekFrom, Write as _};
use std::path::PathBuf;
use std::sync::Mutex;

use agent_types::TokenUsage;
use chrono::{Local, NaiveDate};
use fs2::FileExt;
use rua_config::types::token_usage::{TokenUsageEntry, TokenUsageFile};
use serde_json::Value;
use tracing::{debug, warn};

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct UsageKey {
    date: NaiveDate,
    provider_id: String,
    model_id: String,
}

#[derive(Debug, Clone, Default)]
pub struct AccumulatedUsage {
    pub total: TokenUsage,
    pub request_count: u64,
}

#[derive(Debug, Clone, Default)]
pub struct UsageSnapshot {
    pub by_model: Vec<ModelUsageEntry>,
    pub grand_total: TokenUsage,
    pub total_requests: u64,
}

#[derive(Debug, Clone)]
pub struct ModelUsageEntry {
    pub date: NaiveDate,
    pub provider_id: String,
    pub model_id: String,
    pub usage: TokenUsage,
    pub request_count: u64,
}

pub fn normalize_legacy_format(json: Value) -> Value {
    let obj = match json.as_object() {
        Some(o) if !o.is_empty() => o,
        _ => return json,
    };

    let is_flat = obj
        .keys()
        .any(|k| NaiveDate::parse_from_str(k, "%Y-%m-%d").is_err());

    if is_flat {
        let today = Local::now().date_naive().format("%Y-%m-%d").to_string();
        let mut wrapper = serde_json::Map::new();
        wrapper.insert(today, json);
        return backfill_entries(Value::Object(wrapper));
    }

    backfill_entries(json)
}

fn backfill_entries(json: Value) -> Value {
    let obj = match json.as_object() {
        Some(o) => o,
        None => return json,
    };
    let mut result = serde_json::Map::new();
    for (date_key, models_val) in obj {
        let models = match models_val.as_object() {
            Some(m) => m,
            None => continue,
        };
        let mut fixed = serde_json::Map::new();
        for (composite_key, entry_val) in models {
            let mut entry = match entry_val.as_object() {
                Some(e) => e.clone(),
                None => continue,
            };
            let parts: Vec<&str> = composite_key.splitn(2, ':').collect();
            if parts.len() == 2 {
                entry
                    .entry("provider_id")
                    .or_insert_with(|| Value::String(parts[0].to_string()));
                entry
                    .entry("model_name")
                    .or_insert_with(|| Value::String(parts[1].to_string()));
            }
            for field in ["prompt_tokens", "completion_tokens", "call_count"] {
                entry
                    .entry(field)
                    .or_insert_with(|| Value::Number(0.into()));
            }
            fixed.insert(composite_key.clone(), Value::Object(entry));
        }
        result.insert(date_key.clone(), Value::Object(fixed));
    }
    Value::Object(result)
}

pub struct UsageTracker {
    data: Mutex<HashMap<UsageKey, AccumulatedUsage>>,
    file_path: PathBuf,
}

impl UsageTracker {
    pub fn new(working_dir: PathBuf) -> Self {
        let file_path = working_dir.join("token_usage.json");
        let tracker = Self {
            data: Mutex::new(HashMap::new()),
            file_path,
        };
        if let Err(e) = tracker.load_from_disk() {
            warn!("failed to load token usage from disk: {e}");
        }
        tracker
    }

    fn load_from_disk(&self) -> io::Result<()> {
        if !self.file_path.exists() {
            return Ok(());
        }
        let raw = fs::read_to_string(&self.file_path)?;
        if raw.trim().is_empty() {
            return Ok(());
        }

        if self.needs_migration(&raw) {
            self.migrate_old_format(&raw)?;
            return self.load_from_disk();
        }

        let file_data: TokenUsageFile = match serde_json::from_str(&raw) {
            Ok(d) => d,
            Err(e) => {
                warn!("direct parse failed ({e}), trying legacy normalize");
                let json: Value = serde_json::from_str(&raw).map_err(io::Error::other)?;
                let normalized = normalize_legacy_format(json);
                serde_json::from_value(normalized).map_err(io::Error::other)?
            }
        };

        let mut data = self.data.lock().unwrap_or_else(|p| p.into_inner());
        for (date_str, models) in &file_data {
            let date = match NaiveDate::parse_from_str(date_str, "%Y-%m-%d") {
                Ok(d) => d,
                Err(_) => continue,
            };
            for (composite_key, entry) in models {
                let (provider_id, model_id) =
                    if entry.provider_id.is_empty() || entry.model_name.is_empty() {
                        let parts: Vec<&str> = composite_key.splitn(2, ':').collect();
                        if parts.len() == 2 {
                            (
                                if entry.provider_id.is_empty() {
                                    parts[0].to_string()
                                } else {
                                    entry.provider_id.clone()
                                },
                                if entry.model_name.is_empty() {
                                    parts[1].to_string()
                                } else {
                                    entry.model_name.clone()
                                },
                            )
                        } else {
                            (entry.provider_id.clone(), entry.model_name.clone())
                        }
                    } else {
                        (entry.provider_id.clone(), entry.model_name.clone())
                    };
                let key = UsageKey {
                    date,
                    provider_id,
                    model_id,
                };
                let acc = data.entry(key).or_default();
                acc.total.prompt_tokens += entry.prompt_tokens;
                acc.total.completion_tokens += entry.completion_tokens;
                acc.request_count += entry.call_count;
            }
        }
        Ok(())
    }

    fn needs_migration(&self, raw: &str) -> bool {
        let parsed: serde_json::Value = match serde_json::from_str(raw) {
            Ok(v) => v,
            Err(_) => return false,
        };
        let obj = match parsed.as_object() {
            Some(o) => o,
            None => return false,
        };
        if obj.is_empty() {
            return false;
        }

        obj.keys()
            .any(|k| NaiveDate::parse_from_str(k, "%Y-%m-%d").is_err())
    }

    fn migrate_old_format(&self, raw: &str) -> io::Result<()> {
        let backup = self.file_path.with_extension("json.bak");
        fs::write(&backup, raw)?;
        warn!(
            "migrated old token_usage format, backup at {}",
            backup.display()
        );

        let old: HashMap<String, TokenUsageEntry> =
            serde_json::from_str(raw).map_err(io::Error::other)?;

        let today = Local::now().date_naive().format("%Y-%m-%d").to_string();
        let mut migrated = TokenUsageFile::new();
        migrated.insert(today, HashMap::new());
        let day = migrated.values_mut().next().unwrap();

        for (composite_key, entry) in old {
            day.insert(composite_key, entry);
        }

        let json = serde_json::to_string_pretty(&migrated).map_err(io::Error::other)?;
        fs::write(&self.file_path, json)?;
        Ok(())
    }

    pub fn record(
        &self,
        provider_id: &str,
        model_id: &str,
        usage: &TokenUsage,
        at_date: Option<NaiveDate>,
    ) {
        if usage.is_empty() {
            return;
        }
        let date = at_date.unwrap_or_else(|| Local::now().date_naive());

        if let Err(e) = self.flush_record(date, provider_id, model_id, usage) {
            warn!("failed to flush token usage to disk: {e}");
        }

        let key = UsageKey {
            date,
            provider_id: provider_id.to_string(),
            model_id: model_id.to_string(),
        };
        let mut data = self.data.lock().unwrap_or_else(|p| p.into_inner());
        let acc = data.entry(key).or_default();
        acc.total.accumulate(usage);
        acc.request_count += 1;

        debug!(
            provider_id,
            model_id,
            prompt_tokens = usage.prompt_tokens,
            completion_tokens = usage.completion_tokens,
            "recorded usage"
        );
    }

    fn flush_record(
        &self,
        date: NaiveDate,
        provider_id: &str,
        model_id: &str,
        usage: &TokenUsage,
    ) -> io::Result<()> {
        if let Some(parent) = self.file_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&self.file_path)?;

        file.lock_exclusive()?;

        let mut content = String::new();
        file.read_to_string(&mut content)?;

        let mut existing: TokenUsageFile = if content.trim().is_empty() {
            HashMap::new()
        } else {
            match serde_json::from_str(&content) {
                Ok(d) => d,
                Err(_) => {
                    let json: Value =
                        serde_json::from_str(&content).unwrap_or(Value::Object(Default::default()));
                    let normalized = normalize_legacy_format(json);
                    serde_json::from_value(normalized).unwrap_or_default()
                }
            }
        };

        let date_str = date.format("%Y-%m-%d").to_string();
        let composite_key = format!("{provider_id}:{model_id}");
        let day = existing.entry(date_str).or_default();
        let entry = day.entry(composite_key).or_insert_with(|| TokenUsageEntry {
            provider_id: provider_id.to_string(),
            model_name: model_id.to_string(),
            ..Default::default()
        });
        entry.prompt_tokens += usage.prompt_tokens;
        entry.completion_tokens += usage.completion_tokens;
        entry.call_count += 1;

        let json = serde_json::to_string_pretty(&existing).map_err(io::Error::other)?;
        file.set_len(0)?;
        file.seek(SeekFrom::Start(0))?;
        file.write_all(json.as_bytes())?;
        file.sync_all()?;

        Ok(())
    }

    pub fn snapshot(&self) -> UsageSnapshot {
        let data = self.data.lock().unwrap_or_else(|p| p.into_inner());
        let mut grand_total = TokenUsage::default();
        let mut total_requests = 0u64;
        let mut by_model = Vec::with_capacity(data.len());

        for (key, acc) in data.iter() {
            grand_total.accumulate(&acc.total);
            total_requests += acc.request_count;
            by_model.push(ModelUsageEntry {
                date: key.date,
                provider_id: key.provider_id.clone(),
                model_id: key.model_id.clone(),
                usage: acc.total.clone(),
                request_count: acc.request_count,
            });
        }

        by_model.sort_by(|a, b| {
            a.date
                .cmp(&b.date)
                .then(a.provider_id.cmp(&b.provider_id))
                .then(a.model_id.cmp(&b.model_id))
        });

        UsageSnapshot {
            by_model,
            grand_total,
            total_requests,
        }
    }

    pub fn get(
        &self,
        date: NaiveDate,
        provider_id: &str,
        model_id: &str,
    ) -> Option<AccumulatedUsage> {
        let key = UsageKey {
            date,
            provider_id: provider_id.to_string(),
            model_id: model_id.to_string(),
        };
        self.data
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(&key)
            .cloned()
    }

    pub fn total(&self) -> TokenUsage {
        let data = self.data.lock().unwrap_or_else(|p| p.into_inner());
        let mut total = TokenUsage::default();
        for acc in data.values() {
            total.accumulate(&acc.total);
        }
        total
    }

    pub fn reset(&self) {
        self.data.lock().unwrap_or_else(|p| p.into_inner()).clear();
    }
}
