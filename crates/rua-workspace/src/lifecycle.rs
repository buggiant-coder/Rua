use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::WorkspaceError;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceLifecycleState {
    #[default]
    New,

    CoreReady,

    MetadataReady,

    RuntimeReady,

    Draining,

    Stopped,

    Failed,
}

impl WorkspaceLifecycleState {
    pub fn can_accept_requests(&self) -> bool {
        matches!(
            self,
            Self::CoreReady | Self::MetadataReady | Self::RuntimeReady
        )
    }

    pub fn is_healthy(&self) -> bool {
        !matches!(self, Self::Failed | Self::Stopped)
    }

    pub fn is_ready(&self) -> bool {
        matches!(self, Self::RuntimeReady)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InitPhase {
    Core,

    Metadata,

    Runtime,

    Background,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlatformProfile {
    #[default]
    Desktop,

    Mobile,
}

impl PlatformProfile {
    pub fn max_blocking_phase(&self) -> InitPhase {
        match self {
            Self::Desktop => InitPhase::Runtime,
            Self::Mobile => InitPhase::Core,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "policy", rename_all = "snake_case")]
pub enum ShutdownPolicy {
    Graceful {
        #[serde(with = "humantime_serde")]
        timeout: Duration,
    },

    CancelActive {
        #[serde(with = "humantime_serde")]
        timeout: Duration,
    },

    Immediate,
}

impl Default for ShutdownPolicy {
    fn default() -> Self {
        Self::Graceful {
            timeout: Duration::from_secs(30),
        }
    }
}

impl ShutdownPolicy {
    /// The single bound this policy gives the whole shutdown.
    ///
    /// `Immediate` is not "no bound": it must still finish quickly, so it uses a
    /// short fixed deadline rather than waiting on a drain that will never be
    /// attempted.
    pub fn deadline(&self) -> tokio::time::Instant {
        let timeout = match self {
            ShutdownPolicy::Graceful { timeout } | ShutdownPolicy::CancelActive { timeout } => {
                *timeout
            }
            ShutdownPolicy::Immediate => Duration::from_millis(250),
        };
        tokio::time::Instant::now() + timeout
    }
}

/// Which kind of runnable work a [`WorkUnit`] names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkKind {
    /// A Conversation Turn executed by the Agent.
    Turn,

    /// A Task Attempt owned by one Chat.
    Task,
}

/// One identifiable piece of work the owner holds.
///
/// Work is named, not counted: a count cannot be re-checked later, and a
/// shutdown that trusted a count could not tell "terminated" from "vanished".
/// The Chat Address is the real owner key, so a Project-Scoped Chat's work is
/// attributed through the Project rather than guessed from the Agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkUnit {
    pub attempt_no: Option<u32>,
    pub kind: WorkKind,

    pub chat: rua_core::ChatAddress,

    /// Turn ID for [`WorkKind::Turn`], Task ID for [`WorkKind::Task`].
    pub id: String,
}

/// Live work an Agent holds, observed from its owner.
///
/// A Workspace cannot enumerate Conversation turns or Task Attempts by itself:
/// their authority lives in the process composition layer, so these units come
/// from that authority and never from a mirrored counter.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentWork {
    #[serde(default)]
    pub units: Vec<WorkUnit>,

    /// Input the owner accepted but has not started. It is real pending work: a
    /// shutdown that ignored it would let a new Turn begin after the Agent was
    /// reported stopped.
    #[serde(default)]
    pub queued_inputs: usize,
}

impl AgentWork {
    pub fn is_empty(&self) -> bool {
        self.units.is_empty() && self.queued_inputs == 0
    }

    pub fn count(&self, kind: WorkKind) -> usize {
        self.units.iter().filter(|unit| unit.kind == kind).count()
    }
}

/// The owner seam for work a Workspace dispatches but does not own.
///
/// `rua-workspace` must not depend on `rua-server`, so the Conversation and
/// Task authority is reached through this trait. A composition root that leaves
/// the authority unset gets no confirmation and therefore no clean shutdown
/// report: there is deliberately no fallback that reports success.
#[async_trait]
pub trait AgentWorkAuthority: Send + Sync {
    async fn resume(&self, agent_id: &str) -> Result<(), WorkspaceError>;
    async fn begin_shutdown(&self, agent_id: &str) -> Result<(), WorkspaceError>;

