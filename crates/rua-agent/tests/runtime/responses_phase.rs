use std::sync::Arc;

use agent_types::{AgentEvent, ChatRequest, ChatResponse, FinishReason, ModelDialogueItem, Role};
use async_trait::async_trait;
use futures::StreamExt;
use llm_multiplex::providers::openai_responses::{
    parse_response_stream_event_with_state, ResponseStreamParseState,
};
use llm_multiplex::{
    ChatModel, EventStream, LlmError, ModelInfo, OpenAiCompatibleConfig, OpenAiResponsesProvider,
};
use rua_agent::projection_preparation::project_message_log_entries_for_model;
use rua_agent::turn_persistence::TurnPersistenceHook;
use rua_contract::agent::AgentRequest;
use rua_session::{
    ChatMetadata, ChatSessionHeader, NewConversationMessage, NewConversationTurn, TurnLifecycle,
    WorkspaceSessionStore,
};
use serde_json::{json, Value};

struct ResponsesPhaseModel {
    complete: bool,
    info: ModelInfo,
}

#[async_trait]
impl ChatModel for ResponsesPhaseModel {
    async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, LlmError> {
        unreachable!("stream-only test model")
    }

    fn chat_stream(&self, _request: ChatRequest) -> EventStream {
        let mut state = ResponseStreamParseState::default();
        let mut events = Vec::new();
        let messages = if self.complete {
            vec![
                ("provider-1", "commentary", "Working."),
                ("provider-2", "commentary", "More work."),
                ("provider-3", "final_answer", "Done."),
            ]
        } else {
            vec![("provider-1", "commentary", "Partial work.")]
        };
        for (index, (id, phase, text)) in messages.iter().enumerate() {
            // Some providers supply phase only when a message is done.
            let added_phase = if *phase == "final_answer" {
                Value::Null
            } else {
                json!(phase)
            };
            for value in [
                json!({"type": "response.output_item.added", "output_index": index,
                       "item": {"type": "message", "id": id, "role": "assistant", "phase": added_phase}}),
                json!({"type": "response.output_text.delta", "output_index": index,
                       "item_id": id, "delta": text}),
                json!({"type": "response.output_item.done", "output_index": index,
                       "item": {"type": "message", "id": id, "role": "assistant", "phase": phase,
                                "content": [{"type": "output_text", "text": text}]}}),
            ] {
                events.extend(parse_response_stream_event_with_state(&value, &mut state).events);
            }
        }
        if self.complete {
            events.push(AgentEvent::End {
                finish_reason: FinishReason::Stop,
                usage: None,
                metadata: Value::Null,
            });
        } else {
            events.push(AgentEvent::Error {
                code: Some("stream_interrupted".into()),
                message: "stream interrupted".into(),
                retryable: false,
                metadata: Value::Null,
            });
        }
        Box::pin(futures::stream::iter(events.into_iter().map(Ok)))
    }

    fn model_info(&self) -> &ModelInfo {
        &self.info
    }
}

