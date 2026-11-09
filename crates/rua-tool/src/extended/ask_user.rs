use std::collections::HashMap;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use rua_contract::Tool;
use rua_core::event::AskQuestion;
use rua_core::tool::{ToolContext, ToolResult, ToolSchema};

use crate::annotations::{ApprovalPolicy, ExtendedToolAnnotations};
use crate::error::ToolError;
use crate::schema::ToolSpec;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AskMode {
    #[default]
    Text,

    SingleSelect,

    MultiSelect,
}

#[async_trait]
pub trait UserInteractionHandler: Send + Sync {
    async fn ask(
        &self,
        session_id: &str,
        channel: &str,
        question: &str,
        mode: &AskMode,
        options: &[String],
        timeout_ms: u64,
    ) -> Result<AskResponse, ToolError>;

    fn is_interactive(&self, channel: &str) -> bool;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AskResponse {
    pub text: Option<String>,

    pub selected: Vec<usize>,

    pub timed_out: bool,
}

pub struct NonInteractiveHandler;

#[async_trait]
impl UserInteractionHandler for NonInteractiveHandler {
    async fn ask(
        &self,
        _session_id: &str,
        _channel: &str,
        _question: &str,
        _mode: &AskMode,
        _options: &[String],
        _timeout_ms: u64,
    ) -> Result<AskResponse, ToolError> {
        Err(ToolError::InteractionUnavailable {
            reason: "No interactive channel available. The current context \
                     (background task or cron job) does not support user interaction."
                .to_string(),
        })
    }

    fn is_interactive(&self, _channel: &str) -> bool {
        false
    }
}

const GC_MAX_AGE_SECS: f64 = 1800.0;
const GC_MAX_PENDING: usize = 200;

#[derive(Debug, Clone)]
pub struct PendingQuestion {
    pub request_id: String,
    pub session_id: String,
    pub questions: Vec<AskQuestion>,
    pub created_at: f64,
    pub status: String,
    pub answer: Option<String>,
}

pub struct AskUserService {
    pending: Mutex<HashMap<String, PendingQuestion>>,
}

impl AskUserService {
    fn new() -> Self {
        Self {
            pending: Mutex::new(HashMap::new()),
        }
    }

    pub fn global() -> &'static AskUserService {
        use std::sync::OnceLock;
        static INSTANCE: OnceLock<AskUserService> = OnceLock::new();
        INSTANCE.get_or_init(AskUserService::new)
    }

    pub async fn create_pending(
        &self,
        session_id: &str,
        questions: Vec<AskQuestion>,
    ) -> PendingQuestion {
        let request_id = uuid::Uuid::new_v4().to_string();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();

        let record = PendingQuestion {
            request_id: request_id.clone(),
            session_id: session_id.to_string(),
            questions,
            created_at: now,
            status: "pending".to_string(),
            answer: None,
        };

        let mut pending = self.pending.lock().await;
        pending.insert(request_id, record.clone());
        gc_locked(&mut pending, now);
        record
    }

    pub async fn get_pending_by_session(&self, session_id: &str) -> Option<PendingQuestion> {
        let pending = self.pending.lock().await;
        pending
            .values()
            .filter(|r| r.session_id == session_id && r.status == "pending")
            .min_by(|a, b| a.created_at.partial_cmp(&b.created_at).unwrap())
            .cloned()
    }

    pub async fn clear_by_session(&self, session_id: &str) -> usize {
        let mut pending = self.pending.lock().await;
        let before = pending.len();
        pending.retain(|_, v| !(v.session_id == session_id && v.status == "pending"));
        before - pending.len()
    }

    pub async fn resolve(&self, request_id: &str, answer: &str) -> Option<PendingQuestion> {
        let mut pending = self.pending.lock().await;
        let record = pending.remove(request_id)?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();
        Some(PendingQuestion {
            status: "answered".to_string(),
            answer: Some(answer.to_string()),
            created_at: now,
            ..record
        })
    }
}

fn gc_locked(pending: &mut HashMap<String, PendingQuestion>, now: f64) {
    let expired: Vec<String> = pending
        .iter()
        .filter(|(_, v)| now - v.created_at > GC_MAX_AGE_SECS)
        .map(|(k, _)| k.clone())
        .collect();
    for k in expired {
        pending.remove(&k);
    }
    if pending.len() > GC_MAX_PENDING {
        let overflow = pending.len() - GC_MAX_PENDING;
        let mut entries: Vec<(String, f64)> = pending
            .iter()
            .map(|(k, v)| (k.clone(), v.created_at))
            .collect();
        entries.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        for (key, _) in entries.into_iter().take(overflow) {
            pending.remove(&key);
        }
    }
}

