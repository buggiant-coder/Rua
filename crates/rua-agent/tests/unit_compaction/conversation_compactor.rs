use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use agent_types::{ContentPart, ModelDialogueItem, Role};
use async_trait::async_trait;
use rua_agent::{
    AgentError, AgentErrorKind, CompactionConfig, CompactionGrade, ConversationCompactor,
    DeviceState, MobileCompactionPolicy, MockSummaryGenerator, SummaryGenerator,
};

fn sys(text: &str) -> ModelDialogueItem {
    ModelDialogueItem::text(Role::System, text)
}

fn user(text: &str) -> ModelDialogueItem {
    ModelDialogueItem::text(Role::User, text)
}

fn asst(text: &str) -> ModelDialogueItem {
    ModelDialogueItem::text(Role::Assistant, text)
}

fn long_conversation() -> Vec<ModelDialogueItem> {
    let mut messages = vec![sys("system prompt")];
    for i in 0..10 {
        messages.push(user(&format!("question {i}")));
        messages.push(asst(&format!("answer {i}")));
    }
    messages
}

#[tokio::test]
async fn compact_noop_when_below_threshold() {
    let config = CompactionConfig {
        keep_last_turns: 10,
        ..Default::default()
    };
    let compactor = ConversationCompactor::new(config);
    let messages = vec![sys("system"), user("hello"), asst("hi")];

    let result = compactor.compact(&messages).await.unwrap();

    assert_eq!(result.compacted_turns, 0);
    assert!(!result.used_llm_summary);
    assert!(!result.used_hard_truncation);
    assert_eq!(result.messages.len(), messages.len());
}

#[tokio::test]
async fn compact_hard_truncation_preserves_invariants() {
    let config = CompactionConfig {
        keep_last_turns: 1,
        enable_llm_summary: false,
        ..Default::default()
    };
    let compactor = ConversationCompactor::new(config);

    let result = compactor.compact(&long_conversation()).await.unwrap();

    assert!(result.used_hard_truncation);
    assert!(!result.used_llm_summary);
    assert_eq!(result.messages[0].role, Role::System);
    assert_eq!(result.messages[0].text_content(), "system prompt");
    assert!(
        result
            .messages
            .iter()
            .any(|message| message.text_content() == "question 9"),
        "last user turn must be preserved after hard truncation"
    );
}

#[tokio::test]
async fn compact_with_mock_summary_generator() {
    let config = CompactionConfig {
        keep_last_turns: 1,
        enable_llm_summary: true,
        ..Default::default()
    };
    let generator = MockSummaryGenerator::new("Summary of earlier conversation.");
    let compactor = ConversationCompactor::new(config).with_summary_generator(Arc::new(generator));

    let result = compactor.compact(&long_conversation()).await.unwrap();

    assert!(result.used_llm_summary);
    assert!(!result.used_hard_truncation);
    assert!(result.summary_message_id.is_some());
    assert!(
        result.messages.iter().any(|message| message
            .text_content()
            .contains("Summary of earlier conversation")),
        "summary text must appear in compacted messages"
    );
    assert!(
        result
            .messages
            .iter()
            .any(|message| message.text_content() == "question 9"),
        "last user turn must survive LLM summary compaction"
    );
}

#[tokio::test]
async fn compact_strips_thinking_only_from_summary_input_when_disabled() {
    let config = CompactionConfig {
        keep_last_turns: 1,
        enable_llm_summary: true,
        compact_with_thinking_block: false,
        ..Default::default()
    };
    let generator = Arc::new(CapturingSummaryGenerator::default());
    let compactor = ConversationCompactor::new(config).with_summary_generator(generator.clone());
    let mut early_answer = asst("visible answer");
    early_answer.content.insert(
        0,
        ContentPart::Thinking {
            text: "private reasoning marker".to_string(),
            signature: None,
        },
    );
    let messages = vec![
        sys("system"),
        user("early question"),
        early_answer,
        user("recent question"),
        asst("recent answer"),
    ];

    let result = compactor.compact(&messages).await.unwrap();

    assert!(result.used_llm_summary);
    let captured = generator.messages();
    assert!(captured
        .iter()
        .any(|message| message.text_content().contains("visible answer")));
    assert!(captured.iter().all(|message| {
        message
            .content
            .iter()
            .all(|part| !matches!(part, ContentPart::Thinking { .. }))
    }));
    assert!(result.messages.iter().any(|message| {
        message
            .content
            .iter()
            .any(|part| matches!(part, ContentPart::Text { text } if text == "recent answer"))
    }));
}

