use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RunControl {
    Running,

    Stop,

    Interject,

    InterjectAfterTool,

    InterjectAtBoundary,

    ContinueAtBoundary,
}

impl RunControl {
    pub fn is_terminal(self) -> bool {
        !matches!(self, Self::Running)
    }

    pub fn is_stop(self) -> bool {
        matches!(self, Self::Stop)
    }

    pub fn is_interject(self) -> bool {
        matches!(
            self,
            Self::Interject | Self::InterjectAtBoundary | Self::InterjectAfterTool
        )
    }

    pub fn should_exit_at_boundary(self) -> bool {
        matches!(
            self,
            Self::InterjectAtBoundary | Self::ContinueAtBoundary | Self::InterjectAfterTool
        )
    }

    pub fn is_immediate_interject(self) -> bool {
        matches!(self, Self::Interject)
    }
}
