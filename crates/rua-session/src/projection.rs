#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextTargetBand {
    pub upper_target_tokens: u64,
    pub emergency_limit_tokens: u64,
    pub tail_reserve_tokens: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ContextTargetBandOverride {
    pub upper_target_tokens: Option<u64>,
    pub emergency_limit_tokens: Option<u64>,
    pub tail_reserve_tokens: Option<u64>,
}

impl ContextTargetBand {
    pub fn resolve(defaults: Self, override_values: Option<ContextTargetBandOverride>) -> Self {
        let Some(override_values) = override_values else {
            return defaults;
        };

        let mut resolved = Self {
            upper_target_tokens: override_values
                .upper_target_tokens
                .unwrap_or(defaults.upper_target_tokens),
            emergency_limit_tokens: override_values
                .emergency_limit_tokens
                .unwrap_or(defaults.emergency_limit_tokens),
            tail_reserve_tokens: override_values
                .tail_reserve_tokens
                .unwrap_or(defaults.tail_reserve_tokens),
        };
        if resolved.upper_target_tokens > resolved.emergency_limit_tokens {
            resolved.upper_target_tokens = resolved.emergency_limit_tokens;
        }
        if resolved.tail_reserve_tokens > resolved.emergency_limit_tokens {
            resolved.tail_reserve_tokens = resolved.emergency_limit_tokens;
        }
        resolved
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ProjectionEntryMode {
    #[default]
    ForegroundTurn,
    PreModelCall,
    EarlyBackgroundCompaction,
    ForegroundContextMaintenance,
    ModelSwitchPreflight,
    CrashRecovery,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectionFitStatus {
    FitsTarget,
    FitsEmergencyButNeedsMaintenance,
    RequiresCheckpoint,
    RequiresEmergencyFallback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectionEntryAction {
    BuildProjection,
    BuildProjectionAndScheduleEarlyBackgroundCompaction,
    UseEmergencyBoundedProjection,
    ScheduleLowPriorityCheckpoint,
    ScheduleHighPriorityCheckpoint,
    SignalEmergencyFallbackRequired,
    CreateCheckpointSynchronously,
    ReportSafeCompactionFailed,
    AllowModelSwitch,
    AllowModelSwitchAndScheduleEarlyBackgroundCompaction,
    BlockAndAskToRunCheckpoint,
    UseOldCheckpointPlusBoundedTail,
    BuildProjectionAndMarkMaintenanceNeeded,
    BuildEmergencyBoundedProjection,
}

use crate::CheckpointRetainedSequenceRange;
use rua_config::AgentsRunningConfig;

pub fn context_target_band_from_running(
    running: &AgentsRunningConfig,
    max_input: u64,
    allow_early_compaction_target: bool,
) -> ContextTargetBand {
    let max_input = max_input.max(1);
    let blocking_ratio = running
        .context_compact
        .blocking_compact_ratio
        .clamp(0.05, 0.99);
    let target_ratio = if allow_early_compaction_target
        && running.context_compact.early_background_compaction_enabled
    {
        running
            .context_compact
            .early_compaction_target_ratio
            .clamp(0.05, blocking_ratio)
    } else {
        blocking_ratio
    };
    let reserve_ratio = running
        .context_compact
        .context_reserve_ratio
        .clamp(0.01, 0.5);
    let upper_target_tokens = ((max_input as f64) * target_ratio).max(1.0) as u64;
    ContextTargetBand {
        upper_target_tokens: upper_target_tokens.min(max_input),
        emergency_limit_tokens: max_input,
        tail_reserve_tokens: ((max_input as f64) * reserve_ratio).max(1.0) as u64,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectionMessageGroupEstimate {
    pub turn_id: String,
    pub turn_ordinal: u64,
    pub first_sequence: u64,
    pub last_sequence: u64,
    pub token_estimate: u64,
    pub foldable: bool,
    pub retained_sequence_ranges: Vec<CheckpointRetainedSequenceRange>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectionFoldFrontier {
    pub turn_id: String,
    pub start_turn_ordinal: u64,
    pub end_turn_ordinal: u64,
    pub start_sequence: u64,
    pub end_sequence: u64,
    pub estimated_tokens_to_fold: u64,
    pub retained_sequence_ranges: Vec<CheckpointRetainedSequenceRange>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectionEmergencyTail {
    pub start_turn_ordinal: u64,
    pub start_sequence: u64,
    pub estimated_tail_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectionPlanningInput {
    pub entry_mode: ProjectionEntryMode,
    pub current_projection_tokens: u64,
    pub checkpoint_tokens: u64,
    pub system_context_tokens: u64,
    pub pending_user_input_tokens: u64,
    pub message_groups: Vec<ProjectionMessageGroupEstimate>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectionPlan {
    pub status: ProjectionFitStatus,
    pub action: ProjectionEntryAction,
    pub maintenance_needed: bool,
    pub fold_frontier: Option<ProjectionFoldFrontier>,
    pub emergency_tail: Option<ProjectionEmergencyTail>,
}

#[derive(Debug, Clone, Copy)]
pub struct ProjectionPlanner {
    target_band: ContextTargetBand,
}

impl ProjectionPlanner {
    pub fn new(target_band: ContextTargetBand) -> Self {
        Self { target_band }
    }

    pub fn target_band(&self) -> ContextTargetBand {
        self.target_band
    }

    pub fn widest_fold_frontier(
        message_groups: &[ProjectionMessageGroupEstimate],
    ) -> Option<ProjectionFoldFrontier> {
        let foldable = message_groups
            .iter()
            .take_while(|group| group.foldable)
            .collect::<Vec<_>>();
        let first = foldable.first()?;
        let last = foldable.last()?;
        Some(ProjectionFoldFrontier {
            turn_id: last.turn_id.clone(),
            start_turn_ordinal: first.turn_ordinal,
            end_turn_ordinal: last.turn_ordinal,
            start_sequence: first.first_sequence,
            end_sequence: last.last_sequence,
            estimated_tokens_to_fold: foldable.iter().map(|group| group.token_estimate).sum(),
            retained_sequence_ranges: last.retained_sequence_ranges.clone(),
        })
    }

    pub fn plan(&self, input: ProjectionPlanningInput) -> ProjectionPlan {
        let fold_frontier = self.select_fold_frontier(&input);
        let emergency_tail = self.select_emergency_tail(&input);
        let emergency_budget = self.target_band.emergency_limit_tokens;

        let status = if input.current_projection_tokens <= self.target_band.upper_target_tokens {
            ProjectionFitStatus::FitsTarget
        } else if input.current_projection_tokens <= emergency_budget {
            ProjectionFitStatus::FitsEmergencyButNeedsMaintenance
        } else if fold_frontier.is_some() {
            ProjectionFitStatus::RequiresCheckpoint
        } else {
            ProjectionFitStatus::RequiresEmergencyFallback
        };

        ProjectionPlan {
            status,
            action: entry_action(input.entry_mode, status),
            maintenance_needed: !matches!(status, ProjectionFitStatus::FitsTarget),
            fold_frontier,
            emergency_tail,
        }
    }

    fn select_fold_frontier(
        &self,
        input: &ProjectionPlanningInput,
    ) -> Option<ProjectionFoldFrontier> {
        if input
            .system_context_tokens
            .saturating_add(input.pending_user_input_tokens)
            > self.target_band.emergency_limit_tokens
        {
            return None;
        }

        if input.message_groups.is_empty() {
            return None;
        }

        if input.current_projection_tokens <= self.target_band.upper_target_tokens {
            return None;
        }

        let mut remaining_tokens = input
            .message_groups
            .iter()
            .map(|group| group.token_estimate)
            .sum::<u64>();
        let mut split_index = 0_usize;
        for (index, group) in input.message_groups.iter().enumerate() {
            if !group.foldable {
                break;
            }
            if remaining_tokens <= self.target_band.tail_reserve_tokens {
                break;
            }
            remaining_tokens = remaining_tokens.saturating_sub(group.token_estimate);
            split_index = index + 1;
        }

        if split_index == 0 {
            return None;
        }

        let foldable = &input.message_groups[..split_index];
        let first = foldable.first()?;
        let last = foldable.last()?;
        Some(ProjectionFoldFrontier {
            turn_id: last.turn_id.clone(),
            start_turn_ordinal: first.turn_ordinal,
            end_turn_ordinal: last.turn_ordinal,
            start_sequence: first.first_sequence,
            end_sequence: last.last_sequence,
            estimated_tokens_to_fold: foldable.iter().map(|group| group.token_estimate).sum(),
            retained_sequence_ranges: last.retained_sequence_ranges.clone(),
        })
    }

    fn select_emergency_tail(
        &self,
        input: &ProjectionPlanningInput,
    ) -> Option<ProjectionEmergencyTail> {
        if input.current_projection_tokens <= self.target_band.upper_target_tokens {
            return None;
        }

        // Blocking compaction can fail before the hard limit. Keep its reserved
        // recent suffix available instead of falling back to one raw message.
        let tail_limit =
            if input.current_projection_tokens <= self.target_band.emergency_limit_tokens {
                self.target_band
                    .tail_reserve_tokens
                    .saturating_add(input.system_context_tokens)
                    .saturating_add(input.pending_user_input_tokens)
                    .min(self.target_band.emergency_limit_tokens)
            } else {
                self.target_band.emergency_limit_tokens
            };
        let mut tail_tokens = input
            .system_context_tokens
            .saturating_add(input.pending_user_input_tokens);
        let mut start = None;
        for group in input.message_groups.iter().rev() {
            if tail_tokens.saturating_add(group.token_estimate) > tail_limit {
                break;
            }
            tail_tokens = tail_tokens.saturating_add(group.token_estimate);
            start = Some(group);
        }
        let start = start?;

        Some(ProjectionEmergencyTail {
            start_turn_ordinal: start.turn_ordinal,
            start_sequence: start.first_sequence,
            estimated_tail_tokens: tail_tokens,
        })
    }
}

fn entry_action(mode: ProjectionEntryMode, status: ProjectionFitStatus) -> ProjectionEntryAction {
    match mode {
        ProjectionEntryMode::PreModelCall => match status {
            ProjectionFitStatus::FitsTarget => ProjectionEntryAction::BuildProjection,
            ProjectionFitStatus::FitsEmergencyButNeedsMaintenance
            | ProjectionFitStatus::RequiresCheckpoint => {
                ProjectionEntryAction::CreateCheckpointSynchronously
            }
            ProjectionFitStatus::RequiresEmergencyFallback => {
                ProjectionEntryAction::UseEmergencyBoundedProjection
            }
        },
        ProjectionEntryMode::ForegroundTurn => match status {
            ProjectionFitStatus::FitsTarget => ProjectionEntryAction::BuildProjection,
            ProjectionFitStatus::FitsEmergencyButNeedsMaintenance => {
                ProjectionEntryAction::BuildProjectionAndScheduleEarlyBackgroundCompaction
            }
            ProjectionFitStatus::RequiresCheckpoint
            | ProjectionFitStatus::RequiresEmergencyFallback => {
                ProjectionEntryAction::UseEmergencyBoundedProjection
            }
        },
        ProjectionEntryMode::EarlyBackgroundCompaction => match status {
            ProjectionFitStatus::FitsTarget => ProjectionEntryAction::BuildProjection,
            ProjectionFitStatus::FitsEmergencyButNeedsMaintenance => {
                ProjectionEntryAction::ScheduleLowPriorityCheckpoint
            }
            ProjectionFitStatus::RequiresCheckpoint => {
                ProjectionEntryAction::ScheduleHighPriorityCheckpoint
            }
            ProjectionFitStatus::RequiresEmergencyFallback => {
                ProjectionEntryAction::SignalEmergencyFallbackRequired
            }
        },
        ProjectionEntryMode::ForegroundContextMaintenance => match status {
            ProjectionFitStatus::RequiresEmergencyFallback => {
                ProjectionEntryAction::ReportSafeCompactionFailed
            }
            ProjectionFitStatus::FitsTarget
            | ProjectionFitStatus::FitsEmergencyButNeedsMaintenance
            | ProjectionFitStatus::RequiresCheckpoint => {
                ProjectionEntryAction::CreateCheckpointSynchronously
            }
        },
        ProjectionEntryMode::ModelSwitchPreflight => match status {
            ProjectionFitStatus::FitsTarget => ProjectionEntryAction::AllowModelSwitch,
            ProjectionFitStatus::FitsEmergencyButNeedsMaintenance => {
                ProjectionEntryAction::AllowModelSwitchAndScheduleEarlyBackgroundCompaction
            }
            ProjectionFitStatus::RequiresCheckpoint
            | ProjectionFitStatus::RequiresEmergencyFallback => {
                ProjectionEntryAction::BlockAndAskToRunCheckpoint
            }
        },
        ProjectionEntryMode::CrashRecovery => match status {
            ProjectionFitStatus::FitsTarget => ProjectionEntryAction::BuildProjection,
            ProjectionFitStatus::FitsEmergencyButNeedsMaintenance => {
                ProjectionEntryAction::BuildProjectionAndMarkMaintenanceNeeded
            }
            ProjectionFitStatus::RequiresCheckpoint => {
                ProjectionEntryAction::UseOldCheckpointPlusBoundedTail
            }
            ProjectionFitStatus::RequiresEmergencyFallback => {
                ProjectionEntryAction::BuildEmergencyBoundedProjection
            }
        },
    }
}
