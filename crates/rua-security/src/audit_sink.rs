use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{NaiveDate, Utc};
use tracing::field::{Field, Visit};
use tracing::Subscriber;
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::registry::LookupSpan;

pub struct FileAuditSink {
    dir: PathBuf,
    state: Mutex<SinkState>,
}

struct SinkState {
    current_date: NaiveDate,
    current_path: PathBuf,
}

impl FileAuditSink {
    pub fn new(audit_dir: impl Into<PathBuf>) -> Self {
        let dir = audit_dir.into();
        let today = Utc::now().date_naive();
        let path = Self::path_for_date(&dir, today);
        Self {
            dir,
            state: Mutex::new(SinkState {
                current_date: today,
                current_path: path,
            }),
        }
    }

    fn path_for_date(dir: &Path, date: NaiveDate) -> PathBuf {
        dir.join(format!("audit-{}.jsonl", date.format("%Y-%m-%d")))
    }

    fn append_line(&self, json_line: &str) {
        let today = Utc::now().date_naive();
        let mut state = match self.state.lock() {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[audit-sink] lock poisoned: {e}");
                return;
            }
        };

        if today != state.current_date {
            state.current_date = today;
            state.current_path = Self::path_for_date(&self.dir, today);
        }

        if let Err(e) = fs::create_dir_all(&self.dir) {
            eprintln!(
                "[audit-sink] cannot create audit dir {}: {e}",
                self.dir.display()
            );
            return;
        }

        let result = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&state.current_path)
            .and_then(|mut f| writeln!(f, "{json_line}"));

        if let Err(e) = result {
            eprintln!(
                "[audit-sink] write failed {}: {e}",
                state.current_path.display()
            );
        }
    }
}

impl<S> Layer<S> for FileAuditSink
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        let mut visitor = AuditVisitor::default();
        event.record(&mut visitor);

        if !visitor.is_audit {
            return;
        }

        let mut map = visitor.fields;
        map.insert(
            "timestamp".to_string(),
            serde_json::Value::String(Utc::now().to_rfc3339()),
        );

        match serde_json::to_string(&map) {
            Ok(line) => self.append_line(&line),
            Err(e) => eprintln!("[audit-sink] serialize failed: {e}"),
        }
    }
}

#[derive(Default)]
struct AuditVisitor {
    is_audit: bool,
    fields: serde_json::Map<String, serde_json::Value>,
}

impl Visit for AuditVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        let name = field.name();
        if name == "message" {
            let text = format!("{value:?}");
            if text == "security_audit_event" || text == "\"security_audit_event\"" {
                self.is_audit = true;
            }
            self.fields
                .insert(name.to_string(), serde_json::Value::String(text));
        } else {
            self.fields.insert(
                name.to_string(),
                serde_json::Value::String(format!("{value:?}")),
            );
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        let name = field.name();
        if name == "message" && value == "security_audit_event" {
            self.is_audit = true;
        }
        self.fields.insert(
            name.to_string(),
            serde_json::Value::String(value.to_string()),
        );
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.fields.insert(
            field.name().to_string(),
            serde_json::Value::Number(value.into()),
        );
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.fields.insert(
            field.name().to_string(),
            serde_json::Value::Number(value.into()),
        );
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.fields
            .insert(field.name().to_string(), serde_json::Value::Bool(value));
    }
}

pub fn read_audit_entries(
    audit_dir: &Path,
    since: Option<chrono::DateTime<Utc>>,
    limit: usize,
) -> Vec<serde_json::Value> {
    let mut files = match fs::read_dir(audit_dir) {
        Ok(entries) => entries
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                if name.starts_with("audit-") && name.ends_with(".jsonl") {
                    Some(e.path())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>(),
        Err(_) => return Vec::new(),
    };

    files.sort_unstable_by(|a, b| b.cmp(a));

    let mut results = Vec::new();

    for file_path in &files {
        let content = match fs::read_to_string(file_path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let mut file_entries: Vec<serde_json::Value> = content
            .lines()
            .filter(|line| !line.trim().is_empty())
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();

        file_entries.reverse();

        if let Some(since_ts) = since {
            file_entries.retain(|entry| {
                entry
                    .get("timestamp")
                    .and_then(|v| v.as_str())
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                    .map(|ts| ts >= since_ts)
                    .unwrap_or(false)
            });
        }

        for entry in file_entries {
            if results.len() >= limit {
                return results;
            }
            results.push(entry);
        }

        if results.len() >= limit {
            break;
        }
    }

    results
}