#[derive(Debug, Deserialize)]
struct QuestionItem {
    question: String,
    #[serde(default, rename = "type")]
    mode: Option<AskMode>,
    #[serde(default)]
    options: Option<Vec<String>>,
    #[serde(default)]
    multi_select: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct AskUserArgs {
    #[serde(default)]
    question: Option<String>,
    #[serde(default)]
    questions: Option<Vec<QuestionItem>>,
    #[serde(default)]
    mode: AskMode,
    #[serde(default)]
    options: Vec<String>,
    #[serde(default)]
    timeout_secs: Option<u64>,
}

const DEFAULT_TIMEOUT_SECS: u64 = 300;

pub struct AskUserTool;

impl AskUserTool {
    pub fn new() -> Self {
        Self
    }

    pub fn spec() -> ToolSpec {
        ToolSpec {
            name: "ask_user".to_string(),
            description: "Ask the user a question and wait for their response. \
                          Supports text input and single/multi select."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "question": {
                        "type": "string",
                        "description": "The question to ask the user (single-question mode)"
                    },
                    "questions": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "question": { "type": "string" },
                                "type": {
                                    "type": "string",
                                    "enum": ["text", "single_select", "multi_select"]
                                },
                                "options": {
                                    "type": "array",
                                    "items": { "type": "string" }
                                },
                                "multi_select": {
                                    "type": "boolean",
                                    "description": "Allow multiple selections (default: false)"
                                }
                            },
                            "required": ["question"]
                        },
                        "description": "Array of questions"
                    },
                    "mode": {
                        "type": "string",
                        "enum": ["text", "single_select", "multi_select"],
                        "description": "Interaction mode (default: text)"
                    },
                    "options": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Options for single/multi select mode"
                    },
                    "timeout_secs": {
                        "type": "integer",
                        "description": "Timeout in seconds (default: 300)"
                    }
                }
            }),
            annotations: ExtendedToolAnnotations {
                read_only: true,
                idempotent: false,
                requires_approval: ApprovalPolicy::Never,
                allowed_on_mobile: true,
                ..Default::default()
            },
            namespace: "builtin".into(),
        }
    }
}

impl Default for AskUserTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for AskUserTool {
    fn name(&self) -> &str {
        "ask_user"
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
        let args: AskUserArgs =
            serde_json::from_value(args).map_err(|e| rua_core::RuaError::Tool {
                tool: "ask_user".into(),
                message: e.to_string(),
            })?;

        let is_background = ctx
            .metadata
            .get("is_background")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        if is_background {
            return Err(rua_core::RuaError::from(
                ToolError::InteractionUnavailable {
                    reason: "Cannot ask user in background context. \
                             The current task is running without a user session."
                        .to_string(),
                },
            ));
        }

        let _timeout = args.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS);

        let questions: Vec<AskQuestion> = if let Some(raw_questions) = args.questions {
            if raw_questions.is_empty() {
                return Err(rua_core::RuaError::Tool {
                    tool: "ask_user".into(),
                    message: "questions array must not be empty".into(),
                });
            }
            raw_questions
                .into_iter()
                .map(|q| {
                    let multi = q
                        .multi_select
                        .unwrap_or(matches!(q.mode.as_ref(), Some(AskMode::MultiSelect)));
                    AskQuestion {
                        question: q.question,
                        options: q.options.unwrap_or_default(),
                        multi_select: multi,
                    }
                })
                .collect()
        } else if let Some(question) = args.question {
            vec![AskQuestion {
                question,
                options: args.options,
                multi_select: args.mode == AskMode::MultiSelect,
            }]
        } else {
            return Err(rua_core::RuaError::Tool {
                tool: "ask_user".into(),
                message: "either 'question' or 'questions' must be provided".into(),
            });
        };

        let svc = AskUserService::global();
        let pending = svc.create_pending(&ctx.session_id, questions.clone()).await;

        let fallback_text = format_questions_text(&questions);

        let mut result = ToolResult::text("ask_user", fallback_text);
        result.metadata.insert(
            "_ask_user".to_string(),
            serde_json::json!({
                "request_id": pending.request_id,
                "questions": questions,
            }),
        );
        Ok(result)
    }
}

fn format_questions_text(questions: &[AskQuestion]) -> String {
    let mut parts: Vec<String> = Vec::new();
    for (i, q) in questions.iter().enumerate() {
        parts.push(format!("Q{}: {}", i + 1, q.question));
        if !q.options.is_empty() {
            let hint = if q.multi_select {
                "(multi-select)"
            } else {
                "(single-select)"
            };
            parts.push(format!("   {hint}"));
            for (j, opt) in q.options.iter().enumerate() {
                parts.push(format!("  {}. {opt}", j + 1));
            }
        }
        parts.push(String::new());
    }
    parts.push("Please reply with your answers.".to_string());
    parts.join("\n")
}
