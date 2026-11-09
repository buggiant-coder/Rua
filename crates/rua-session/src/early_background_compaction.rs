use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use tokio_util::sync::CancellationToken;

use crate::projection::{ProjectionFitStatus, ProjectionPlan};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EarlyBackgroundCompactionPriority {
    Low,
    High,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EarlyBackgroundCompactionDecision {
    DoNothing,
    ScheduleCheckpoint {
        priority: EarlyBackgroundCompactionPriority,
        reason: String,
    },
    SignalEmergencyFallbackRequired,
}

#[derive(Debug, Clone)]
pub struct EarlyBackgroundCompactionInput {
    pub plan: ProjectionPlan,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct EarlyBackgroundCompactionPolicy;

impl EarlyBackgroundCompactionPolicy {
    pub fn evaluate(
        &self,
        input: EarlyBackgroundCompactionInput,
    ) -> EarlyBackgroundCompactionDecision {
        match input.plan.status {
            ProjectionFitStatus::FitsTarget => EarlyBackgroundCompactionDecision::DoNothing,
            ProjectionFitStatus::FitsEmergencyButNeedsMaintenance => {
                EarlyBackgroundCompactionDecision::ScheduleCheckpoint {
                    priority: EarlyBackgroundCompactionPriority::Low,
                    reason: "target_band_exceeded".to_string(),
                }
            }
            ProjectionFitStatus::RequiresCheckpoint => {
                EarlyBackgroundCompactionDecision::ScheduleCheckpoint {
                    priority: EarlyBackgroundCompactionPriority::High,
                    reason: "checkpoint_required".to_string(),
                }
            }
            ProjectionFitStatus::RequiresEmergencyFallback => {
                EarlyBackgroundCompactionDecision::SignalEmergencyFallbackRequired
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct EarlyBackgroundCompactionLease {
    pub chat_id: String,
    pub generation: u64,
    cancellation: CancellationToken,
    state: Arc<Mutex<HashMap<String, ChatEarlyBackgroundCompactionState>>>,
}

impl EarlyBackgroundCompactionLease {
    pub fn is_cancelled(&self) -> bool {
        if self.cancellation.is_cancelled() {
            return true;
        }
        let state = self.state.lock().unwrap();
        state
            .get(&self.chat_id)
            .is_none_or(|current| current.generation != self.generation)
    }

    pub async fn cancelled(&self) {
        self.cancellation.cancelled().await;
    }
}

#[derive(Debug, Clone)]
struct ChatEarlyBackgroundCompactionState {
    generation: u64,
    cancellation: CancellationToken,
}

#[derive(Debug, Default, Clone)]
pub struct EarlyBackgroundCompactionCoordinator {
    state: Arc<Mutex<HashMap<String, ChatEarlyBackgroundCompactionState>>>,
}

impl EarlyBackgroundCompactionCoordinator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn start_job(&self, chat_id: impl Into<String>) -> EarlyBackgroundCompactionLease {
        let chat_id = chat_id.into();
        let mut state = self.state.lock().unwrap();
        if let Some(current) = state.get(&chat_id) {
            current.cancellation.cancel();
        }
        let next_generation = state
            .get(&chat_id)
            .map(|current| current.generation + 1)
            .unwrap_or(1);
        let cancellation = CancellationToken::new();
        state.insert(
            chat_id.clone(),
            ChatEarlyBackgroundCompactionState {
                generation: next_generation,
                cancellation: cancellation.clone(),
            },
        );
        EarlyBackgroundCompactionLease {
            chat_id,
            generation: next_generation,
            cancellation,
            state: Arc::clone(&self.state),
        }
    }

    pub fn try_start_job(
        &self,
        chat_id: impl Into<String>,
    ) -> Option<EarlyBackgroundCompactionLease> {
        let chat_id = chat_id.into();
        let mut state = self.state.lock().unwrap();
        if state
            .get(&chat_id)
            .is_some_and(|current| !current.cancellation.is_cancelled())
        {
            return None;
        }
        let next_generation = state
            .get(&chat_id)
            .map(|current| current.generation + 1)
            .unwrap_or(1);
        let cancellation = CancellationToken::new();
        state.insert(
            chat_id.clone(),
            ChatEarlyBackgroundCompactionState {
                generation: next_generation,
                cancellation: cancellation.clone(),
            },
        );
        Some(EarlyBackgroundCompactionLease {
            chat_id,
            generation: next_generation,
            cancellation,
            state: Arc::clone(&self.state),
        })
    }

    pub fn preempt_for_foreground(&self, chat_id: &str) {
        let mut state = self.state.lock().unwrap();
        let next_generation = state
            .get(chat_id)
            .map(|current| current.generation + 1)
            .unwrap_or(1);
        if let Some(current) = state.get(chat_id) {
            current.cancellation.cancel();
        }
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        state.insert(
            chat_id.to_string(),
            ChatEarlyBackgroundCompactionState {
                generation: next_generation,
                cancellation,
            },
        );
    }

    pub fn can_publish(&self, lease: &EarlyBackgroundCompactionLease) -> bool {
        !lease.is_cancelled()
    }

    pub fn finish_job(&self, lease: &EarlyBackgroundCompactionLease) {
        let mut state = self.state.lock().unwrap();
        if state
            .get(&lease.chat_id)
            .is_some_and(|current| current.generation == lease.generation)
        {
            state.remove(&lease.chat_id);
        }
    }
}

static EARLY_BACKGROUND_COMPACTION_COORDINATOR: OnceLock<EarlyBackgroundCompactionCoordinator> =
    OnceLock::new();

pub fn early_background_compaction_coordinator() -> &'static EarlyBackgroundCompactionCoordinator {
    EARLY_BACKGROUND_COMPACTION_COORDINATOR.get_or_init(EarlyBackgroundCompactionCoordinator::new)
}
