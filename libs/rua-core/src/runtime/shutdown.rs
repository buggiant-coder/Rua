use serde::{Deserialize, Serialize};
use std::fmt;
use std::time::Duration;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum ShutdownPhase {
    StopAccepting,

    NotifyStreams,

    CancelNonCritical,

    WaitPersistence,

    StopSubsystems,

    FlushObservability,

    ReleasePowerResources,
}

impl fmt::Display for ShutdownPhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::StopAccepting => "stop_accepting",
            Self::NotifyStreams => "notify_streams",
            Self::CancelNonCritical => "cancel_non_critical",
            Self::WaitPersistence => "wait_persistence",
            Self::StopSubsystems => "stop_subsystems",
            Self::FlushObservability => "flush_observability",
            Self::ReleasePowerResources => "release_power_resources",
        };
        write!(f, "{s}")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShutdownConfig {
    pub total_timeout: Duration,

    pub persistence_grace: Duration,

    pub subsystem_timeout: Duration,

    pub flush_timeout: Duration,
}

impl Default for ShutdownConfig {
    fn default() -> Self {
        Self {
            total_timeout: Duration::from_secs(30),
            persistence_grace: Duration::from_secs(5),
            subsystem_timeout: Duration::from_secs(10),
            flush_timeout: Duration::from_secs(5),
        }
    }
}

pub struct ShutdownController {
    config: ShutdownConfig,
    current_phase: Option<ShutdownPhase>,
}

impl ShutdownController {
    pub fn new(config: ShutdownConfig) -> Self {
        Self {
            config,
            current_phase: None,
        }
    }

    pub fn config(&self) -> &ShutdownConfig {
        &self.config
    }

    pub fn current_phase(&self) -> Option<ShutdownPhase> {
        self.current_phase
    }

    pub fn is_shutting_down(&self) -> bool {
        self.current_phase.is_some()
    }

    pub fn advance(&mut self) -> Option<ShutdownPhase> {
        let next = match self.current_phase {
            None => Some(ShutdownPhase::StopAccepting),
            Some(ShutdownPhase::StopAccepting) => Some(ShutdownPhase::NotifyStreams),
            Some(ShutdownPhase::NotifyStreams) => Some(ShutdownPhase::CancelNonCritical),
            Some(ShutdownPhase::CancelNonCritical) => Some(ShutdownPhase::WaitPersistence),
            Some(ShutdownPhase::WaitPersistence) => Some(ShutdownPhase::StopSubsystems),
            Some(ShutdownPhase::StopSubsystems) => Some(ShutdownPhase::FlushObservability),
            Some(ShutdownPhase::FlushObservability) => Some(ShutdownPhase::ReleasePowerResources),
            Some(ShutdownPhase::ReleasePowerResources) => None,
        };
        self.current_phase = next;
        next
    }

    pub fn phase_timeout(&self) -> Duration {
        match self.current_phase {
            Some(ShutdownPhase::WaitPersistence) => self.config.persistence_grace,
            Some(ShutdownPhase::StopSubsystems) => self.config.subsystem_timeout,
            Some(ShutdownPhase::FlushObservability) => self.config.flush_timeout,

            _ => Duration::from_secs(5),
        }
    }
}
