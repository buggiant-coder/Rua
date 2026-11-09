use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use react_agent::{
    PermissionHookDecision, PermissionRequestBoundary, PermissionRequestInvocation,
    ToolBeforeInvocation,
};
use rua_core::ChatAddress;
use rua_server::services::product_hook_execution::HandlerExecutionResult;
use rua_server::services::product_hooks::{
    ChatConfigurationMaterialization, CreateUserHookDefinition, HookExecutionRequirement,
    HookHandlerView, ProductHookService,
};
use rua_server::services::product_permission_request::{
    ProductPermissionInvoker, ProductPermissionRequestBoundary,
};
use tokio_util::sync::CancellationToken;

struct RecordingInvoker {
    calls: Mutex<Vec<String>>,
    outcomes: Vec<Result<serde_json::Value, String>>,
}

#[async_trait]
impl ProductPermissionInvoker for RecordingInvoker {
    async fn invoke(
        &self,
        requirement: &HookExecutionRequirement,
        _invocation: &ToolBeforeInvocation,
        _cancellation: CancellationToken,
    ) -> Result<HandlerExecutionResult, String> {
        let index = self.calls.lock().unwrap().len();
        self.calls
            .lock()
            .unwrap()
            .push(requirement.definition_revision.clone());
        self.outcomes[index]
            .clone()
            .map(|outcome| HandlerExecutionResult {
                outcome,
                diagnostics: Vec::new(),
                attempts: 1,
            })
    }
}

async fn boundary(
    outcomes: Vec<Result<serde_json::Value, String>>,
) -> (
    ProductPermissionRequestBoundary,
    Arc<ProductHookService>,
    Arc<RecordingInvoker>,
    ChatAddress,
) {
    let hooks = Arc::new(ProductHookService::new());
    let agent_id = "permission-agent";
    for logical_id in ["first", "second", "third"] {
        let definition = hooks
            .create_user_definition(
                agent_id,
                CreateUserHookDefinition {
                    logical_id: logical_id.to_string(),
                    display_name: logical_id.to_string(),
                    event: "permission.request".to_string(),
                    capabilities: vec!["permission_request".to_string()],
                    handler: HookHandlerView {
                        kind: "command".to_string(),
                        executable: None,
                        program: Some("/usr/bin/hook-runtime".to_string()),
                        args: vec![logical_id.to_string()],
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
    (
        ProductPermissionRequestBoundary::new(hooks.clone(), invoker.clone()),
        hooks,
        invoker,
        address,
    )
}

fn invocation(address: &ChatAddress) -> PermissionRequestInvocation {
    let call = agent_types::ToolCall::new("call-a", "echo", serde_json::json!({"value": "final"}));
    PermissionRequestInvocation {
        request_id: "request-a".to_string(),
        prompt: "approval required".to_string(),
        argument_digest: react_agent::argument_digest(&call),
        call,
        agent_id: "permission-agent".to_string(),
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
async fn all_handlers_run_and_deny_outranks_allow_and_failure() {
    let (boundary, hooks, invoker, address) = boundary(vec![
        Ok(serde_json::json!({"decision": "allow"})),
        Err("handler failed".to_string()),
        Ok(serde_json::json!({"decision": "deny", "reason": "blocked"})),
    ])
    .await;
    let expected_digest = invocation(&address).argument_digest;

    let decision = boundary
        .decide(invocation(&address), CancellationToken::new())
        .await
        .unwrap();

    assert_eq!(
        decision,
        PermissionHookDecision::Deny {
            reason: "blocked".to_string()
        }
    );
    assert_eq!(invoker.calls.lock().unwrap().len(), 3);
    let stored = hooks.contributions(&address, "turn-a").await;
    assert_eq!(stored.len(), 3);
    assert!(stored
        .iter()
        .any(|item| item.diagnostic.as_deref() == Some("handler failed")));
    assert!(stored
        .iter()
        .all(|item| item.content["argument_digest"] == expected_digest));
}

#[tokio::test]
async fn failures_and_abstentions_aggregate_to_no_decision() {
    let (boundary, _, invoker, address) = boundary(vec![
        Err("first failed".to_string()),
        Ok(serde_json::json!({"decision": "no_decision"})),
        Err("third failed".to_string()),
    ])
    .await;

    let decision = boundary
        .decide(invocation(&address), CancellationToken::new())
        .await
        .unwrap();

    assert_eq!(decision, PermissionHookDecision::NoDecision);
    assert_eq!(invoker.calls.lock().unwrap().len(), 3);
}
