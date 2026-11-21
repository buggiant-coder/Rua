use rua_device_network::{DeviceConnectivityState, DeviceSessionManager};
use tokio::sync::watch;

use super::ChatTaskService;
use crate::services::conversation_input_intake::ConversationInputIntake;

impl ChatTaskService {
    /// Retry only persisted result deliveries when connectivity returns. Never
    /// replay Task acceptance, commands or controls after a transport failure.
    pub fn spawn_result_recovery(
        &self,
        intake: ConversationInputIntake,
        mut shutdown: watch::Receiver<bool>,
    ) {
        let network = self.network.clone();
        let mut availability = network.subscribe_availability();
        tokio::spawn(async move {
            let mut sessions: Option<(DeviceSessionManager, watch::Receiver<u64>)> = None;
            let mut should_resume = true;
            let mut retry = tokio::time::interval_at(
                tokio::time::Instant::now() + std::time::Duration::from_secs(30),
                std::time::Duration::from_secs(30),
            );
            retry.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                if *shutdown.borrow() {
                    return;
                }
                let manager = network
                    .lock()
                    .await
                    .as_ref()
                    .map(|runtime| runtime.session_manager().clone());
                if !matches!((&manager, &sessions), (Some(next), Some((current, _))) if next.same_instance(current))
                {
                    sessions = manager.map(|manager| {
                        let changes = manager.subscribe_connectivity_changes();
                        (manager, changes)
                    });
                    should_resume = true;
                }
                if should_resume {
                    tokio::select! {
                        _ = shutdown.wait_for(|stopping| *stopping) => return,
                        result = intake.resume_task_results(None) => {
                            if let Err(error) = result {
                                tracing::warn!(%error, "Persisted Task results remain pending");
                            }
                        }
                    }
                }
                should_resume = false;
                tokio::select! {
                    _ = shutdown.wait_for(|stopping| *stopping) => return,
                    // An acknowledgement can be lost while the same connection
                    // remains healthy. Only persisted pending deliveries are
                    // retried; the Task execution itself is never replayed.
                    _ = retry.tick() => should_resume = true,
                    changed = availability.changed() => if changed.is_err() { return; },
                    changed = async {
                        match sessions.as_mut() {
                            Some((_, changes)) => changes.changed().await,
                            None => std::future::pending().await,
                        }
                    } => match changed {
                        Err(_) => sessions = None,
                        Ok(()) => {
                            // A connection can be replaced without an observable
                            // offline interval. The revision, not just the peer set,
                            // establishes that pending receipts can be retried.
                            should_resume = sessions.as_ref().is_some_and(|(manager, _)|
                                manager.connectivity().iter().any(|peer|
                                    peer.state == DeviceConnectivityState::Connected));
                        }
                    },
                }
            }
        });
    }
}
