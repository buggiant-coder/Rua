use chrono::{DateTime, Utc};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MisfireDecision {
    RunNow,

    Skip,
}

impl fmt::Display for MisfireDecision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RunNow => write!(f, "run_now"),
            Self::Skip => write!(f, "skip"),
        }
    }
}

pub fn decide_misfire(
    scheduled: DateTime<Utc>,
    now: DateTime<Utc>,
    grace_seconds: u32,
    policy: &str,
) -> MisfireDecision {
    if policy == "skip" {
        return MisfireDecision::Skip;
    }

    let elapsed = now.signed_duration_since(scheduled);
    if elapsed.num_seconds() <= i64::from(grace_seconds) {
        MisfireDecision::RunNow
    } else {
        MisfireDecision::Skip
    }
}