#[tokio::test]
async fn compact_keeps_only_the_newest_configured_thinking_turns() {
    let config = CompactionConfig {
        keep_last_turns: 1,
        enable_llm_summary: true,
        compact_with_thinking_block: true,
        thinking_turns_to_keep: 1,
        ..Default::default()
    };
    let generator = Arc::new(CapturingSummaryGenerator::default());
    let compactor = ConversationCompactor::new(config).with_summary_generator(generator.clone());
    let assistant_with_thinking = |thinking: &str, answer: &str| ModelDialogueItem {
        id: None,
        role: Role::Assistant,
        content: vec![
            ContentPart::Thinking {
                text: thinking.to_string(),
                signature: None,
            },
            ContentPart::Text {
                text: answer.to_string(),
            },
        ],
        name: None,
        created_at: None,
        metadata: serde_json::Map::new(),
    };
    let messages = vec![
        sys("system"),
        user("question one"),
        assistant_with_thinking("old thinking", "answer one"),
        user("question two"),
        assistant_with_thinking("newer thinking", "answer two"),
        user("recent question"),
        asst("recent answer"),
    ];

    compactor.compact(&messages).await.unwrap();

    let captured = generator.messages();
    let thinking = captured
        .iter()
        .flat_map(|message| message.content.iter())
        .filter_map(|part| match part {
            ContentPart::Thinking { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(thinking, vec!["newer thinking"]);
}

#[tokio::test]
async fn compact_keeps_all_thinking_when_configured_with_minus_one() {
    let config = CompactionConfig {
        keep_last_turns: 1,
        enable_llm_summary: true,
        compact_with_thinking_block: true,
        thinking_turns_to_keep: -1,
        ..Default::default()
    };
    let generator = Arc::new(CapturingSummaryGenerator::default());
    let compactor = ConversationCompactor::new(config).with_summary_generator(generator.clone());
    let messages = vec![
        sys("system"),
        user("question one"),
        ModelDialogueItem {
            id: None,
            role: Role::Assistant,
            content: vec![
                ContentPart::Thinking {
                    text: "thinking one".to_string(),
                    signature: None,
                },
                ContentPart::Text {
                    text: "answer one".to_string(),
                },
            ],
            name: None,
            created_at: None,
            metadata: serde_json::Map::new(),
        },
        user("question two"),
        ModelDialogueItem {
            id: None,
            role: Role::Assistant,
            content: vec![
                ContentPart::Thinking {
                    text: "thinking two".to_string(),
                    signature: None,
                },
                ContentPart::Text {
                    text: "answer two".to_string(),
                },
            ],
            name: None,
            created_at: None,
            metadata: serde_json::Map::new(),
        },
        user("recent question"),
        asst("recent answer"),
    ];

    compactor.compact(&messages).await.unwrap();

    let thinking = generator
        .messages()
        .iter()
        .flat_map(|message| message.content.iter())
        .filter_map(|part| match part {
            ContentPart::Thinking { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .map(str::to_string)
        .collect::<Vec<_>>();
    assert_eq!(thinking, vec!["thinking one", "thinking two"]);
}

#[tokio::test]
async fn compact_falls_back_to_truncation_on_summary_failure() {
    let config = CompactionConfig {
        keep_last_turns: 1,
        enable_llm_summary: true,
        ..Default::default()
    };
    let generator = MockSummaryGenerator::failing();
    let compactor = ConversationCompactor::new(config).with_summary_generator(Arc::new(generator));

    let result = compactor.compact(&long_conversation()).await.unwrap();

    assert!(!result.used_llm_summary);
    assert!(result.used_hard_truncation);
    assert!(result.audit.summary_attempted);
    assert!(!result.audit.summary_succeeded);
}

#[tokio::test]
async fn compact_falls_back_to_hard_truncation_when_summary_disabled() {
    let config = CompactionConfig {
        keep_last_turns: 1,
        enable_llm_summary: false,
        ..Default::default()
    };
    let compactor = ConversationCompactor::new(config);

    let result = compactor.compact(&long_conversation()).await.unwrap();

    assert!(result.used_hard_truncation);
    assert!(!result.used_llm_summary);
}

#[test]
fn mobile_policy_defaults() {
    let policy = MobileCompactionPolicy::default();

    assert!(policy.allow_summary(&DeviceState::Foreground));
    assert!(!policy.allow_summary(&DeviceState::Background));
    assert!(policy.allow_summary(&DeviceState::ChargingWifi));
}

#[tokio::test]
async fn compact_graded_updates_existing_summary_with_folded_tail() {
    let config = CompactionConfig {
        keep_last_turns: 1,
        enable_llm_summary: true,
        ..Default::default()
    };
    let generator = Arc::new(UpdatingSummaryGenerator::new("updated summary"));
    let compactor = ConversationCompactor::new(config).with_summary_generator(generator.clone());

    let result = compactor
        .compact_graded(
            &long_conversation(),
            Some("Previous structured summary to preserve."),
        )
        .await
        .unwrap();

    assert_eq!(result.grade, Some(CompactionGrade::G6LlmSummary));
    assert!(result.used_llm_summary);
    assert_eq!(
        generator.call_count(),
        1,
        "incremental compact must call the LLM to update the existing summary"
    );
    assert_eq!(
        generator.previous_summary(),
        Some("Previous structured summary to preserve.".to_string())
    );
    assert!(
        result
            .messages
            .iter()
            .any(|message| message.text_content().contains("updated summary")),
        "updated summary should be inserted into compacted messages"
    );
}

#[tokio::test]
async fn g5_ptl_retry_drops_messages_and_succeeds() {
    let config = CompactionConfig {
        keep_last_turns: 1,
        enable_llm_summary: true,
        ..Default::default()
    };
    let generator = Arc::new(PtlThenSuccessGenerator::new(
        2,
        "Summary after PTL retries".into(),
    ));
    let compactor = ConversationCompactor::new(config).with_summary_generator(generator.clone());

    let result = compactor
        .compact_graded(&long_conversation(), None)
        .await
        .unwrap();

    assert_eq!(
        result.grade,
        Some(CompactionGrade::G5PtlRetry),
        "must be G5 since first 2 attempts failed with PTL"
    );
    assert!(result.used_llm_summary);
    assert_eq!(
        generator.call_count(),
        3,
        "2 PTL failures + 1 success = 3 calls"
    );
    assert!(
        result
            .messages
            .iter()
            .any(|message| message.text_content().contains("Summary after PTL retries")),
        "summary text must appear in result"
    );
}

#[tokio::test]
async fn compact_updating_summary_uses_previous_summary_path() {
    let config = CompactionConfig {
        keep_last_turns: 1,
        enable_llm_summary: true,
        ..Default::default()
    };
    let generator = Arc::new(UpdatingSummaryGenerator::new(
        "Updated replacement summary with old and new context",
    ));
    let compactor = ConversationCompactor::new(config).with_summary_generator(generator.clone());

    let result = compactor
        .compact_updating_summary(
            &long_conversation(),
            Some("Previous structured summary to preserve."),
        )
        .await
        .unwrap();

    assert!(result.used_llm_summary);
    assert_eq!(result.grade, Some(CompactionGrade::G6LlmSummary));
    assert_eq!(
        generator.call_count(),
        1,
        "summary generation should use the unified summarize path"
    );
    assert_eq!(
        generator.previous_summary(),
        Some("Previous structured summary to preserve.".to_string())
    );
    assert!(
        result.messages.iter().any(|message| message
            .text_content()
            .contains("Updated replacement summary")),
        "updated replacement summary must be persisted in the compacted messages"
    );
}

struct UpdatingSummaryGenerator {
    response: String,
    calls: AtomicU64,
    previous_summary: std::sync::Mutex<Option<String>>,
}

#[derive(Default)]
struct CapturingSummaryGenerator {
    messages: std::sync::Mutex<Vec<ModelDialogueItem>>,
}

impl CapturingSummaryGenerator {
    fn messages(&self) -> Vec<ModelDialogueItem> {
        self.messages.lock().unwrap().clone()
    }
}

#[async_trait]
impl SummaryGenerator for CapturingSummaryGenerator {
    async fn summarize(
        &self,
        messages: &[ModelDialogueItem],
        _previous_summary: Option<&str>,
        _max_tokens: Option<usize>,
        _model: Option<&rua_session::CheckpointModelIdentity>,
    ) -> Result<String, AgentError> {
        *self.messages.lock().unwrap() = messages.to_vec();
        Ok("summary without thinking".to_string())
    }
}

impl UpdatingSummaryGenerator {
    fn new(response: &str) -> Self {
        Self {
            response: response.into(),
            calls: AtomicU64::new(0),
            previous_summary: std::sync::Mutex::new(None),
        }
    }

    fn call_count(&self) -> u64 {
        self.calls.load(Ordering::Relaxed)
    }

    fn previous_summary(&self) -> Option<String> {
        self.previous_summary.lock().unwrap().clone()
    }
}

#[async_trait]
impl SummaryGenerator for UpdatingSummaryGenerator {
    async fn summarize(
        &self,
        _messages: &[ModelDialogueItem],
        previous_summary: Option<&str>,
        _max_tokens: Option<usize>,
        _model: Option<&rua_session::CheckpointModelIdentity>,
    ) -> Result<String, AgentError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        *self.previous_summary.lock().unwrap() = previous_summary.map(str::to_string);
        assert_eq!(
            previous_summary,
            Some("Previous structured summary to preserve.")
        );
        Ok(self.response.clone())
    }
}

struct PtlThenSuccessGenerator {
    fail_count: usize,
    success_response: String,
    calls: AtomicU64,
}

impl PtlThenSuccessGenerator {
    fn new(fail_count: usize, success_response: String) -> Self {
        Self {
            fail_count,
            success_response,
            calls: AtomicU64::new(0),
        }
    }

    fn call_count(&self) -> u64 {
        self.calls.load(Ordering::Relaxed)
    }
}

#[async_trait]
impl SummaryGenerator for PtlThenSuccessGenerator {
    async fn summarize(
        &self,
        _messages: &[ModelDialogueItem],
        _previous_summary: Option<&str>,
        _max_tokens: Option<usize>,
        _model: Option<&rua_session::CheckpointModelIdentity>,
    ) -> Result<String, AgentError> {
        let attempt = self.calls.fetch_add(1, Ordering::Relaxed);
        if (attempt as usize) < self.fail_count {
            Err(AgentError::new(
                AgentErrorKind::Provider,
                "prompt is too long for this model",
            ))
        } else {
            Ok(self.success_response.clone())
        }
    }
}
