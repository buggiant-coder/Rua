use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use rua_contract::Tool;
use rua_core::tool::{ToolContext, ToolResult, ToolSchema};
use rua_session::{ActiveWorkPlanContent, ActiveWorkPlanStep, ActiveWorkPlanStepStatus};

use crate::annotations::{ApprovalPolicy, ExtendedToolAnnotations};
use crate::schema::ToolSpec;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum StepStatus {
    Pending,
    InProgress,
    Completed,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct PlanStep {
    step: String,
    status: StepStatus,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct UpdatePlanArgs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    explanation: Option<String>,
    plan: Vec<PlanStep>,
}

pub struct UpdatePlanTool;

impl UpdatePlanTool {
    pub fn new() -> Self {
        Self
    }

    pub fn spec() -> ToolSpec {
        ToolSpec {
            name: "update_plan".to_string(),
            description: "Update the current Turn's structured work checklist. Submit the complete checklist on every call. Use for non-trivial multi-step work; this does not change Collaboration Mode.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "explanation": {
                        "type": "string",
                        "description": "Optional explanation for this checklist update"
                    },
                    "plan": {
                        "type": "array",
                        "description": "Complete ordered checklist snapshot",
                        "items": {
                            "type": "object",
                            "properties": {
                                "step": {
                                    "type": "string",
                                    "description": "Task step text"
                                },
                                "status": {
                                    "type": "string",
                                    "enum": ["pending", "in_progress", "completed"]
                                }
                            },
                            "required": ["step", "status"],
                            "additionalProperties": false
                        }
                    }
                },
                "required": ["plan"],
                "additionalProperties": false
            }),
            annotations: ExtendedToolAnnotations {
                read_only: false,
                idempotent: true,
                requires_approval: ApprovalPolicy::Never,
                allowed_on_mobile: true,
                ..Default::default()
            },
            namespace: "builtin".into(),
        }
    }
}

impl Default for UpdatePlanTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for UpdatePlanTool {
    fn name(&self) -> &str {
        "update_plan"
    }

    fn schema(&self) -> ToolSchema {
        let spec = Self::spec();
        ToolSchema {
            name: spec.name,
            description: spec.description,
            parameters: spec.parameters,
        }
    }

    async fn call(
        &self,
        args: serde_json::Value,
        ctx: ToolContext,
    ) -> rua_core::Result<ToolResult> {
        let args: UpdatePlanArgs =
            serde_json::from_value(args).map_err(|error| rua_core::RuaError::Tool {
                tool: self.name().to_string(),
                message: error.to_string(),
            })?;
        let in_progress = args
            .plan
            .iter()
            .filter(|step| matches!(step.status, StepStatus::InProgress))
            .count();
        if in_progress > 1 {
            return Err(rua_core::RuaError::Tool {
                tool: self.name().to_string(),
                message: "at most one plan step may be in_progress".to_string(),
            });
        }
        if args.plan.iter().any(|step| step.step.trim().is_empty()) {
            return Err(rua_core::RuaError::Tool {
                tool: self.name().to_string(),
                message: "plan step text must not be empty".to_string(),
            });
        }

        if let Some(conversation_store_dir) = ctx.conversation_store_dir.as_ref() {
            let store = rua_session::WorkspaceSessionStore::new(conversation_store_dir);
            let content = ActiveWorkPlanContent {
                explanation: args.explanation.clone(),
                plan: args
                    .plan
                    .iter()
                    .map(|step| ActiveWorkPlanStep {
                        step: step.step.clone(),
                        status: match step.status {
                            StepStatus::Pending => ActiveWorkPlanStepStatus::Pending,
                            StepStatus::InProgress => ActiveWorkPlanStepStatus::InProgress,
                            StepStatus::Completed => ActiveWorkPlanStepStatus::Completed,
                        },
                    })
                    .collect(),
            };
            let source_turn_id = ctx
                .metadata
                .get("turn_id")
                .and_then(serde_json::Value::as_str);
            let source_sequence = store
                .active_message_log_end_sequence_for_chat(&ctx.session_id)
                .await?;
            store
                .save_active_work_plan(&ctx.session_id, source_turn_id, source_sequence, content)
                .await?;
        }

        let snapshot = serde_json::to_value(&args).map_err(|error| rua_core::RuaError::Tool {
            tool: self.name().to_string(),
            message: error.to_string(),
        })?;
        let mut result = ToolResult::text(self.name(), "Plan updated");
        result
            .metadata
            .insert("work_checklist".to_string(), snapshot);
        Ok(result)
    }
}
