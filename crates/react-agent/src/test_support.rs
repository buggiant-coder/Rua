use agent_types::{
    AgentEvent, ChatRequest, ChatResponse, FinishReason, ModelDialogueItem, Role, TokenUsage,
};
use async_trait::async_trait;
use llm_multiplex::traits::{ChatModel, EventStream, ModelInfo};
use llm_multiplex::LlmError;

pub struct MockChatModel {
    event_sequences: tokio::sync::Mutex<Vec<Vec<AgentEvent>>>,

    info: ModelInfo,

    call_count: tokio::sync::Mutex<usize>,
}

impl MockChatModel {
    pub fn text_response(text: &str) -> Self {
        let events = vec![
            AgentEvent::Start {
                run_id: "mock-run".into(),
                model: Some("mock-model".into()),
                metadata: serde_json::Value::Null,
            },
            AgentEvent::TextDelta {
                delta: text.to_string(),
                metadata: serde_json::Value::Null,
            },
            AgentEvent::End {
                finish_reason: FinishReason::Stop,
                usage: Some(TokenUsage::new(10, 5)),
                metadata: serde_json::Value::Null,
            },
        ];
        Self::from_sequences(vec![events])
    }

    pub fn tool_call_response(tool_name: &str, args: serde_json::Value) -> Self {
        let call_id = format!("tc-mock-{}", uuid::Uuid::new_v4());
        let events = vec![
            AgentEvent::Start {
                run_id: "mock-run".into(),
                model: Some("mock-model".into()),
                metadata: serde_json::Value::Null,
            },
            AgentEvent::ToolCallDelta {
                id: call_id.clone(),
                index: 0,
                name: Some(tool_name.to_string()),
                arguments_delta: serde_json::to_string(&args).unwrap(),
                metadata: serde_json::Value::Null,
            },
            AgentEvent::End {
                finish_reason: FinishReason::ToolUse,
                usage: Some(TokenUsage::new(10, 5)),
                metadata: serde_json::Value::Null,
            },
        ];
        Self::from_sequences(vec![events])
    }

    pub fn tool_then_text(tool_name: &str, args: serde_json::Value, final_text: &str) -> Self {
        let call_id = format!("tc-mock-{}", uuid::Uuid::new_v4());
        let tool_events = vec![
            AgentEvent::Start {
                run_id: "mock-run".into(),
                model: Some("mock-model".into()),
                metadata: serde_json::Value::Null,
            },
            AgentEvent::ToolCallDelta {
                id: call_id,
                index: 0,
                name: Some(tool_name.to_string()),
                arguments_delta: serde_json::to_string(&args).unwrap(),
                metadata: serde_json::Value::Null,
            },
            AgentEvent::End {
                finish_reason: FinishReason::ToolUse,
                usage: Some(TokenUsage::new(10, 5)),
                metadata: serde_json::Value::Null,
            },
        ];
        let text_events = vec![
            AgentEvent::Start {
                run_id: "mock-run".into(),
                model: Some("mock-model".into()),
                metadata: serde_json::Value::Null,
            },
            AgentEvent::TextDelta {
                delta: final_text.to_string(),
                metadata: serde_json::Value::Null,
            },
            AgentEvent::End {
                finish_reason: FinishReason::Stop,
                usage: Some(TokenUsage::new(10, 5)),
                metadata: serde_json::Value::Null,
            },
        ];
        Self::from_sequences(vec![tool_events, text_events])
    }

    pub fn thinking_response(thinking: &str, text: &str) -> Self {
        let events = vec![
            AgentEvent::Start {
                run_id: "mock-run".into(),
                model: Some("mock-model".into()),
                metadata: serde_json::Value::Null,
            },
            AgentEvent::ThinkingDelta {
                delta: thinking.to_string(),
                signature: Some("sig-mock".into()),
                metadata: serde_json::Value::Null,
            },
            AgentEvent::TextDelta {
                delta: text.to_string(),
                metadata: serde_json::Value::Null,
            },
            AgentEvent::End {
                finish_reason: FinishReason::Stop,
                usage: Some(TokenUsage::new(10, 5)),
                metadata: serde_json::Value::Null,
            },
        ];
        Self::from_sequences(vec![events])
    }

    pub fn error_response(message: &str) -> Self {
        let events = vec![
            AgentEvent::Start {
                run_id: "mock-run".into(),
                model: Some("mock-model".into()),
                metadata: serde_json::Value::Null,
            },
            AgentEvent::Error {
                code: None,
                message: message.to_string(),
                retryable: false,
                metadata: serde_json::Value::Null,
            },
        ];
        Self::from_sequences(vec![events])
    }

    pub fn context_overflow_response() -> Self {
        let events = vec![
            AgentEvent::Start {
                run_id: "mock-run".into(),
                model: Some("mock-model".into()),
                metadata: serde_json::Value::Null,
            },
            AgentEvent::Error {
                code: Some("context_length_exceeded".into()),
                message: "context window exceeded".to_string(),
                retryable: false,
                metadata: serde_json::Value::Null,
            },
        ];
        Self::from_sequences(vec![events])
    }

