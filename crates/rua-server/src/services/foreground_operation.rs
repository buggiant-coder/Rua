use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, watch};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ForegroundOperationKind {
    ConversationExecution,
    ChatViewer,
    TerminalViewer,
    AirdropTransfer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MobileResidencyState {
    Foreground,
    Active,
    BackgroundGrace,
    ResidentIdle,
    Suspended,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ForegroundOperation {
    pub lease_id: String,
    pub operation_id: String,
    pub kind: ForegroundOperationKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_device_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_device_id: Option<String>,
    pub bytes_written: u64,
    pub total_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ForegroundOperationSnapshot {
    pub revision: u64,
    pub background: bool,
    pub active_count: usize,
    pub locks_required: bool,
    pub residency_state: MobileResidencyState,
    pub grace_generation: u64,
    pub operations: Vec<ForegroundOperation>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForegroundOperationTransition {
    AcquiredWhileBackground,
    FinalReleasedWhileBackground { grace_generation: u64 },
}

struct ForegroundOperationEntry {
    operation: ForegroundOperation,
    last_updated: Instant,
}

struct ForegroundOperationState {
    enabled: bool,
    background: bool,
    allow_background_runtime: bool,
    grace_active: bool,
    grace_generation: u64,
    suspended: bool,
    revision: u64,
    operations: BTreeMap<String, ForegroundOperationEntry>,
}

pub struct ForegroundOperationRegistry {
    state: Mutex<ForegroundOperationState>,
    snapshots: watch::Sender<ForegroundOperationSnapshot>,
    transitions: broadcast::Sender<ForegroundOperationTransition>,
}

impl ForegroundOperationRegistry {
    pub fn new(enabled: bool) -> Self {
        let snapshot = ForegroundOperationSnapshot {
            revision: 0,
            background: false,
            active_count: 0,
            locks_required: false,
            residency_state: MobileResidencyState::Foreground,
            grace_generation: 0,
            operations: Vec::new(),
        };
        let (snapshots, _) = watch::channel(snapshot);
        let (transitions, _) = broadcast::channel(32);
        Self {
            state: Mutex::new(ForegroundOperationState {
                enabled,
                background: false,
                allow_background_runtime: false,
                grace_active: false,
                grace_generation: 0,
                suspended: false,
                revision: 0,
                operations: BTreeMap::new(),
            }),
            snapshots,
            transitions,
        }
    }

    pub fn acquire(
        &self,
        lease_id: String,
        operation_id: String,
        kind: ForegroundOperationKind,
    ) -> bool {
        self.acquire_operation(ForegroundOperation {
            lease_id,
            operation_id,
            kind,
            source_device_id: None,
            target_device_id: None,
            bytes_written: 0,
            total_bytes: 0,
        })
    }

    pub fn acquire_transfer(
        &self,
        lease_id: String,
        operation_id: String,
        source_device_id: String,
        target_device_id: String,
        total_bytes: u64,
    ) -> bool {
        if source_device_id == target_device_id {
            return false;
        }
        self.acquire_operation(ForegroundOperation {
            lease_id,
            operation_id,
            kind: ForegroundOperationKind::AirdropTransfer,
            source_device_id: Some(source_device_id),
            target_device_id: Some(target_device_id),
            bytes_written: 0,
            total_bytes,
        })
    }

    fn acquire_operation(&self, operation: ForegroundOperation) -> bool {
        let mut state = self.state.lock().expect("foreground operations lock");
        if !state.enabled {
            return false;
        }
        if let Some(existing) = state.operations.get_mut(&operation.lease_id) {
            if existing.operation.operation_id != operation.operation_id
                || existing.operation.kind != operation.kind
            {
                return false;
            }
            existing.last_updated = Instant::now();
            return true;
        }
        let was_empty = state.operations.is_empty();
        state.operations.insert(
            operation.lease_id.clone(),
            ForegroundOperationEntry {
                operation,
                last_updated: Instant::now(),
            },
        );
        if state.background {
            state.grace_active = false;
            state.suspended = false;
        }
        self.publish_locked(&mut state);
        if was_empty && state.background {
            let _ = self
                .transitions
                .send(ForegroundOperationTransition::AcquiredWhileBackground);
        }
        true
    }

    pub fn update_transfer(&self, lease_id: &str, bytes_written: u64, total_bytes: u64) {
        let mut state = self.state.lock().expect("foreground operations lock");
        let Some(entry) = state.operations.get_mut(lease_id) else {
            return;
        };
        if entry.operation.kind != ForegroundOperationKind::AirdropTransfer {
            return;
        }
        entry.operation.bytes_written = bytes_written;
        entry.operation.total_bytes = total_bytes;
        entry.last_updated = Instant::now();
        self.publish_locked(&mut state);
    }

    pub fn touch(&self, lease_id: &str) -> bool {
        let mut state = self.state.lock().expect("foreground operations lock");
        let Some(entry) = state.operations.get_mut(lease_id) else {
            return false;
        };
        entry.last_updated = Instant::now();
        true
    }

    pub fn release(&self, lease_id: &str) -> bool {
        let mut state = self.state.lock().expect("foreground operations lock");
        if state.operations.remove(lease_id).is_none() {
            return false;
        }
        let grace_generation = self.begin_grace_after_final_release_locked(&mut state);
        self.publish_locked(&mut state);
        if let Some(grace_generation) = grace_generation {
            let _ = self.transitions.send(
                ForegroundOperationTransition::FinalReleasedWhileBackground { grace_generation },
            );
        }
        true
    }

    pub fn expire_stale(&self, max_idle: Duration) -> Vec<String> {
        let mut state = self.state.lock().expect("foreground operations lock");
        let expired = state
            .operations
            .iter()
            .filter(|(_, entry)| entry.last_updated.elapsed() >= max_idle)
            .map(|(lease_id, _)| lease_id.clone())
            .collect::<Vec<_>>();
        let had_active = !state.operations.is_empty();
        for lease_id in &expired {
            state.operations.remove(lease_id);
        }
        if !expired.is_empty() {
            let grace_generation = had_active
                .then(|| self.begin_grace_after_final_release_locked(&mut state))
                .flatten();
            self.publish_locked(&mut state);
            if let Some(grace_generation) = grace_generation {
                let _ = self.transitions.send(
                    ForegroundOperationTransition::FinalReleasedWhileBackground {
                        grace_generation,
                    },
                );
            }
        }
        expired
    }

    pub fn enter_background(&self, allow_background_runtime: bool) -> Option<u64> {
        let mut state = self.state.lock().expect("foreground operations lock");
        if !state.enabled {
            return None;
        }
        if state.background && state.allow_background_runtime == allow_background_runtime {
            return None;
        }
        state.background = true;
        state.allow_background_runtime = allow_background_runtime;
        state.suspended = false;
        let generation = if state.operations.is_empty() && !allow_background_runtime {
            state.grace_generation = state.grace_generation.wrapping_add(1);
            state.grace_active = true;
            Some(state.grace_generation)
        } else {
            state.grace_active = false;
            None
        };
        self.publish_locked(&mut state);
        generation
    }

    pub fn enter_foreground(&self) {
        let mut state = self.state.lock().expect("foreground operations lock");
        if !state.enabled || (!state.background && !state.grace_active && !state.suspended) {
            return;
        }
        state.background = false;
        state.allow_background_runtime = false;
        state.grace_active = false;
        state.suspended = false;
        self.publish_locked(&mut state);
    }

    pub fn mark_suspended(&self, grace_generation: u64) -> bool {
        let mut state = self.state.lock().expect("foreground operations lock");
        if !state.enabled
            || !state.background
            || !state.operations.is_empty()
            || !state.grace_active
            || state.grace_generation != grace_generation
        {
            return false;
        }
        state.grace_active = false;
        state.suspended = true;
        self.publish_locked(&mut state);
        true
    }

    pub fn has_active(&self) -> bool {
        !self
            .state
            .lock()
            .expect("foreground operations lock")
            .operations
            .is_empty()
    }

    pub fn snapshot(&self) -> ForegroundOperationSnapshot {
        snapshot_from_state(&self.state.lock().expect("foreground operations lock"))
    }

    pub fn subscribe(&self) -> watch::Receiver<ForegroundOperationSnapshot> {
        self.snapshots.subscribe()
    }

    pub fn subscribe_transitions(&self) -> broadcast::Receiver<ForegroundOperationTransition> {
        self.transitions.subscribe()
    }

    fn publish_locked(&self, state: &mut ForegroundOperationState) {
        state.revision = state.revision.wrapping_add(1);
        self.snapshots.send_replace(snapshot_from_state(state));
    }

    fn begin_grace_after_final_release_locked(
        &self,
        state: &mut ForegroundOperationState,
    ) -> Option<u64> {
        if !state.operations.is_empty() || !state.background || state.allow_background_runtime {
            return None;
        }
        state.grace_generation = state.grace_generation.wrapping_add(1);
        state.grace_active = true;
        state.suspended = false;
        Some(state.grace_generation)
    }
}

impl Default for ForegroundOperationRegistry {
    fn default() -> Self {
        Self::new(false)
    }
}

fn snapshot_from_state(state: &ForegroundOperationState) -> ForegroundOperationSnapshot {
    let locks_required = !state.operations.is_empty()
        || state.grace_active
        || (state.background && state.allow_background_runtime);
    let residency_state = if !state.operations.is_empty() {
        MobileResidencyState::Active
    } else if !state.background {
        MobileResidencyState::Foreground
    } else if state.grace_active {
        MobileResidencyState::BackgroundGrace
    } else if state.suspended {
        MobileResidencyState::Suspended
    } else {
        MobileResidencyState::ResidentIdle
    };
    ForegroundOperationSnapshot {
        revision: state.revision,
        background: state.background,
        active_count: state.operations.len(),
        locks_required,
        residency_state,
        grace_generation: state.grace_generation,
        operations: state
            .operations
            .values()
            .map(|entry| entry.operation.clone())
            .collect(),
    }
}
