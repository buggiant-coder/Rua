use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::RwLock;

use crate::endpoints::dto::McpToolInfo;

const DEFAULT_TTL: Duration = Duration::from_secs(600);

#[derive(Clone)]
struct CacheEntry {
    tools: Vec<McpToolInfo>,
    fetched_at: Instant,
}

pub struct McpToolsCache {
    inner: RwLock<HashMap<(String, String), CacheEntry>>,
    ttl: Duration,
}

impl McpToolsCache {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: RwLock::new(HashMap::new()),
            ttl: DEFAULT_TTL,
        })
    }

    pub async fn get(&self, agent_id: &str, server_id: &str) -> Option<Vec<McpToolInfo>> {
        let key = (agent_id.to_string(), server_id.to_string());
        let entry = {
            let g = self.inner.read().await;
            g.get(&key).cloned()
        };
        let entry = entry?;
        if entry.fetched_at.elapsed() >= self.ttl {
            return None;
        }
        Some(entry.tools)
    }

    pub async fn put(&self, agent_id: &str, server_id: &str, tools: Vec<McpToolInfo>) {
        let key = (agent_id.to_string(), server_id.to_string());
        let entry = CacheEntry {
            tools,
            fetched_at: Instant::now(),
        };
        self.inner.write().await.insert(key, entry);
    }

    pub async fn invalidate(&self, agent_id: &str, server_id: &str) {
        self.inner
            .write()
            .await
            .remove(&(agent_id.to_string(), server_id.to_string()));
    }

    pub async fn invalidate_agent(&self, agent_id: &str) {
        self.inner
            .write()
            .await
            .retain(|(aid, _), _| aid != agent_id);
    }
}