async fn assert_phase_persistence(complete: bool) {
    let workspace = tempfile::tempdir().unwrap();
    let chat_id = "responses-phase";
    let turn_id = "turn-phase";
    let store = WorkspaceSessionStore::new(workspace.path());
    store
        .save_chat_header(&ChatSessionHeader::new(ChatMetadata {
            chat_id: chat_id.into(),
            agent_id: "agent-1".into(),
            title: "Phase".into(),
            created_at: "2026-09-21T00:00:00Z".into(),
            last_activity_at: "2026-09-21T00:00:00Z".into(),
            pinned: false,
            archived: false,
            preview: None,
        }))
        .await
        .unwrap();
    store
        .append_conversation_turn(
            chat_id,
            NewConversationTurn {
                turn_id: turn_id.into(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-09-21T00:00:00Z".into(),
                    assistant_started_at: None,
                    assistant_completed_at: None,
                    status: "in_progress".into(),
                },
                selected_variant_id: None,
                messages: vec![NewConversationMessage {
                    message_id: "user-1".into(),
                    role: "user".into(),
                    created_at: "2026-09-21T00:00:00Z".into(),
                    status: "completed".into(),
                    content: json!([{"type": "text", "text": "continue"}]),
                    provider: None,
                    tool: None,
                    metadata: Default::default(),
                }],
            },
        )
        .await
        .unwrap();
    let request = AgentRequest {
        session_id: chat_id.into(),
        user_id: "user-1".into(),
        agent_id: "agent-1".into(),
        messages: vec![],
        model_override: None,
        tool_mode: "auto".into(),
        attachments: vec![],
        display_preferences: None,
        disabled_tools: vec![],
        metadata: [
            ("turn_id".into(), json!(turn_id)),
            ("loaded_message_count".into(), json!(1)),
        ]
        .into(),
        cancel: None,
        run_control: None,
        workspace_dir: Some(workspace.path().into()),
        conversation_store_dir: Some(workspace.path().into()),
        working_dir: None,
    };
    let model = Arc::new(ResponsesPhaseModel {
        complete,
        info: ModelInfo {
            id: "test".into(),
            name: "test".into(),
            context_window: None,
            max_output_tokens: None,
            supports_multimodal: None,
            supports_image: None,
            supports_video: None,
            probe_source: None,
            generate_kwargs: Default::default(),
        },
    });
    let events: Vec<_> = react_agent::ReactAgent::builder(model)
        .hook(Arc::new(
            TurnPersistenceHook::from_request(&request).unwrap(),
        ))
        .build()
        .run_stream(vec![ModelDialogueItem::text(Role::User, "continue")])
        .collect()
        .await;
    assert_eq!(
        events
            .iter()
            .any(|event| matches!(event, AgentEvent::Error { .. })),
        !complete
    );
    let entries = WorkspaceSessionStore::new(workspace.path())
        .read_turn_messages(chat_id, turn_id)
        .await
        .unwrap();
    let assistant: Vec<_> = entries
        .iter()
        .filter(|entry| entry.role == "assistant")
        .collect();
    let expected = if complete {
        vec![
            ("commentary", "Working."),
            ("commentary", "More work."),
            ("final_answer", "Done."),
        ]
    } else {
        vec![("commentary", "Partial work.")]
    };
    assert_eq!(
        assistant.len(),
        expected.len(),
        "preserve provider message boundaries in the log"
    );
    for (entry, (phase, text)) in assistant.iter().zip(&expected) {
        assert_eq!(entry.metadata["phase"], *phase);
        assert_eq!(entry.content[0]["text"], *text);
        let stream_id = events
            .iter()
            .find_map(|event| match event {
                AgentEvent::TextDelta { delta, metadata } if delta == text => {
                    metadata["rua_message_id"].as_str()
                }
                _ => None,
            })
            .unwrap();
        assert_eq!(
            entry.message_id, stream_id,
            "stream and durable identities must agree"
        );
    }
    let projected = project_message_log_entries_for_model(&entries);
    let provider = OpenAiResponsesProvider::new(OpenAiCompatibleConfig {
        provider_id: "responses".into(),
        provider_name: "Responses".into(),
        base_url: "http://127.0.0.1:9/v1".into(),
        api_key: None,
        api_keys: vec![],
        headers: Default::default(),
        models: vec![],
        timeout_secs: 1,
        generate_kwargs: Default::default(),
        support_model_discovery: false,
        support_connection_check: false,
        is_local: true,
    })
    .unwrap();
    let wire = provider.build_request_body(&ChatRequest::new(projected).with_model("gpt-5.6-sol"));
    let input = wire["input"].as_array().unwrap();
    assert_eq!(input.len(), expected.len() + 1);
    assert!(input[0].get("phase").is_none());
    for (item, (phase, text)) in input[1..].iter().zip(expected) {
        assert_eq!(item["phase"], phase);
        assert_eq!(item["content"], text);
    }
}

#[tokio::test]
async fn phases_survive_stream_persistence_reload_projection_and_wire() {
    assert_phase_persistence(true).await;
}

#[tokio::test]
async fn interrupted_stream_preserves_partial_message_phase() {
    assert_phase_persistence(false).await;
}
