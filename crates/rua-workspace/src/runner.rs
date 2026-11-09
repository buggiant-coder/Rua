use std::collections::{HashMap, HashSet, VecDeque};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use rua_channel::core::SessionCanceller;

use crate::error::WorkspaceError;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "policy", rename_all = "snake_case")]
pub enum SessionConcurrencyPolicy {
    Reject,

    Queue {
        max_queue: usize,
    },

    #[default]
    CancelPrevious,
}

pub struct ActiveSessionTracker {
    active: RwLock<HashMap<String, ActiveSessionEntry>>,

    queues: RwLock<HashMap<String, VecDeque<String>>>,

    policy: SessionConcurrencyPolicy,
}

#[derive(Debug, Clone)]
struct ActiveSessionEntry {
    request_id: String,

    #[allow(dead_code)]
    agent_id: String,

    cancel: tokio_util::sync::CancellationToken,
}

#[derive(Debug)]
pub enum SessionAcquireResult {
    Acquired,

    CancelledPrevious { previous_request_id: String },

    Queued { position: usize },

    Rejected,
}

impl ActiveSessionTracker {
    pub fn new(policy: SessionConcurrencyPolicy) -> Self {
        Self {
            active: RwLock::new(HashMap::new()),
            queues: RwLock::new(HashMap::new()),
            policy,
        }
    }

    pub async fn try_acquire(
        &self,
        session_id: &str,
        request_id: &str,
        agent_id: &str,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<SessionAcquireResult, WorkspaceError> {
        let mut active = self.active.write().await;

        if let Some(existing) = active.get(session_id) {
            match &self.policy {
                SessionConcurrencyPolicy::Reject => {
                    return Ok(SessionAcquireResult::Rejected);
                }
                SessionConcurrencyPolicy::Queue { max_queue } => {
                    drop(active);
                    let mut queues = self.queues.write().await;
                    let queue = queues.entry(session_id.to_string()).or_default();
                    if queue.len() >= *max_queue {
                        return Err(WorkspaceError::session_busy(format!(
                            "Session {} queue full (max {})",
                            session_id, max_queue
                        )));
                    }
                    queue.push_back(request_id.to_string());
                    return Ok(SessionAcquireResult::Queued {
                        position: queue.len(),
                    });
                }
                SessionConcurrencyPolicy::CancelPrevious => {
                    let prev_id = existing.request_id.clone();
                    existing.cancel.cancel();

                    active.insert(
                        session_id.to_string(),
                        ActiveSessionEntry {
                            request_id: request_id.to_string(),
                            agent_id: agent_id.to_string(),
                            cancel,
                        },
                    );
                    return Ok(SessionAcquireResult::CancelledPrevious {
                        previous_request_id: prev_id,
                    });
                }
            }
        }

        active.insert(
            session_id.to_string(),
            ActiveSessionEntry {
                request_id: request_id.to_string(),
                agent_id: agent_id.to_string(),
                cancel,
            },
        );

        Ok(SessionAcquireResult::Acquired)
    }

    pub async fn release(&self, session_id: &str, request_id: &str) {
        let mut active = self.active.write().await;
        if let Some(entry) = active.get(session_id) {
            if entry.request_id == request_id {
                active.remove(session_id);
            }
        }
    }

    pub async fn is_active(&self, session_id: &str) -> bool {
        self.active.read().await.contains_key(session_id)
    }

    pub async fn active_sessions(&self) -> HashSet<String> {
        self.active.read().await.keys().cloned().collect()
    }

    pub async fn active_count(&self) -> usize {
        self.active.read().await.len()
    }

    pub async fn cancel_session(&self, session_id: &str) -> bool {
        let active = self.active.read().await;
        if let Some(entry) = active.get(session_id) {
            entry.cancel.cancel();
            true
        } else {
            false
        }
    }

    pub async fn cancel_all(&self) -> usize {
        let active = self.active.read().await;
        let count = active.len();
        for entry in active.values() {
            entry.cancel.cancel();
        }
        count
    }

    pub async fn active_request_id(&self, session_id: &str) -> Option<String> {
        self.active
            .read()
            .await
            .get(session_id)
            .map(|e| e.request_id.clone())
    }
}

#[async_trait]
impl SessionCanceller for ActiveSessionTracker {
    async fn cancel_session(&self, session_id: &str) -> bool {
        self.cancel_session(session_id).await
    }

    async fn active_session_count(&self) -> usize {
        self.active.read().await.len()
    }
}
