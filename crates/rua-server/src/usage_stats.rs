use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, RwLock};

const FILE_NAME: &str = "usage_stats.json";
const FLUSH_INTERVAL: Duration = Duration::from_secs(30);

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct AgentCounters {
    #[serde(default)]
    pub calls: u64,
    #[serde(default)]
    pub last_used: Option<DateTime<Utc>>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct McpCounters {
    #[serde(default)]
    pub calls: u64,
    #[serde(default)]
    pub last_used: Option<DateTime<Utc>>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct StatsFile {
    #[serde(default)]
    agents: HashMap<String, AgentCounters>,
    #[serde(default)]
    mcp: HashMap<String, McpCounters>,
}

pub struct UsageStats {
    path: PathBuf,
    inner: RwLock<StatsFile>,
    dirty: RwLock<bool>,
    flush_lock: Mutex<()>,
}

impl UsageStats {
    pub fn load(data_dir: &Path) -> Arc<Self> {
        let path = data_dir.join(FILE_NAME);
        let (inner, existed) = match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<StatsFile>(&bytes) {
                Ok(parsed) => (parsed, true),
                Err(e) => {
                    tracing::warn!(
                        path = %path.display(),
                        error = %e,
                        "usage_stats.json parse failed, starting fresh"
                    );
                    (StatsFile::default(), false)
                }
            },
            Err(_) => (StatsFile::default(), false),
        };
        tracing::info!(
            path = %path.display(),
            existed,
            "usage_stats loaded"
        );
        Arc::new(Self {
            path,
            inner: RwLock::new(inner),
            dirty: RwLock::new(false),
            flush_lock: Mutex::new(()),
        })
    }

    pub async fn bump_agent(&self, agent_id: &str) {
        let mut g = self.inner.write().await;
        let entry = g.agents.entry(agent_id.to_string()).or_default();
        entry.calls += 1;
        entry.last_used = Some(Utc::now());
        drop(g);
        *self.dirty.write().await = true;
    }

    pub async fn bump_mcp(&self, agent_id: &str, server_id: &str) {
        let key = format!("{agent_id}/{server_id}");
        let mut g = self.inner.write().await;
        let entry = g.mcp.entry(key).or_default();
        entry.calls += 1;
        entry.last_used = Some(Utc::now());
        drop(g);
        *self.dirty.write().await = true;
    }

    pub async fn agents_to_preheat(
        &self,
        min_calls: u64,
        max_age_days: i64,
        skip_agent_id: &str,
    ) -> Vec<String> {
        let cutoff = Utc::now() - chrono::Duration::days(max_age_days);
        let g = self.inner.read().await;
        let mut out: Vec<(String, u64)> = g
            .agents
            .iter()
            .filter(|(id, c)| {
                id.as_str() != skip_agent_id
                    && c.calls >= min_calls
                    && c.last_used.map(|t| t >= cutoff).unwrap_or(false)
            })
            .map(|(id, c)| (id.clone(), c.calls))
            .collect();
        out.sort_by_key(|item| std::cmp::Reverse(item.1));
        out.into_iter().map(|(id, _)| id).collect()
    }

    pub async fn flush_if_dirty(&self) {
        let _flush = self.flush_lock.lock().await;
        {
            let mut dirty = self.dirty.write().await;
            if !*dirty {
                return;
            }
            *dirty = false;
        }

        let snapshot = self.inner.read().await.clone();
        let value = match serde_json::to_value(&snapshot) {
            Ok(value) => value,
            Err(e) => {
                tracing::warn!(error = %e, "usage_stats serialize failed");
                *self.dirty.write().await = true;
                return;
            }
        };
        if let Err(e) = rua_config::atomic_write_json(&self.path, &value) {
            tracing::warn!(path = %self.path.display(), error = %e, "usage_stats atomic write failed");
            *self.dirty.write().await = true;
        }
    }
}

pub fn spawn_flush_task(
    stats: Arc<UsageStats>,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(FLUSH_INTERVAL);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = tick.tick() => stats.flush_if_dirty().await,
                changed = shutdown_rx.changed() => {
                    match changed {
                        Ok(()) if *shutdown_rx.borrow() => {
                            stats.flush_if_dirty().await;
                            return;
                        }
                        Ok(()) => {}
                        Err(_) => {
                            stats.flush_if_dirty().await;
                            return;
                        }
                    }
                }
            }
        }
    })
}
