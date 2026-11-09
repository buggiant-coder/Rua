use std::sync::Arc;

use agent_types::{AgentEvent, FinishReason};
use futures::StreamExt;
use react_agent::test_support::MockChatModel;
use react_agent::{ReactAgent, ReactConfig};
use rua_core::event::AskQuestion;
use rua_tool::extended::ask_user::AskUserService;

fn test_tool_ctx(session_id: &str) -> rua_core::tool::ToolContext {
    rua_core::tool::ToolContext {
        agent_id: "test-agent".into(),
        session_id: session_id.into(),
        working_dir: std::path::PathBuf::from("/tmp"),
        metadata: Default::default(),
        cancel: None,
        task_runtime: None,

        ..Default::default()
    }
}

#[tokio::test]
async fn pending_ask_user_skips_llm_call() {
    let session_id = format!("ask-shortcircuit-{}", uuid::Uuid::new_v4());

    let svc = AskUserService::global();
    svc.create_pending(
        &session_id,
        vec![AskQuestion {
            question: "pick a color".into(),
            options: vec!["red".into(), "blue".into()],
            multi_select: false,
        }],
    )
    .await;

    let model = Arc::new(MockChatModel::text_response("THIS SHOULD NOT APPEAR"));
    let agent = ReactAgent::builder(model)
        .config(ReactConfig::default())
        .tool_ctx(test_tool_ctx(&session_id))
        .build();

    let mut stream = agent.run_stream(vec![agent_types::ModelDialogueItem::text(
        agent_types::Role::User,
        "hello",
    )]);

    let mut got_text = false;
    let mut got_end = false;
    while let Some(event) = stream.next().await {
        match event {
            AgentEvent::TextDelta { .. } => got_text = true,
            AgentEvent::End {
                finish_reason: FinishReason::Stop,
                ..
            } => got_end = true,
            _ => {}
        }
    }

    assert!(
        !got_text,
        "LLM should not have been called — no TextDelta expected"
    );
    assert!(got_end, "stream should end with FinishReason::Stop");
}

#[tokio::test]
async fn no_pending_ask_user_runs_normally() {
    let session_id = format!("no-pending-{}", uuid::Uuid::new_v4());

    let model = Arc::new(MockChatModel::text_response("normal response"));
    let agent = ReactAgent::builder(model)
        .config(ReactConfig::default())
        .tool_ctx(test_tool_ctx(&session_id))
        .build();

    let mut stream = agent.run_stream(vec![agent_types::ModelDialogueItem::text(
        agent_types::Role::User,
        "hello",
    )]);

    let mut got_text = false;
    let mut got_end = false;
    while let Some(event) = stream.next().await {
        match event {
            AgentEvent::TextDelta { delta, .. } if delta.contains("normal response") => {
                got_text = true;
            }
            AgentEvent::End {
                finish_reason: FinishReason::Stop,
                ..
            } => got_end = true,
            _ => {}
        }
    }

    assert!(got_text, "LLM should produce text when no pending ask_user");
    assert!(got_end, "stream should end normally");
}
