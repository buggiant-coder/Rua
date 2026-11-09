use std::pin::Pin;
use std::sync::{Arc, Mutex};

use agent_types::{
    AgentEvent, ChatRequest, ChatResponse, FinishReason, ModelDialogueItem, Role, TokenUsage,
    ToolDefinition,
};
use async_trait::async_trait;
use futures::{Stream, StreamExt};
use llm_multiplex::traits::{ChatModel, EventStream, ModelInfo};
use llm_multiplex::LlmError;
use react_agent::hooks::{HookEvent, RecordingHook};
use react_agent::toolkit::{
    AgentToolkit, DisclosureClass, MockToolExecutor, ToolDisclosureMeta, ToolSourceKind,
};
use react_agent::{
    build_skills_prompt_block, register_skill_pseudo_tool, ReactAgent, ReactConfig, SkillInfo,
    ToolDisclosureConfig, ToolDisclosureMode,
};
use serde_json::json;

struct CapturingToolModel {
    requests: Arc<Mutex<Vec<ChatRequest>>>,
    event_sequences: Mutex<Vec<Vec<AgentEvent>>>,
    info: ModelInfo,
}

impl CapturingToolModel {
    fn new(event_sequences: Vec<Vec<AgentEvent>>) -> Self {
        Self {
            requests: Arc::new(Mutex::new(Vec::new())),
            event_sequences: Mutex::new(event_sequences),
            info: ModelInfo {
                id: "tool-disclosure-test-model".into(),
                name: "Tool Disclosure Test Model".into(),
                context_window: Some(1_000),
                max_output_tokens: Some(256),
                supports_multimodal: None,
                supports_image: None,
                supports_video: None,
                probe_source: Some("test".into()),
                generate_kwargs: serde_json::Map::new(),
            },
        }
    }

    fn requests(&self) -> Arc<Mutex<Vec<ChatRequest>>> {
        Arc::clone(&self.requests)
    }
}

#[async_trait]
impl ChatModel for CapturingToolModel {
    async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, LlmError> {
        Ok(ChatResponse {
            message: ModelDialogueItem::text(Role::Assistant, "unused"),
            usage: Some(TokenUsage::new(1, 1)),
            model: Some(self.info.id.clone()),
            finish_reason: FinishReason::Stop,
            metadata: serde_json::Map::new(),
            extra: Default::default(),
        })
    }

    fn chat_stream(&self, request: ChatRequest) -> EventStream {
        self.requests.lock().unwrap().push(request);
        let events = self
            .event_sequences
            .lock()
            .unwrap()
            .drain(..1)
            .next()
            .unwrap_or_else(|| {
                vec![AgentEvent::End {
                    finish_reason: FinishReason::Stop,
                    usage: None,
                    metadata: serde_json::Value::Null,
                }]
            });
        let stream: Pin<Box<dyn Stream<Item = Result<AgentEvent, LlmError>> + Send>> =
            Box::pin(futures::stream::iter(events.into_iter().map(Ok)));
        stream
    }

    fn model_info(&self) -> &ModelInfo {
        &self.info
    }
}

fn tool_definition(name: &str, description: &str, parameters: serde_json::Value) -> ToolDefinition {
    ToolDefinition {
        name: name.to_string(),
        description: description.to_string(),
        parameters,
        strict: None,
        annotations: Default::default(),
        metadata: Default::default(),
    }
}

fn meta(class: DisclosureClass, source_kind: ToolSourceKind) -> ToolDisclosureMeta {
    ToolDisclosureMeta {
        class,
        source_kind,
        source_name: Some("e2e".into()),
        category: Some("e2e".into()),
        schema_tokens: Some(50),
    }
}

