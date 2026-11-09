use serde_json::Value;

use crate::diagnostics::{RepairDiagnostic, RepairStrategyKind};

#[derive(Debug, Clone, PartialEq)]
pub struct RepairOutcome {
    pub value: Value,

    pub repaired_json: String,

    pub changed: bool,

    pub strategies_applied: Vec<RepairStrategyKind>,

    pub confidence: RepairConfidence,

    pub diagnostics: Vec<RepairDiagnostic>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RepairStringOutcome {
    pub value: String,

    pub repaired_json: String,

    pub changed: bool,

    pub strategies_applied: Vec<RepairStrategyKind>,

    pub confidence: RepairConfidence,

    pub diagnostics: Vec<RepairDiagnostic>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RepairConfidence {
    Low = 0,

    Medium = 1,

    High = 2,

    Exact = 3,
}

impl std::fmt::Display for RepairConfidence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Exact => write!(f, "exact"),
            Self::High => write!(f, "high"),
            Self::Medium => write!(f, "medium"),
            Self::Low => write!(f, "low"),
        }
    }
}