    /// The work this Agent holds right now.
    async fn outstanding_work(&self, agent_id: &str) -> Result<AgentWork, WorkspaceError>;

    /// Asks the owner to end this Agent's work and reports what it still holds.
    ///
    /// The return value is a fresh observation, never a count of requests sent:
    /// a cancellation that was ignored must stay visible as still-held work.
    async fn cancel_work(&self, agent_id: &str) -> Result<AgentWork, WorkspaceError>;

    /// Which of `units` the owner now sees in a genuinely terminal state.
    ///
    /// A cancelled Turn or Attempt is not a terminated one. This reads the same
    /// authority that ends the work — a committed terminal fact for a Turn, a
    /// terminal Attempt state for a Task — so the shutdown report can only count
    /// terminations it can actually evidence.
    async fn terminated_units(&self, units: &[WorkUnit]) -> Result<Vec<WorkUnit>, WorkspaceError>;
}

/// What an Agent shutdown actually did.
///
/// Every field starts as "not observed" and is only raised by an observation:
/// work is counted as terminated only when its owner confirms a terminal state,
/// and anything still held, still running, or no longer observable is reported as
/// unresolved instead of being rounded up to a clean stop. Nothing here claims
/// durability: a `clean` report means the owned resources stopped and the work
/// owner confirmed termination, not that any state was flushed to disk.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ShutdownReport {
    /// Turns the owner confirmed terminal, and that ended without a cancellation
    /// request (a `Graceful` drain).
    pub turns_drained: usize,

    /// Turns the owner confirmed terminal *because* this shutdown cancelled them.
    pub turns_cancelled: usize,

    /// Turns this shutdown did not confirm terminal: still running, no longer
    /// observable, or a cancel request that never landed.
    pub turns_unresolved: usize,

    /// Tasks whose owner confirmed a terminal Attempt state.
    pub tasks_cancelled: usize,

    /// Queued inputs the owner discarded while blocking new submissions.
    pub queued_inputs_discarded: usize,

    /// Queued inputs the owner still held when the shutdown stopped observing.
    pub queued_inputs_unresolved: usize,

    /// Tasks the owner did not confirm terminal.
    pub tasks_unresolved: usize,

    /// Components whose stop reported a failure. Cleanup continues past them.
    pub failed_components: Vec<String>,

    /// Components whose stop exceeded its bound. They stay owned and a later
    /// shutdown retries them, so this is distinct from a reported failure.
    pub timed_out_components: Vec<String>,

    /// The work authority was reached and confirmed it holds nothing.
    pub authority_confirmed: bool,

    /// Every owned component was confirmed stopped. Independent of any time
    /// bound, and distinct from `finished_within_timeout`.
    pub cleanup_completed: bool,

    /// The whole shutdown finished inside the policy's deadline.
    pub finished_within_timeout: bool,
}

impl ShutdownReport {
    /// Records a component that did not stop cleanly.
    ///
    /// Failures accumulate as names because a caller needs to know *which*
    /// component leaked, not just that something did.
    pub fn fail(&mut self, component: impl Into<String>) {
        let component = component.into();
        if !self.failed_components.contains(&component) {
            self.failed_components.push(component);
        }
    }

    /// No failures, nothing unresolved, and the owner confirmed termination.
    ///
    /// A timed-out component is never part of a clean shutdown, even when the
    /// overall deadline was met for everything else.
    pub fn is_clean(&self) -> bool {
        self.cleanup_completed
            && self.finished_within_timeout
            && self.failed_components.is_empty()
            && self.timed_out_components.is_empty()
            && self.authority_confirmed
            && self.turns_unresolved == 0
            && self.tasks_unresolved == 0
            && self.queued_inputs_unresolved == 0
    }
}

mod humantime_serde {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::time::Duration;

    pub fn serialize<S>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u64(duration.as_secs())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Duration, D::Error>
    where
        D: Deserializer<'de>,
    {
        let secs = u64::deserialize(deserializer)?;
        Ok(Duration::from_secs(secs))
    }
}
