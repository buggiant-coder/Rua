use async_trait::async_trait;
use tokio::sync::broadcast;

use crate::services::user_notification::{LocalUserNotificationAdapter, UserNotificationRequest};

#[derive(Debug)]
pub struct ConsoleUserNotificationAdapter {
    notifications: broadcast::Sender<UserNotificationRequest>,
}

impl ConsoleUserNotificationAdapter {
    pub fn new() -> Self {
        let (notifications, _) = broadcast::channel(64);
        Self { notifications }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<UserNotificationRequest> {
        self.notifications.subscribe()
    }
}

impl Default for ConsoleUserNotificationAdapter {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl LocalUserNotificationAdapter for ConsoleUserNotificationAdapter {
    async fn present(&self, request: UserNotificationRequest) -> bool {
        self.notifications.send(request).is_ok()
    }
}
