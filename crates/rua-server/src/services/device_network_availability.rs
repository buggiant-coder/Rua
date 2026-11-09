use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tokio::sync::watch;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceNetworkAvailabilityState {
    Suspended,
    Starting,
    Ready,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeviceNetworkAvailabilitySnapshot {
    pub revision: u64,
    pub state: DeviceNetworkAvailabilityState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

pub struct DeviceNetworkAvailability {
    snapshots: watch::Sender<DeviceNetworkAvailabilitySnapshot>,
    transition_lock: Mutex<()>,
}

impl DeviceNetworkAvailability {
    pub fn new() -> Self {
        let (snapshots, _) = watch::channel(DeviceNetworkAvailabilitySnapshot {
            revision: revision_now(),
            state: DeviceNetworkAvailabilityState::Suspended,
            detail: None,
        });
        Self {
            snapshots,
            transition_lock: Mutex::new(()),
        }
    }

    pub fn snapshot(&self) -> DeviceNetworkAvailabilitySnapshot {
        self.snapshots.borrow().clone()
    }

    pub fn subscribe(&self) -> watch::Receiver<DeviceNetworkAvailabilitySnapshot> {
        self.snapshots.subscribe()
    }

    pub fn mark_suspended(&self) {
        self.transition(DeviceNetworkAvailabilityState::Suspended, None);
    }

    pub fn mark_starting(&self) {
        self.transition(DeviceNetworkAvailabilityState::Starting, None);
    }

    pub fn mark_ready(&self) {
        self.transition(DeviceNetworkAvailabilityState::Ready, None);
    }

    pub fn mark_failed(&self, detail: impl Into<String>) {
        self.transition(DeviceNetworkAvailabilityState::Failed, Some(detail.into()));
    }

    fn transition(&self, state: DeviceNetworkAvailabilityState, detail: Option<String>) {
        let _guard = self
            .transition_lock
            .lock()
            .expect("device network availability transition lock");
        let current = self.snapshots.borrow().clone();
        if current.state == state && current.detail == detail {
            return;
        }
        self.snapshots
            .send_replace(DeviceNetworkAvailabilitySnapshot {
                revision: current.revision.saturating_add(1).max(revision_now()),
                state,
                detail,
            });
    }
}

fn revision_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

impl Default for DeviceNetworkAvailability {
    fn default() -> Self {
        Self::new()
    }
}
