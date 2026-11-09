use std::sync::{Arc, RwLock};
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

pub const USER_NOTIFICATION_WIRE_VERSION: u16 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserNotificationSender {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserNotificationRequest {
    pub target_device_id: String,
    pub sender: UserNotificationSender,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rua_uri: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "message_type", content = "payload", rename_all = "snake_case")]
pub enum UserNotificationWireEnvelope {
    UserNotification {
        version: u16,
        request: UserNotificationRequest,
    },
}

impl UserNotificationWireEnvelope {
    pub fn new(request: UserNotificationRequest) -> Self {
        Self::UserNotification {
            version: USER_NOTIFICATION_WIRE_VERSION,
            request,
        }
    }

    pub fn into_current_request(self) -> Option<UserNotificationRequest> {
        match self {
            Self::UserNotification { version, request }
                if version == USER_NOTIFICATION_WIRE_VERSION =>
            {
                Some(request)
            }
            Self::UserNotification { .. } => None,
        }
    }
}

#[async_trait]
pub trait LocalUserNotificationAdapter: Send + Sync {
    async fn present(&self, request: UserNotificationRequest) -> bool;
}

#[async_trait]
pub trait RemoteUserNotificationTransport: Send + Sync {
    async fn send(&self, request: UserNotificationRequest) -> bool;
}

pub struct UserNotificationService {
    local_device_id: String,
    local_adapter: Arc<dyn LocalUserNotificationAdapter>,
    remote_transport: Arc<dyn RemoteUserNotificationTransport>,
}

impl std::fmt::Debug for UserNotificationService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("UserNotificationService")
            .field("local_device_id", &self.local_device_id)
            .finish_non_exhaustive()
    }
}

impl UserNotificationService {
    pub fn new(
        local_device_id: impl Into<String>,
        local_adapter: Arc<dyn LocalUserNotificationAdapter>,
        remote_transport: Arc<dyn RemoteUserNotificationTransport>,
    ) -> Self {
        Self {
            local_device_id: local_device_id.into(),
            local_adapter,
            remote_transport,
        }
    }

    pub async fn send(&self, request: UserNotificationRequest) {
        if request.target_device_id == self.local_device_id {
            let _ = self.local_adapter.present(request).await;
        } else {
            let _ = self.remote_transport.send(request).await;
        }
    }

    pub async fn receive(&self, request: UserNotificationRequest) {
        if request.target_device_id == self.local_device_id {
            let _ = self.local_adapter.present(request).await;
        }
    }
}

pub struct DeviceSessionUserNotificationTransport {
    session_manager: Arc<RwLock<Option<rua_device_network::DeviceSessionManager>>>,
    timeout: Duration,
}

impl std::fmt::Debug for DeviceSessionUserNotificationTransport {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DeviceSessionUserNotificationTransport")
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

impl DeviceSessionUserNotificationTransport {
    pub fn new(
        session_manager: Arc<RwLock<Option<rua_device_network::DeviceSessionManager>>>,
        timeout: Duration,
    ) -> Self {
        Self {
            session_manager,
            timeout,
        }
    }
}

#[async_trait]
impl RemoteUserNotificationTransport for DeviceSessionUserNotificationTransport {
    async fn send(&self, request: UserNotificationRequest) -> bool {
        let session_manager = self
            .session_manager
            .read()
            .ok()
            .and_then(|guard| guard.as_ref().cloned());
        let Some(session_manager) = session_manager else {
            return false;
        };
        let target_device_id = request.target_device_id.clone();
        let Ok(payload) = serde_json::to_vec(&UserNotificationWireEnvelope::new(request)) else {
            return false;
        };
        // Include peer address resolution in the notification delivery deadline.
        matches!(
            tokio::time::timeout(
                self.timeout,
                session_manager.command_once_to(&target_device_id, &payload, self.timeout),
            )
            .await,
            Ok(Ok(_))
        )
    }
}