    pub fn from_sequences(sequences: Vec<Vec<AgentEvent>>) -> Self {
        Self {
            event_sequences: tokio::sync::Mutex::new(sequences),
            info: ModelInfo {
                id: "mock-model".into(),
                name: "Mock Model".into(),
                context_window: Some(128_000),
                max_output_tokens: Some(4096),
                supports_multimodal: None,
                supports_image: None,
                supports_video: None,
                probe_source: Some("mock".into()),
                generate_kwargs: serde_json::Map::new(),
            },
            call_count: tokio::sync::Mutex::new(0),
        }
    }

    pub fn multi_tool_iterations(iterations: usize, final_text: &str) -> Self {
        let mut sequences = Vec::new();

        for i in 0..iterations {
            let call_id = format!("tc-{i}");
            let tool_events = vec![
                AgentEvent::Start {
                    run_id: "mock-run".into(),
                    model: Some("mock-model".into()),
                    metadata: serde_json::Value::Null,
                },
                AgentEvent::ToolCallDelta {
                    id: call_id,
                    index: 0,
                    name: Some("bash".into()),
                    arguments_delta: serde_json::json!({"command": format!("cmd-{i}")}).to_string(),
                    metadata: serde_json::Value::Null,
                },
                AgentEvent::End {
                    finish_reason: FinishReason::ToolUse,
                    usage: Some(TokenUsage::new(10, 5)),
                    metadata: serde_json::Value::Null,
                },
            ];
            sequences.push(tool_events);
        }

        let text_events = vec![
            AgentEvent::Start {
                run_id: "mock-run".into(),
                model: Some("mock-model".into()),
                metadata: serde_json::Value::Null,
            },
            AgentEvent::TextDelta {
                delta: final_text.to_string(),
                metadata: serde_json::Value::Null,
            },
            AgentEvent::End {
                finish_reason: FinishReason::Stop,
                usage: Some(TokenUsage::new(10, 5)),
                metadata: serde_json::Value::Null,
            },
        ];
        sequences.push(text_events);

        Self::from_sequences(sequences)
    }

    pub fn with_multimodal(mut self, supports: Option<bool>) -> Self {
        self.info.supports_multimodal = supports;
        self
    }

    pub fn split_tool_call_deltas(tool_name: &str, delta_fragments: Vec<&str>) -> Self {
        let call_id = "tc-split".to_string();
        let mut events = vec![AgentEvent::Start {
            run_id: "mock-run".into(),
            model: Some("mock-model".into()),
            metadata: serde_json::Value::Null,
        }];

        for (i, fragment) in delta_fragments.iter().enumerate() {
            events.push(AgentEvent::ToolCallDelta {
                id: if i == 0 {
                    call_id.clone()
                } else {
                    String::new()
                },
                index: 0,
                name: if i == 0 {
                    Some(tool_name.to_string())
                } else {
                    None
                },
                arguments_delta: fragment.to_string(),
                metadata: serde_json::Value::Null,
            });
        }

        events.push(AgentEvent::End {
            finish_reason: FinishReason::ToolUse,
            usage: Some(TokenUsage::new(10, 5)),
            metadata: serde_json::Value::Null,
        });

        Self::from_sequences(vec![events])
    }
}

#[async_trait]
impl ChatModel for MockChatModel {
    async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, LlmError> {
        Ok(ChatResponse {
            message: ModelDialogueItem::text(Role::Assistant, "mock response"),
            usage: Some(TokenUsage::new(10, 5)),
            model: Some("mock-model".into()),
            finish_reason: FinishReason::Stop,
            metadata: serde_json::Map::new(),
            extra: Default::default(),
        })
    }

    fn chat_stream(&self, _request: ChatRequest) -> EventStream {
        let sequences = self.event_sequences.try_lock();
        let events = match sequences {
            Ok(mut seqs) => {
                if seqs.len() > 1 {
                    seqs.remove(0)
                } else if !seqs.is_empty() {
                    seqs[0].clone()
                } else {
                    vec![
                        AgentEvent::Start {
                            run_id: "mock-run".into(),
                            model: Some("mock-model".into()),
                            metadata: serde_json::Value::Null,
                        },
                        AgentEvent::End {
                            finish_reason: FinishReason::Stop,
                            usage: None,
                            metadata: serde_json::Value::Null,
                        },
                    ]
                }
            }
            Err(_) => {
                vec![
                    AgentEvent::Start {
                        run_id: "mock-run".into(),
                        model: Some("mock-model".into()),
                        metadata: serde_json::Value::Null,
                    },
                    AgentEvent::End {
                        finish_reason: FinishReason::Stop,
                        usage: None,
                        metadata: serde_json::Value::Null,
                    },
                ]
            }
        };

        if let Ok(mut count) = self.call_count.try_lock() {
            *count += 1;
        }

        Box::pin(futures::stream::iter(events.into_iter().map(Ok)))
    }

    fn model_info(&self) -> &ModelInfo {
        &self.info
    }
}

pub async fn collect_events(stream: crate::agent::AgentEventStream) -> Vec<AgentEvent> {
    use futures::StreamExt;
    stream.collect().await
}

pub fn test_tool_ctx() -> rua_core::tool::ToolContext {
    rua_core::tool::ToolContext {
        agent_id: "test-agent".into(),
        session_id: "test-session".into(),
        working_dir: std::path::PathBuf::from("/tmp"),
        metadata: Default::default(),
        cancel: None,
        task_runtime: None,
        ..Default::default()
    }
}
