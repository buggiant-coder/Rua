use agent_types::AgentEvent;
use futures::StreamExt;
use llm_multiplex::SanitizedStream;

fn make_event_stream(
    events: Vec<Result<AgentEvent, llm_multiplex::LlmError>>,
) -> llm_multiplex::EventStream {
    Box::pin(futures::stream::iter(events))
}

#[tokio::test]
async fn no_duplicate_text_delta_at_end() {
    let events = vec![
        Ok(AgentEvent::TextDelta {
            delta: "hello ".into(),
            metadata: serde_json::Value::Null,
        }),
        Ok(AgentEvent::TextDelta {
            delta: "world".into(),
            metadata: serde_json::Value::Null,
        }),
        Ok(AgentEvent::End {
            finish_reason: agent_types::FinishReason::Stop,
            usage: None,
            metadata: serde_json::Value::Null,
        }),
    ];

    let inner = make_event_stream(events);
    let mut stream = SanitizedStream::wrap(inner);

    let mut text_deltas = Vec::new();
    while let Some(Ok(event)) = stream.next().await {
        if let AgentEvent::TextDelta { delta, .. } = &event {
            text_deltas.push(delta.clone());
        }
    }

    let total_text: String = text_deltas.iter().cloned().collect();
    assert_eq!(
        total_text, "hello world",
        "accumulated text should be 'hello world' (not doubled); got deltas: {:?}",
        text_deltas
    );
    assert_eq!(
        text_deltas.len(),
        2,
        "should emit exactly 2 TextDelta events (pass-through only); got: {:?}",
        text_deltas
    );
}
