use std::sync::Arc;
use std::time::Duration;

use rua_device_network::{DeviceNetworkRuntime, DeviceSessionManager};
use tokio::sync::{Mutex, MutexGuard};
use tokio::time::Instant;

use super::device_network_availability::{
    DeviceNetworkAvailability, DeviceNetworkAvailabilityState,
};

/// One resident runtime cell and its lifecycle readiness signal. Consumers wait
/// for the host-owned transition; they never initialize a competing runtime.
pub struct DeviceNetworkAccess {
    runtime: Mutex<Option<DeviceNetworkRuntime>>,
    availability: Arc<DeviceNetworkAvailability>,
}

impl DeviceNetworkAccess {
    pub fn new(
        runtime: Option<DeviceNetworkRuntime>,
        availability: Arc<DeviceNetworkAvailability>,
    ) -> Self {
        Self {
            runtime: Mutex::new(runtime),
            availability,
        }
    }

    pub fn try_lock(
        &self,
    ) -> Result<MutexGuard<'_, Option<DeviceNetworkRuntime>>, tokio::sync::TryLockError> {
        self.runtime.try_lock()
    }

    pub async fn lock(&self) -> MutexGuard<'_, Option<DeviceNetworkRuntime>> {
        self.runtime.lock().await
    }

    pub fn subscribe_availability(
        &self,
    ) -> tokio::sync::watch::Receiver<
        super::device_network_availability::DeviceNetworkAvailabilitySnapshot,
    > {
        self.availability.subscribe()
    }

    pub async fn session_manager(&self, deadline: Instant) -> Result<DeviceSessionManager, String> {
        let mut changes = self.availability.subscribe();
        tokio::time::timeout_at(deadline, async {
            loop {
                // Subscribe before reading the cell so installation cannot be missed.
                let snapshot = changes.borrow_and_update().clone();
                if let Some(runtime) = self.runtime.lock().await.as_ref() {
                    return Ok(runtime.session_manager().clone());
                }
                match snapshot.state {
                    DeviceNetworkAvailabilityState::Starting => {}
                    DeviceNetworkAvailabilityState::Failed => {
                        return Err(format!(
                            "Device Network Runtime initialization failed: {}",
                            snapshot.detail.as_deref().unwrap_or("unknown failure")
                        ))
                    }
                    DeviceNetworkAvailabilityState::Suspended => {
                        return Err("Device Network Runtime is suspended".into())
                    }
                    DeviceNetworkAvailabilityState::Ready => {
                        return Err("Device Network Runtime is unavailable".into())
                    }
                }
                changes
                    .changed()
                    .await
                    .map_err(|_| "Device Network Runtime lifecycle ended".to_string())?;
            }
        })
        .await
        .map_err(|_| "Device Network Runtime readiness timed out".to_string())?
    }
}

pub(crate) async fn current_session_manager(
    device_network: &Arc<DeviceNetworkAccess>,
) -> Result<DeviceSessionManager, String> {
    device_network
        .session_manager(Instant::now() + Duration::from_secs(10))
        .await
}
