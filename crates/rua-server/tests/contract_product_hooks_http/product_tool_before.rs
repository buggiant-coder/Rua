use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use react_agent::{ToolBeforeBoundary, ToolBeforeInvocation, ToolBeforeOutcome};
use rua_core::ChatAddress;
use rua_server::services::product_hook_execution::HandlerExecutionResult;
use rua_server::services::product_hooks::{
    ChatConfigurationMaterialization, CreateUserHookDefinition, HookExecutionRequirement,
    HookHandlerView, ProductHookService,
};
use rua_server::services::product_tool_before::{
    ProductToolBeforeBoundary, ProductToolBeforeInvoker,
};
use tokio_util::sync::CancellationToken;

struct RecordingInvoker {
    calls: Mutex<Vec<(String, serde_json::Value)>>,
    outcomes: Vec<serde_json::Value>,
}

#[async_trait]
impl ProductToolBeforeInvoker for RecordingInvoker {
    async fn invoke(
        &self,
        requirement: &HookExecutionRequirement,
        invocation: &ToolBeforeInvocation,
        _cancellation: CancellationToken,
    ) -> Result<HandlerExecutionResult, String> {
        let index = self.calls.lock().unwrap().len();
        self.calls.lock().unwrap().push((
            requirement.definition_revision.clone(),
            invocation.call.arguments.clone(),
        ));
        Ok(HandlerExecutionResult {
            outcome: self.outcomes[index].clone(),
            diagnostics: Vec::new(),
            attempts: 1,
        })
    }
}

async fn frozen_boundary(
    outcomes: Vec<serde_json::Value>,
) -> (
    Arc<RecordingInvoker>,
    ProductToolBeforeBoundary,
    ChatAddress,
) {
    let hooks = Arc::new(ProductHookService::new());
    let agent_id = "tool-before-agent";
    for (logical_id, executable) in [("first", "first"), ("second", "second")] {
        let definition = hooks
            .create_user_definition(
                agent_id,
                CreateUserHookDefinition {
                    logical_id: logical_id.to_string(),
                    display_name: logical_id.to_string(),
                    event: "tool.before".to_string(),
                    capabilities: vec!["tool_arguments".to_string()],
                    handler: HookHandlerView {
                        kind: "command".to_string(),
                        executable: None,
                        program: Some("/usr/bin/hook-runtime".to_string()),
                        args: vec![executable.to_string()],
                        available: true,
                        working_directory: Some("/workspace".to_string()),
                        timeout_seconds: Some(30),
                        required_resources: Vec::new(),
                    },
                    owner_namespace: None,
                    source: None,
                },
            )
            .await
            .unwrap();
        hooks
            .set_agent_activation(agent_id, &definition.id, true)
            .await
            .unwrap();
    }
    let address = ChatAddress::agent(agent_id, "chat-a");
    let configuration = hooks
        .resolve_complete_chat_configuration(
            &address,
            ChatConfigurationMaterialization {
                selected_agent_id: agent_id.to_string(),
                running_config: Default::default(),
                security_config: Default::default(),
                agent_assignment: None,
                prompt_file_content: None,
                tool_config_snapshot: Default::default(),
                capability_configuration: None,
                model: None,
                thinking: None,
                collaboration_mode: serde_json::Value::Null,
                harness_configuration: None,
                authority_device_id: "device-a".to_string(),
            },
        )
        .await
        .unwrap();
    hooks
        .adopt_chat_configuration(&address, configuration)
        .await;
    hooks.freeze_turn(&address, "turn-a").await;
    let invoker = Arc::new(RecordingInvoker {
        calls: Mutex::new(Vec::new()),
        outcomes,
    });
    let boundary = ProductToolBeforeBoundary::new(hooks, invoker.clone());
    (invoker, boundary, address)
}

fn invocation(address: &ChatAddress) -> ToolBeforeInvocation {
    ToolBeforeInvocation {
        call: agent_types::ToolCall::new(
            "call-a",
            "echo",
            serde_json::json!({"value": "original"}),
        ),
        schema: serde_json::json!({
            "type": "object",
            "properties": {"value": {"type": "string"}},
            "required": ["value"],
            "additionalProperties": false
        }),
        agent_id: "tool-before-agent".to_string(),
        session_id: "chat-a".to_string(),
        working_directory: "/workspace".into(),
        metadata: std::collections::HashMap::from([
            (
                rua_agent::hooks::SECURITY_CHAT_ADDRESS_METADATA_KEY.to_string(),
                serde_json::to_value(address).unwrap(),
            ),
            ("turn_id".to_string(), serde_json::json!("turn-a")),
        ]),
    }
}

#[tokio::test]
async fn resolved_mutators_run_serially_over_each_validated_candidate() {
    let (invoker, boundary, address) = frozen_boundary(vec![
        serde_json::json!({"action": "continue", "arguments": {"value": "first"}}),
        serde_json::json!({"action": "continue", "arguments": {"value": "second"}}),
    ])
    .await;

    let result = boundary
        .apply(invocation(&address), CancellationToken::new())
        .await
        .unwrap();

    let ToolBeforeOutcome::Continue(call) = result else {
        panic!("expected mutation to continue");
    };
    assert_eq!(call.arguments, serde_json::json!({"value": "second"}));
    let calls = invoker.calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert!(calls[0].0.ends_with("/first@v1"));
    assert_eq!(calls[0].1, serde_json::json!({"value": "original"}));
    assert!(calls[1].0.ends_with("/second@v1"));
    assert_eq!(calls[1].1, serde_json::json!({"value": "first"}));
}

#[tokio::test]
async fn invalid_candidate_stops_before_the_next_mutator() {
    let (invoker, boundary, address) = frozen_boundary(vec![
        serde_json::json!({"action": "continue", "arguments": {"value": 42}}),
        serde_json::json!({"action": "continue", "arguments": {"value": "second"}}),
    ])
    .await;

    let failure = boundary
        .apply(invocation(&address), CancellationToken::new())
        .await
        .unwrap_err();

    assert!(failure.diagnostic.contains("invalid Tool arguments"));
    assert_eq!(invoker.calls.lock().unwrap().len(), 1);
}
