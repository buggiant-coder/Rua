use std::sync::Arc;

use chrono::Utc;
use rua_channel::core::ChannelManager;
use rua_tunnel::{CloudflareTunnelDriver, TunnelEvent};

use crate::state::TunnelAlertState;

pub(crate) fn spawn_tunnel_event_subscriber(
    tunnel_driver: Arc<tokio::sync::Mutex<Option<CloudflareTunnelDriver>>>,
    alert_state: Arc<tokio::sync::RwLock<TunnelAlertState>>,
    channel_manager: Arc<ChannelManager>,
) {
    tokio::spawn(async move {
        tunnel_event_subscriber_loop(tunnel_driver, alert_state, channel_manager).await;
    });
    tracing::info!("Tunnel event subscriber spawned");
}

pub async fn tunnel_event_subscriber_loop(
    tunnel_driver: Arc<tokio::sync::Mutex<Option<CloudflareTunnelDriver>>>,
    alert_state: Arc<tokio::sync::RwLock<TunnelAlertState>>,
    channel_manager: Arc<ChannelManager>,
) {
    loop {
        let mut rx = loop {
            let guard = tunnel_driver.lock().await;
            if let Some(driver) = guard.as_ref() {
                let rx = driver.subscribe_events();
                {
                    let mut s = alert_state.write().await;
                    s.state = "connecting".to_string();
                }
                break rx;
            }
            drop(guard);
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        };

        loop {
            match rx.recv().await {
                Ok(TunnelEvent::Started { url }) => {
                    tracing::info!(url = %url, "Tunnel started (first connect)");
                    let now = Utc::now();
                    let mut s = alert_state.write().await;
                    s.state = "connected".to_string();
                    s.started_at = Some(now);
                }
                Ok(TunnelEvent::AllRetriesFailed) => {
                    tracing::warn!("Tunnel AllRetriesFailed — broadcasting alert");
                    let now = Utc::now();
                    {
                        let mut s = alert_state.write().await;
                        s.state = "failed".to_string();
                        s.last_failure_at = Some(now);
                    }
                    let alert_text = "Tunnel connection failed: all restart attempts exhausted";
                    let results = channel_manager.broadcast_text(alert_text).await;
                    for (ch_id, res) in &results {
                        if let Err(e) = res {
                            tracing::error!(
                                channel_id = %ch_id,
                                error = %e,
                                "failed to broadcast tunnel alert"
                            );
                        }
                    }
                }
                Ok(TunnelEvent::Restarted { url }) => {
                    tracing::info!(url = %url, "Tunnel restarted");
                    let mut s = alert_state.write().await;
                    s.state = "connected".to_string();
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    tracing::debug!("Tunnel event channel closed, will re-subscribe");
                    let mut s = alert_state.write().await;
                    s.state = "disconnected".to_string();
                    break;
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(skipped = n, "Tunnel event subscriber lagged");
                }
            }
        }
    }
}
