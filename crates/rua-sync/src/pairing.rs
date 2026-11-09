use std::time::{Duration, Instant};

use rand::Rng;
use serde::{Deserialize, Serialize};

use crate::SyncError;

const PIN_LENGTH: usize = 8;
const PIN_TTL: Duration = Duration::from_secs(5 * 60);

pub struct PairingPin {
    code: String,
    created_at: Instant,
    consumed: bool,
}

impl PairingPin {
    pub fn generate() -> Self {
        let mut rng = rand::thread_rng();
        let code: String = (0..PIN_LENGTH)
            .map(|_| rng.gen_range(0..10).to_string())
            .collect();
        Self {
            code,
            created_at: Instant::now(),
            consumed: false,
        }
    }

    pub fn code(&self) -> &str {
        &self.code
    }

    pub fn is_expired(&self) -> bool {
        self.created_at.elapsed() > PIN_TTL
    }

    pub fn is_consumed(&self) -> bool {
        self.consumed
    }

    pub fn consume(&mut self) -> Result<(), SyncError> {
        if self.consumed {
            return Err(SyncError::Pairing("PIN already consumed".into()));
        }
        if self.is_expired() {
            return Err(SyncError::Pairing("PIN expired".into()));
        }
        self.consumed = true;
        Ok(())
    }

    pub fn matches(&self, input: &str) -> bool {
        self.code == input
    }

    pub fn remaining_secs(&self) -> u64 {
        PIN_TTL
            .checked_sub(self.created_at.elapsed())
            .unwrap_or_default()
            .as_secs()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkState {
    pub has_network: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace_id: Option<[u8; 32]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at_ms: Option<u64>,
}
