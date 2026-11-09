use crate::projection::{
    ContextTargetBand, ProjectionEntryMode, ProjectionFitStatus, ProjectionFoldFrontier,
    ProjectionMessageGroupEstimate, ProjectionPlan, ProjectionPlanner, ProjectionPlanningInput,
};
use crate::store::CheckpointModelIdentity;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelSwitchPreflightInput {
    pub current_model: CheckpointModelIdentity,
    pub target_model: CheckpointModelIdentity,
    pub target_band: ContextTargetBand,
    pub current_projection_tokens: u64,
    pub checkpoint_tokens: u64,
    pub system_context_tokens: u64,
    pub pending_user_input_tokens: u64,
    pub message_groups: Vec<ProjectionMessageGroupEstimate>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelSwitchPreflightDecision {
    Allow {
        target_model: CheckpointModelIdentity,
        schedule_maintenance: bool,
        estimated_projection_tokens: u64,
        target_upper_tokens: u64,
        emergency_limit_tokens: u64,
    },
    ConfirmationRequired {
        target_model: CheckpointModelIdentity,
        estimated_projection_tokens: u64,
        target_upper_tokens: u64,
        emergency_limit_tokens: u64,
        recommended_fold_frontier: Option<ProjectionFoldFrontier>,
    },
}

pub struct ModelSwitchPreflight;

impl ModelSwitchPreflight {
    pub fn evaluate(input: ModelSwitchPreflightInput) -> ModelSwitchPreflightDecision {
        let planner = ProjectionPlanner::new(input.target_band);
        let plan = planner.plan(ProjectionPlanningInput {
            entry_mode: ProjectionEntryMode::ModelSwitchPreflight,
            current_projection_tokens: input.current_projection_tokens,
            checkpoint_tokens: input.checkpoint_tokens,
            system_context_tokens: input.system_context_tokens,
            pending_user_input_tokens: input.pending_user_input_tokens,
            message_groups: input.message_groups,
        });

        match plan.status {
            ProjectionFitStatus::FitsTarget => ModelSwitchPreflightDecision::Allow {
                target_model: input.target_model,
                schedule_maintenance: false,
                estimated_projection_tokens: input.current_projection_tokens,
                target_upper_tokens: input.target_band.upper_target_tokens,
                emergency_limit_tokens: input.target_band.emergency_limit_tokens,
            },
            ProjectionFitStatus::FitsEmergencyButNeedsMaintenance => {
                ModelSwitchPreflightDecision::Allow {
                    target_model: input.target_model,
                    schedule_maintenance: true,
                    estimated_projection_tokens: input.current_projection_tokens,
                    target_upper_tokens: input.target_band.upper_target_tokens,
                    emergency_limit_tokens: input.target_band.emergency_limit_tokens,
                }
            }
            ProjectionFitStatus::RequiresCheckpoint
            | ProjectionFitStatus::RequiresEmergencyFallback => {
                ModelSwitchPreflightDecision::ConfirmationRequired {
                    target_model: input.target_model,
                    estimated_projection_tokens: input.current_projection_tokens,
                    target_upper_tokens: input.target_band.upper_target_tokens,
                    emergency_limit_tokens: input.target_band.emergency_limit_tokens,
                    recommended_fold_frontier: plan.fold_frontier,
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelSwitchApplyError {
    ConfirmationRequired,
    MaintenanceFailed(String),
    ProjectionStillUnsafe { status: ProjectionFitStatus },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelSwitchMaintenanceProof {
    pub plan: ProjectionPlan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatModelSwitchState {
    default_model: CheckpointModelIdentity,
}

impl ChatModelSwitchState {
    pub fn new(default_model: CheckpointModelIdentity) -> Self {
        Self { default_model }
    }

    pub fn default_model(&self) -> &CheckpointModelIdentity {
        &self.default_model
    }

    pub fn apply_preflight_decision(
        &mut self,
        decision: ModelSwitchPreflightDecision,
    ) -> Result<CheckpointModelIdentity, ModelSwitchApplyError> {
        match decision {
            ModelSwitchPreflightDecision::Allow { target_model, .. } => {
                self.default_model = target_model;
                Ok(self.default_model.clone())
            }
            ModelSwitchPreflightDecision::ConfirmationRequired { .. } => {
                Err(ModelSwitchApplyError::ConfirmationRequired)
            }
        }
    }

    pub fn apply_confirmed_switch(
        &mut self,
        target_model: CheckpointModelIdentity,
        maintenance: Result<ModelSwitchMaintenanceProof, ModelSwitchApplyError>,
    ) -> Result<CheckpointModelIdentity, ModelSwitchApplyError> {
        let proof = maintenance?;
        if proof.plan.status != ProjectionFitStatus::FitsTarget {
            return Err(ModelSwitchApplyError::ProjectionStillUnsafe {
                status: proof.plan.status,
            });
        }

        self.default_model = target_model;
        Ok(self.default_model.clone())
    }
}