fn disclosure_toolkit(skill: SkillInfo) -> AgentToolkit {
    let mut toolkit = AgentToolkit::new();
    toolkit.register_with_disclosure(
        tool_definition(
            "read",
            "Read a file from the workspace",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" }
                },
                "required": ["path"]
            }),
        ),
        Arc::new(MockToolExecutor::new("read ok")),
        meta(DisclosureClass::Direct, ToolSourceKind::Builtin),
    );
    toolkit.register_with_disclosure(
        tool_definition(
            "task_create",
            "Create a project issue or task",
            json!({
                "type": "object",
                "properties": {
                    "title": { "type": "string" },
                    "priority": { "type": "string" }
                },
                "required": ["title"]
            }),
        ),
        Arc::new(MockToolExecutor::new("task ok")),
        meta(DisclosureClass::DeferredCandidate, ToolSourceKind::Builtin),
    );
    register_skill_pseudo_tool(&mut toolkit, vec![skill]).expect("skill pseudo-tool registers");
    toolkit
}

fn tool_use(call_id: &str, tool_name: &str, args: serde_json::Value) -> Vec<AgentEvent> {
    vec![
        AgentEvent::ToolCallDelta {
            id: call_id.to_string(),
            index: 0,
            name: Some(tool_name.to_string()),
            arguments_delta: serde_json::to_string(&args).expect("tool args json"),
            metadata: serde_json::Value::Null,
        },
        AgentEvent::End {
            finish_reason: FinishReason::ToolUse,
            usage: Some(TokenUsage::new(10, 5)),
            metadata: serde_json::Value::Null,
        },
    ]
}

fn final_text(text: &str) -> Vec<AgentEvent> {
    vec![
        AgentEvent::TextDelta {
            delta: text.to_string(),
            metadata: serde_json::Value::Null,
        },
        AgentEvent::End {
            finish_reason: FinishReason::Stop,
            usage: Some(TokenUsage::new(10, 5)),
            metadata: serde_json::Value::Null,
        },
    ]
}

fn tool_names(request: &ChatRequest) -> Vec<String> {
    let mut names: Vec<String> = request.tools.iter().map(|tool| tool.name.clone()).collect();
    names.sort();
    names
}

