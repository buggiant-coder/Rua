use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamPolicy {
    pub reconnect_grace_ms: u64,

    pub max_buffer_events: usize,

    pub cancel_on_disconnect: bool,
}

impl Default for StreamPolicy {
    fn default() -> Self {
        Self {
            reconnect_grace_ms: 30_000,

            max_buffer_events: 1_000,

            cancel_on_disconnect: true,
        }
    }
}

impl StreamPolicy {
    pub fn console_chat() -> Self {
        Self::default()
    }

    pub fn cron_job() -> Self {
        Self {
            reconnect_grace_ms: 0,
            max_buffer_events: 0,
            cancel_on_disconnect: false,
        }
    }

    pub fn voice_session() -> Self {
        Self {
            reconnect_grace_ms: 2_000,
            max_buffer_events: 100,
            cancel_on_disconnect: true,
        }
    }
}
