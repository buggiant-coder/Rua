use std::sync::Arc;

use async_trait::async_trait;

use rua_acp::permissions::{
    extract_tool_call_id, PermissionDecision, PermissionEvaluator, RequestPermissionRequest,
};
use rua_security::{
    evaluate_command, evaluate_tool_call, BashAnalyzer, GuardDecision, SecurityModule,
    ToolCallContext,
};

#[derive(Debug, Clone)]
pub struct AcpPermissionTaskContext {
    pub task_id: String,
    pub attempt_no: u32,
    pub runner: String,
    pub origin_turn_id: Option<String>,
    pub origin_tool_call_id: Option<String>,
    pub working_dir: String,
}

pub struct SecurityPermissionEvaluator {
    security: Option<Arc<SecurityModule>>,
    task_context: Option<AcpPermissionTaskContext>,
}

impl Default for SecurityPermissionEvaluator {
    fn default() -> Self {
        Self::new()
    }
}

impl SecurityPermissionEvaluator {
    pub fn new() -> Self {
        Self {
            security: None,
            task_context: None,
        }
    }

    pub fn with_security(security: Arc<SecurityModule>) -> Self {
        Self {
            security: Some(security),
            task_context: None,
        }
    }

    pub fn with_task_context(mut self, task_context: AcpPermissionTaskContext) -> Self {
        self.task_context = Some(task_context);
        self
    }
}

#[async_trait]
impl PermissionEvaluator for SecurityPermissionEvaluator {
    async fn evaluate(
        &self,
        agent_id: &str,
        session_id: &str,
        request: &RequestPermissionRequest,
    ) -> PermissionDecision {
        let tool_call_id = extract_tool_call_id(request);
        let tool_name = extract_tool_name(request);
        let command = extract_command(request);
        let paths = extract_paths(request, command.as_deref());

        if let Some(ref cmd) = command {
            let result = evaluate_command(cmd);
            if result.decision == GuardDecision::Deny {
                let reason = result
                    .findings
                    .first()
                    .map(|f| f.message.clone())
                    .unwrap_or_else(|| "blocked by security policy".to_string());
                return PermissionDecision::Denied { reason };
            }
        }

        let mut tool_input = build_tool_input(request, command.as_deref(), &paths);
        if let Some(task) = self.task_context.as_ref() {
            tool_input["_task_context"] = serde_json::json!({
                "task_id": task.task_id,
                "attempt_no": task.attempt_no,
                "runner": task.runner,
                "origin_turn_id": task.origin_turn_id,
                "origin_tool_call_id": task.origin_tool_call_id,
            });
        }
        let mut ctx = ToolCallContext::new(agent_id, session_id, &tool_name, tool_input.clone());
        ctx.channel = "task".to_string();
        ctx.metadata.insert(
            "tool_call_id".to_string(),
            serde_json::Value::String(tool_call_id.clone()),
        );
        if let Some(task) = self.task_context.as_ref() {
            ctx.working_dir = task.working_dir.clone();
            ctx.metadata.insert(
                "turn_id".to_string(),
                serde_json::Value::String(task.origin_turn_id.clone().unwrap_or_else(|| {
                    format!("task:{}:attempt:{}", task.task_id, task.attempt_no)
                })),
            );
            ctx.metadata.insert(
                "task_id".to_string(),
                serde_json::Value::String(task.task_id.clone()),
            );
            ctx.metadata.insert(
                "task_attempt_no".to_string(),
                serde_json::json!(task.attempt_no),
            );
            ctx.metadata.insert(
                "task_runner".to_string(),
                serde_json::Value::String(task.runner.clone()),
            );
        }
        if let Some(security) = self.security.as_ref() {
            if let Some(reason) = security.hard_deny_reason(&ctx).await {
                return PermissionDecision::Denied { reason };
            }
        }
        let guard_result = evaluate_tool_call(&ctx);

        if guard_result.decision == GuardDecision::Deny {
            let reason = guard_result
                .findings
                .first()
                .map(|f| f.message.clone())
                .unwrap_or_else(|| "blocked by security policy".to_string());
            return PermissionDecision::Denied { reason };
        }

        PermissionDecision::Relay
    }
}

fn extract_tool_name(request: &RequestPermissionRequest) -> String {
    request
        .tool_call
        .fields
        .title
        .clone()
        .unwrap_or_else(|| "unknown".to_string())
}

fn extract_command(request: &RequestPermissionRequest) -> Option<String> {
    request
        .tool_call
        .fields
        .raw_input
        .as_ref()
        .and_then(|input| input.get("command"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

fn extract_paths(request: &RequestPermissionRequest, command: Option<&str>) -> Vec<String> {
    let mut paths = Vec::new();

    if let Some(ref locations) = request.tool_call.fields.locations {
        for loc in locations {
            paths.push(loc.path.to_string_lossy().to_string());
        }
    }

    if let Some(ref input) = request.tool_call.fields.raw_input {
        if let Some(path) = input.get("path").and_then(|v| v.as_str()) {
            paths.push(path.to_string());
        }
        if let Some(file_path) = input.get("file_path").and_then(|v| v.as_str()) {
            paths.push(file_path.to_string());
        }
    }

    if let Some(cmd) = command {
        let analyzer = BashAnalyzer::new();
        let analysis = analyzer.analyze(cmd);
        for parsed_cmd in &analysis.commands {
            for path_arg in &parsed_cmd.path_args {
                paths.push(path_arg.clone());
            }
        }
    }

    paths
}

fn build_tool_input(
    request: &RequestPermissionRequest,
    command: Option<&str>,
    paths: &[String],
) -> serde_json::Value {
    let options: Vec<String> = request
        .options
        .iter()
        .map(|o| o.option_id.to_string())
        .collect();

    let mut input = serde_json::json!({
        "tool_call_id": request.tool_call.tool_call_id.to_string(),
        "options": options,
    });

    if let Some(cmd) = command {
        input["command"] = serde_json::Value::String(cmd.to_string());
    }
    if !paths.is_empty() {
        input["paths"] = serde_json::json!(paths);
    }

    input
}
