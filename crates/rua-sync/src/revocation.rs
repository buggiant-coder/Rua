use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum RevocationState {
    #[default]
    Active,

    Suspected {
        consecutive_failures: u32,
    },

    Revoked,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RevocationDetector {
    state: RevocationState,

    cycle_total: u32,

    cycle_failures: u32,

    consecutive_failed_cycles: u32,
}

const REVOCATION_CYCLE_THRESHOLD: u32 = 2;

impl RevocationDetector {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn state(&self) -> &RevocationState {
        &self.state
    }

    pub fn is_revoked(&self) -> bool {
        matches!(self.state, RevocationState::Revoked)
    }

    pub fn record_decrypt(&mut self, success: bool) {
        self.cycle_total += 1;
        if !success {
            self.cycle_failures += 1;
        }
    }

    pub fn end_cycle(&mut self) {
        if self.cycle_total == 0 {
            return;
        }

        if self.cycle_failures == self.cycle_total {
            self.consecutive_failed_cycles += 1;
            if self.consecutive_failed_cycles >= REVOCATION_CYCLE_THRESHOLD {
                self.state = RevocationState::Revoked;
            } else {
                self.state = RevocationState::Suspected {
                    consecutive_failures: self.cycle_failures,
                };
            }
        } else {
            self.consecutive_failed_cycles = 0;
            self.state = RevocationState::Active;
        }

        self.cycle_total = 0;
        self.cycle_failures = 0;
    }

    pub fn mark_revoked(&mut self) {
        self.state = RevocationState::Revoked;
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }
}