#[tokio::test]
async fn console_react_path_discloses_core_tools_and_defers_complex_tools_end_to_end() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let skill = SkillInfo {
        name: "diagnose".into(),
        description: "Debug failures".into(),
        body: "Run a disciplined diagnosis loop.".into(),
        source: react_agent::SkillRuntimeSource::Agent,
        base_dir: tmp.path().join("skills/diagnose"),
    };
    let skill_prompt_block = build_skills_prompt_block(std::slice::from_ref(&skill));
    let toolkit = Arc::new(disclosure_toolkit(skill));
    let config = ReactConfig {
        tool_disclosure: ToolDisclosureConfig {
            enabled: ToolDisclosureMode::On,
            threshold_pct: 10.0,
            search_default_limit: 5,
            search_max_limit: 20,
        },
        ..Default::default()
    };

    let model = Arc::new(CapturingToolModel::new(vec![
        tool_use(
            "search-call",
            "tool_search",
            json!({"query": "project issue", "limit": 10}),
        ),
        tool_use(
            "describe-call",
            "tool_describe",
            json!({"name": "task_create"}),
        ),
        tool_use(
            "task-call",
            "tool_call",
            json!({
                "name": "task_create",
                "arguments": {"title": "ship disclosure"}
            }),
        ),
        final_text("done"),
    ]));
    let captured_requests = model.requests();
    let hook = Arc::new(RecordingHook::new());
    let tool_ctx = rua_core::tool::ToolContext {
        working_dir: tmp.path().to_path_buf(),
        tool_result_store_dir: Some(tmp.path().join("tool_call_logs")),
        ..Default::default()
    };

    let agent = ReactAgent::builder(model)
        .toolkit(Arc::clone(&toolkit))
        .config(config)
        .hook(hook.clone())
        .tool_ctx(tool_ctx)
        .system_prompt("You are Rua.")
        .build();
    let events: Vec<AgentEvent> = agent
        .run_stream(vec![ModelDialogueItem::text(
            Role::User,
            "Find the task tool and create an issue.",
        )])
        .collect()
        .await;

    let first_request_names = {
        let requests = captured_requests.lock().unwrap();
        assert!(
            requests.len() >= 4,
            "bridge search, describe, call, and final answer should each re-enter the model"
        );
        tool_names(&requests[0])
    };
    assert!(first_request_names.contains(&"read".to_string()));
    assert!(first_request_names.contains(&"skill".to_string()));
    assert!(first_request_names.contains(&"tool_search".to_string()));
    assert!(first_request_names.contains(&"tool_describe".to_string()));
    assert!(first_request_names.contains(&"tool_call".to_string()));
    assert!(!first_request_names.contains(&"task_create".to_string()));

    let tool_results: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::ToolResult { result } => Some(result),
            _ => None,
        })
        .collect();
    let search_result = tool_results
        .iter()
        .find(|result| result.name.as_deref() == Some("tool_search"))
        .expect("tool_search result");
    assert!(search_result.text_content().contains("task_create"));
    assert!(
        !search_result.text_content().contains("\"properties\""),
        "search result should stay compact and omit full parameter schemas"
    );
    let describe_result = tool_results
        .iter()
        .find(|result| result.name.as_deref() == Some("tool_describe"))
        .expect("tool_describe result");
    assert!(describe_result.text_content().contains("\"properties\""));
    assert!(tool_results
        .iter()
        .any(|result| result.name.as_deref() == Some("task_create")
            && result.text_content().contains("task ok")));

    let hook_events = hook.events().await;
    assert!(hook_events.iter().any(|event| matches!(
        event,
        HookEvent::BeforeToolCall { tool_name, .. } if tool_name == "task_create"
    )));
    assert!(hook_events.iter().all(|event| !matches!(
        event,
        HookEvent::BeforeToolCall { tool_name, .. } if tool_name == "tool_call"
    )));

    let log_key = rua_tool::recall_key("task-call");
    let log_path = tmp
        .path()
        .join("tool_call_logs")
        .join(format!("{log_key}.json"));
    let log: serde_json::Value =
        serde_json::from_slice(&tokio::fs::read(log_path).await.expect("tool log"))
            .expect("tool log json");
    assert_eq!(log["tool_name"], "task_create");
    assert_eq!(log["input"], json!({"title": "ship disclosure"}));

    let running_on = rua_config::AgentsRunningConfig {
        tool_disclosure: rua_config::types::ToolDisclosureRunningConfig {
            enabled: rua_config::types::ToolDisclosureEnabled::On,
            ..Default::default()
        },
        ..Default::default()
    };
    let running_off = rua_config::AgentsRunningConfig {
        tool_disclosure: rua_config::types::ToolDisclosureRunningConfig {
            enabled: rua_config::types::ToolDisclosureEnabled::Off,
            ..Default::default()
        },
        ..Default::default()
    };
    let disclosure_accounting =
        rua_server::services::context_projection::build_system_context_accounting(
            vec!["You are Rua.".into()],
            skill_prompt_block.clone(),
            Some(toolkit.as_ref()),
            Some(&running_on),
            Some(1_000),
        );
    let direct_accounting =
        rua_server::services::context_projection::build_system_context_accounting(
            vec!["You are Rua.".into()],
            skill_prompt_block,
            Some(toolkit.as_ref()),
            Some(&running_off),
            Some(1_000),
        );
    assert_ne!(
        disclosure_accounting.system_context_hash,
        direct_accounting.system_context_hash
    );
    assert!(disclosure_accounting.disclosure_activated);
    assert!(disclosure_accounting.capability_index_tokens > 0);
    assert!(disclosure_accounting.visible_tool_schema_tokens > 0);
    assert!(disclosure_accounting.system_context_tokens > 0);

    let without_skill_prompt =
        rua_server::services::context_projection::build_system_context_accounting(
            vec!["You are Rua.".into()],
            None,
            Some(toolkit.as_ref()),
            Some(&running_on),
            Some(1_000),
        );
    assert_ne!(
        disclosure_accounting.system_context_hash,
        without_skill_prompt.system_context_hash
    );
    assert!(
        disclosure_accounting.system_context_tokens > without_skill_prompt.system_context_tokens
    );
}
