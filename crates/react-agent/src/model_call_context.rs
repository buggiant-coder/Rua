use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

use agent_types::{
    ChatRequest, ContentPart, ModelDialogueItem, Role, StructuredOutput, ToolDefinition,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::context::AgentContext;
use crate::error::AgentError;

pub const MODEL_CALL_CONTEXT_METADATA_KEY: &str = "rua_model_call_context";
pub const MODEL_CALL_CONTEXT_SECTION_METADATA_KEY: &str = "rua_model_call_context_section";
pub const MODEL_CALL_ID_METADATA_KEY: &str = "rua_model_call_id";
pub const MODEL_INPUT_TOKEN_ESTIMATE_METADATA_KEY: &str = "rua_model_input_token_estimate";
const PROMPT_INSPECTION_PATH_ENV: &str = "RUA_PROMPT_INSPECTION_PATH";
const PROMPT_INSPECTION_INCLUDE_CONTENT_ENV: &str = "RUA_PROMPT_INSPECTION_INCLUDE_CONTENT";

#[async_trait]
pub trait ModelCallProjector: Send + Sync {
    async fn project(
        &self,
        request: ChatRequest,
        event_tx: &tokio::sync::mpsc::Sender<agent_types::AgentEvent>,
    ) -> Result<ChatRequest, AgentError>;

    async fn recover_context_overflow(
        &self,
        _messages: &[ModelDialogueItem],
        error_message: &str,
    ) -> Result<Vec<ModelDialogueItem>, AgentError> {
        Err(AgentError::ContextOverflow {
            message: error_message.to_string(),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelCallSectionKind {
    AgentPromptFiles,
    AgentCustomPrompt,
    WorkspaceContext,
    AgentPromptContext,
    ProjectInstructions,
    BootstrapContext,
    CollaborationMode,
    ExecutionEnvironment,
    SkillCatalog,
    RuntimeGuidance,
}

impl ModelCallSectionKind {
    pub const fn metadata_name(self) -> &'static str {
        match self {
            Self::AgentPromptFiles => "agent_prompt_files",
            Self::AgentCustomPrompt => "agent_custom_prompt",
            Self::WorkspaceContext => "workspace_context",
            Self::AgentPromptContext => "agent_prompt_context",
            Self::ProjectInstructions => "project_instructions",
            Self::BootstrapContext => "bootstrap_context",
            Self::CollaborationMode => "collaboration_mode",
            Self::ExecutionEnvironment => "execution_environment",
            Self::SkillCatalog => "skill_catalog",
            Self::RuntimeGuidance => "runtime_guidance",
        }
    }

    const fn role(self) -> Role {
        Role::Developer
    }

    fn from_metadata_name(name: &str) -> Option<Self> {
        [
            Self::AgentPromptFiles,
            Self::AgentCustomPrompt,
            Self::WorkspaceContext,
            Self::AgentPromptContext,
            Self::ProjectInstructions,
            Self::BootstrapContext,
            Self::CollaborationMode,
            Self::ExecutionEnvironment,
            Self::SkillCatalog,
            Self::RuntimeGuidance,
        ]
        .into_iter()
        .find(|kind| kind.metadata_name() == name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelCallSection {
    pub kind: ModelCallSectionKind,
    pub content: String,
}

impl ModelCallSection {
    pub fn new(kind: ModelCallSectionKind, content: impl Into<String>) -> Self {
        Self {
            kind,
            content: content.into(),
        }
    }

    fn into_message(self) -> ModelDialogueItem {
        let mut message = ModelDialogueItem::text(self.kind.role(), self.content);
        message.metadata.insert(
            MODEL_CALL_CONTEXT_SECTION_METADATA_KEY.to_string(),
            serde_json::Value::String(self.kind.metadata_name().to_string()),
        );
        message
    }
}

pub fn replace_model_call_section(
    messages: &mut Vec<ModelDialogueItem>,
    section: Option<ModelCallSection>,
    kind: ModelCallSectionKind,
) {
    messages.retain(|message| {
        message
            .metadata
            .get(MODEL_CALL_CONTEXT_SECTION_METADATA_KEY)
            .and_then(serde_json::Value::as_str)
            != Some(kind.metadata_name())
    });
    let Some(section) = section else {
        return;
    };
    let message = section.into_message();
    let insertion_index = messages
        .iter()
        .position(|existing| {
            existing
                .metadata
                .get(MODEL_CALL_CONTEXT_SECTION_METADATA_KEY)
                .and_then(serde_json::Value::as_str)
                .and_then(ModelCallSectionKind::from_metadata_name)
                .is_some_and(|existing_kind| existing_kind > kind)
        })
        .unwrap_or_else(|| {
            messages
                .iter()
                .take_while(|message| {
                    message
                        .metadata
                        .contains_key(MODEL_CALL_CONTEXT_SECTION_METADATA_KEY)
                })
                .count()
        });
    messages.insert(insertion_index, message);
}

#[derive(Debug, Clone)]
pub struct ModelCallContext {
    request: ChatRequest,
    model_call_id: String,
    iteration: u32,
    context_window_tokens: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelInputTokenEstimateMethod {
    Fast,
    ProviderDelta,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelInputTokenEstimate {
    pub provider_id: Option<String>,
    pub model: Option<String>,
    pub fast_estimate_divisor: f64,
    pub fast_estimate_tokens: u64,
    pub calibrated_estimate_tokens: u64,
    pub conversation_estimate_tokens: u64,
    pub method: ModelInputTokenEstimateMethod,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calibration_source_provider_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calibration_source_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_fast_estimate_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_provider_prompt_tokens: Option<u64>,
}

#[derive(Serialize)]
struct ModelVisibleInput<'a> {
    messages: Vec<ModelVisibleMessage<'a>>,
    tools: &'a [ToolDefinition],
    structured_output: &'a Option<StructuredOutput>,
}

#[derive(Serialize)]
struct ModelVisibleMessage<'a> {
    role: &'a Role,
    content: &'a [ContentPart],
    name: &'a Option<String>,
    tool_call_id: Option<&'a str>,
}

#[derive(Debug, Default, Serialize)]
struct ModelCallSectionDiagnostic {
    instances: u64,
    estimated_tokens: u64,
}

#[derive(Debug, Serialize)]
struct ModelCallMessageDiagnostic {
    index: usize,
    role: &'static str,
    section: Option<String>,
    estimated_tokens: u64,
}

#[derive(Debug, Serialize)]
struct PromptInspectionMessage {
    index: usize,
    role: &'static str,
    section: Option<String>,
    estimated_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct PromptInspection {
    model: Option<String>,
    input_estimate_tokens: u64,
    tool_count: usize,
    tools: Vec<String>,
    messages: Vec<PromptInspectionMessage>,
}

fn model_call_section_diagnostics(
    messages: &[ModelDialogueItem],
) -> BTreeMap<String, ModelCallSectionDiagnostic> {
    let mut diagnostics = BTreeMap::new();
    for message in messages {
        let Some(section) = message
            .metadata
            .get(MODEL_CALL_CONTEXT_SECTION_METADATA_KEY)
            .and_then(serde_json::Value::as_str)
        else {
            continue;
        };
        let diagnostic = diagnostics
            .entry(section.to_string())
            .or_insert_with(ModelCallSectionDiagnostic::default);
        diagnostic.instances = diagnostic.instances.saturating_add(1);
        diagnostic.estimated_tokens = diagnostic
            .estimated_tokens
            .saturating_add(estimate_model_visible_message_tokens(message));
    }
    diagnostics
}

fn model_visible_message(message: &ModelDialogueItem) -> ModelVisibleMessage<'_> {
    ModelVisibleMessage {
        role: &message.role,
        content: &message.content,
        name: &message.name,
        tool_call_id: message
            .metadata
            .get("tool_call_id")
            .and_then(serde_json::Value::as_str),
    }
}

fn estimate_model_visible_message_tokens(message: &ModelDialogueItem) -> u64 {
    rua_core::token_estimate::estimate_serialized_tokens(
        &model_visible_message(message),
        rua_core::tool::DEFAULT_TOKEN_ESTIMATE_DIVISOR,
    ) as u64
}

pub fn estimate_model_visible_input_tokens_with_messages(
    request: &ChatRequest,
    messages: &[ModelDialogueItem],
    divisor: f64,
) -> u64 {
    rua_core::token_estimate::estimate_serialized_tokens(
        &ModelVisibleInput {
            messages: messages.iter().map(model_visible_message).collect(),
            tools: &request.tools,
            structured_output: &request.structured_output,
        },
        divisor,
    )
}

pub fn estimate_model_visible_conversation_tokens(
    messages: &[ModelDialogueItem],
    divisor: f64,
) -> u64 {
    let messages = messages
        .iter()
        .filter(|message| {
            !message
                .metadata
                .contains_key(MODEL_CALL_CONTEXT_SECTION_METADATA_KEY)
        })
        .map(model_visible_message)
        .collect::<Vec<_>>();
    rua_core::token_estimate::estimate_serialized_tokens(&messages, divisor)
}

fn role_name(role: &Role) -> &'static str {
    match role {
        Role::System => "system",
        Role::Developer => "developer",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
        _ => "unknown",
    }
}

fn message_diagnostics(messages: &[ModelDialogueItem]) -> Vec<ModelCallMessageDiagnostic> {
    messages
        .iter()
        .enumerate()
        .map(|(index, message)| ModelCallMessageDiagnostic {
            index,
            role: role_name(&message.role),
            section: message
                .metadata
                .get(MODEL_CALL_CONTEXT_SECTION_METADATA_KEY)
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
            estimated_tokens: estimate_model_visible_message_tokens(message),
        })
        .collect()
}

fn maybe_write_prompt_inspection(request: &ChatRequest, input_estimate_tokens: u64) {
    let Some(path) = std::env::var_os(PROMPT_INSPECTION_PATH_ENV).map(std::path::PathBuf::from)
    else {
        return;
    };
    let include_content = std::env::var(PROMPT_INSPECTION_INCLUDE_CONTENT_ENV)
        .is_ok_and(|value| matches!(value.as_str(), "1" | "true" | "yes"));
    let inspection = PromptInspection {
        model: request.model.clone(),
        input_estimate_tokens,
        tool_count: request.tools.len(),
        tools: request.tools.iter().map(|tool| tool.name.clone()).collect(),
        messages: request
            .messages
            .iter()
            .enumerate()
            .map(|(index, message)| PromptInspectionMessage {
                index,
                role: role_name(&message.role),
                section: message
                    .metadata
                    .get(MODEL_CALL_CONTEXT_SECTION_METADATA_KEY)
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string),
                estimated_tokens: estimate_model_visible_message_tokens(message),
                content: include_content
                    .then(|| serde_json::to_value(&message.content).unwrap_or_default()),
            })
            .collect(),
    };
    if let Err(error) = write_atomic_json(&path, &inspection) {
        tracing::warn!(
            path = %path.display(),
            error = %error,
            "failed to write prompt inspection"
        );
    }
}

fn write_atomic_json(path: &Path, value: &impl Serialize) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("prompt-inspection.json");
    let temp_path = parent.join(format!(".{file_name}.{}.tmp", uuid::Uuid::new_v4()));
    let write_result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)?;
        serde_json::to_writer_pretty(&mut file, value).map_err(std::io::Error::other)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        std::fs::rename(&temp_path, path)?;
        std::fs::File::open(parent)?.sync_all()
    })();
    if write_result.is_err() {
        let _ = std::fs::remove_file(&temp_path);
    }
    write_result
}

impl ModelCallContext {
    pub fn capture(
        ctx: &AgentContext,
        messages: Vec<ModelDialogueItem>,
        iteration: u32,
        include_tools: bool,
    ) -> Self {
        Self::capture_with_guidance(ctx, messages, iteration, include_tools, None)
    }

    pub fn capture_with_guidance(
        ctx: &AgentContext,
        mut messages: Vec<ModelDialogueItem>,
        iteration: u32,
        include_tools: bool,
        guidance: Option<&str>,
    ) -> Self {
        messages.retain(|message| {
            !message
                .metadata
                .contains_key(MODEL_CALL_CONTEXT_SECTION_METADATA_KEY)
        });
        let mut current_messages = Vec::new();
        let mut sections = BTreeMap::new();
        for section in ctx.model_call_sections.iter().cloned() {
            if !section.content.trim().is_empty() {
                sections.insert(section.kind, section);
            }
        }
        if let Some(guidance) = guidance.filter(|value| !value.trim().is_empty()) {
            sections.insert(
                ModelCallSectionKind::RuntimeGuidance,
                ModelCallSection::new(ModelCallSectionKind::RuntimeGuidance, guidance),
            );
        }
        current_messages.extend(sections.into_values().map(ModelCallSection::into_message));
        messages.splice(0..0, current_messages);

        let mut request = ChatRequest::streaming(messages);
        if include_tools {
            request.tools = ctx.tool_definitions();
        } else {
            request.tool_choice = Some(agent_types::ToolChoice::None);
        }
        request.model.clone_from(&ctx.model_name);
        request.provider_options = ctx.config.provider_options.clone();
        request.reasoning.clone_from(&ctx.config.reasoning);
        request.metadata = ctx.llm_request_metadata();

        let model_info = ctx.model.model_info();
        Self {
            request,
            model_call_id: uuid::Uuid::new_v4().to_string(),
            iteration,
            context_window_tokens: model_info.context_window,
        }
    }

    pub(crate) fn capture_trace(
        &self,
        stage: &'static str,
    ) -> llm_multiplex::debug_capture::CaptureTrace {
        let mut metadata = self.request.metadata.clone();
        metadata.insert(
            MODEL_CALL_ID_METADATA_KEY.into(),
            self.model_call_id.clone().into(),
        );
        let mut trace = llm_multiplex::debug_capture::CaptureTrace::new(&metadata, stage);
        trace.set("iteration", self.iteration);
        if let Some(turn_id) = metadata.get("turn_id") {
            trace.set("turn_id", turn_id.clone());
        }
        trace
    }

    pub(crate) fn model_call_id(&self) -> &str {
        &self.model_call_id
    }

    pub async fn project(
        self,
        ctx: &AgentContext,
        event_tx: &tokio::sync::mpsc::Sender<agent_types::AgentEvent>,
    ) -> Result<ChatRequest, AgentError> {
        let mut trace = self.capture_trace("preparation");
        let hook_result = trace
            .measure_local(
                "before_model_hook_started",
                "before_model_hook_finished",
                ctx.hook
                    .before_model_call(&self.request.messages, self.iteration, event_tx),
            )
            .await;
        if hook_result.is_err() {
            trace.finish("error");
        }
        hook_result?;
        let projected = match ctx.model_call_projector.as_ref() {
            Some(projector) => projector.project(self.request, event_tx).await,
            None => Ok(self.request),
        };
        if projected.is_err() {
            trace.finish("error");
        }
        let mut request = projected?;
        trace.mark("projection_finished");
        let token_estimate = request
            .metadata
            .get(MODEL_INPUT_TOKEN_ESTIMATE_METADATA_KEY)
            .cloned()
            .and_then(|value| serde_json::from_value::<ModelInputTokenEstimate>(value).ok())
            .unwrap_or_else(|| {
                let fast_estimate_tokens = estimate_model_visible_input_tokens_with_messages(
                    &request,
                    &request.messages,
                    rua_core::tool::DEFAULT_TOKEN_ESTIMATE_DIVISOR,
                );
                ModelInputTokenEstimate {
                    provider_id: ctx.provider_id.clone(),
                    model: request.model.clone(),
                    fast_estimate_divisor: rua_core::tool::DEFAULT_TOKEN_ESTIMATE_DIVISOR,
                    fast_estimate_tokens,
                    calibrated_estimate_tokens: fast_estimate_tokens,
                    conversation_estimate_tokens: estimate_model_visible_conversation_tokens(
                        &request.messages,
                        rua_core::tool::DEFAULT_TOKEN_ESTIMATE_DIVISOR,
                    ),
                    method: ModelInputTokenEstimateMethod::Fast,
                    calibration_source_provider_id: None,
                    calibration_source_model: None,
                    previous_fast_estimate_tokens: None,
                    previous_provider_prompt_tokens: None,
                }
            });
        let input_estimate_tokens = token_estimate.fast_estimate_tokens;
        request.metadata.insert(
            MODEL_INPUT_TOKEN_ESTIMATE_METADATA_KEY.to_string(),
            serde_json::to_value(&token_estimate).unwrap_or_default(),
        );
        trace.mark("token_estimate_finished");
        let sections = model_call_section_diagnostics(&request.messages);
        let messages = message_diagnostics(&request.messages);
        let tool_names = request
            .tools
            .iter()
            .map(|tool| tool.name.clone())
            .collect::<Vec<_>>();
        let sections_json = serde_json::to_string(&sections).unwrap_or_else(|_| "{}".to_string());
        tracing::debug!(
            target: "diag.harness",
            model_call_id = %self.model_call_id,
            iteration = self.iteration,
            input_estimate_tokens,
            context_window_tokens = ?self.context_window_tokens,
            tools = request.tools.len(),
            sections = %sections_json,
            "model call context projected"
        );
        request.metadata.insert(
            MODEL_CALL_CONTEXT_METADATA_KEY.to_string(),
            serde_json::json!({
                "iteration": self.iteration,
                "input_estimate_tokens": input_estimate_tokens,
                "calibrated_input_tokens": token_estimate.calibrated_estimate_tokens,
                "input_estimate_method": token_estimate.method,
                "context_window_tokens": self.context_window_tokens,
                "tools": request.tools.len(),
                "tool_names": tool_names,
                "sections": sections,
                "messages": messages,
            }),
        );
        request.metadata.insert(
            MODEL_CALL_ID_METADATA_KEY.to_string(),
            serde_json::Value::String(self.model_call_id),
        );
        trace.mark("diagnostics_finished");
        maybe_write_prompt_inspection(&request, input_estimate_tokens);
        trace.mark("prompt_inspection_finished");
        trace.finish("completed");
        Ok(request)
    }
}
