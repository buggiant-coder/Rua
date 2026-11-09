use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use react_agent::{ToolObservationEvent, ToolObservationInvocation, ToolObserverBoundary};
use rua_core::ChatAddress;
use rua_server::services::product_hook_execution::HandlerExecutionResult;
use rua_server::services::product_hooks::{
    ChatConfigurationMaterialization, CreateUserHookDefinition, HookExecutionRequirement,
    HookHandlerView, ProductHookService,
};
use rua_server::services::product_tool_observer::{
    ProductToolObserverBoundaryImpl, ProductToolObserverInvoker,
};
use tokio_util::sync::CancellationToken;

struct RecordingInvoker {
    calls: Mutex<Vec<String>>,
}

#[async_trait]
impl ProductToolObserverInvoker for RecordingInvoker {
    async fn invoke(
        &self,
        requirement: &HookExecutionRequirement,
        _invocation: &ToolObservationInvocation,
        _cancellation: CancellationToken,
    ) -> Result<HandlerExecutionResult, String> {
        self.calls
            .lock()
            .unwrap()
            .push(requirement.definition_revision.clone());
        if requirement.definition_revision.contains("first") {
            return Err("observer failed".to_string());
        }
        Ok(HandlerExecutionResult {
            outcome: serde_json::json!({
                "replace_result": {"forbidden": true},
                "contributions": [
                    {"kind": "feedback", "content": {"rating": "good"}},
                    {"kind": "context", "content": {"note": "verified"}},
                    {"kind": "result", "content": {"forbidden": true}}
                ]
            }),
            diagnostics: Vec::new(),
            attempts: 1,
        })
    }
}

async fn boundary_for(
    event: &str,
) -> (
    ProductToolObserverBoundaryImpl,
    Arc<ProductHookService>,
    ChatAddress,
    Arc<RecordingInvoker>,
) {
    let hooks = Arc::new(ProductHookService::new());
    let agent_id = "observer-agent";
    for logical_id in ["first", "second"] {
        let definition = hooks
            .create_user_definition(
                agent_id,
                CreateUserHookDefinition {
                    logical_id: logical_id.to_string(),
                    display_name: logical_id.to_string(),
                    event: event.to_string(),
                    capabilities: vec!["context".to_string()],
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
    });
    (
        ProductToolObserverBoundaryImpl::new(hooks.clone(), invoker.clone()),
        hooks,
        address,
        invoker,
    )
}

fn invocation(address: &ChatAddress, event: ToolObservationEvent) -> ToolObservationInvocation {
    ToolObservationInvocation {
        event,
        call: agent_types::ToolCall::new("call-a", "echo", serde_json::json!({})),
        result: if event == ToolObservationEvent::Failed {
            agent_types::ToolResult::error("call-a", "canonical failure")
        } else {
            agent_types::ToolResult::text("call-a", "canonical success")
        },
        agent_id: "observer-agent".to_string(),
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
async fn after_observers_all_run_and_store_only_typed_contributions() {
    let (boundary, hooks, address, invoker) = boundary_for("tool.after").await;
    let contributions = boundary
        .observe(
            invocation(&address, ToolObservationEvent::After),
            CancellationToken::new(),
        )
        .await
        .unwrap();

    assert_eq!(invoker.calls.lock().unwrap().len(), 2);
    assert_eq!(contributions.len(), 2);
    assert_eq!(contributions[0].kind, "feedback");
    assert_eq!(contributions[1].kind, "context");
    let stored = hooks.contributions(&address, "turn-a").await;
    assert_eq!(stored.len(), 3);
    assert!(stored
        .iter()
        .any(|item| item.diagnostic.as_deref() == Some("observer failed")));
    assert!(!stored.iter().any(|item| item.kind == "result"));
}

#[tokio::test]
async fn failed_observers_receive_failure_without_changing_it() {
    let (boundary, hooks, address, invoker) = boundary_for("tool.failed").await;
    let contributions = boundary
        .observe(
            invocation(&address, ToolObservationEvent::Failed),
            CancellationToken::new(),
        )
        .await
        .unwrap();

    assert_eq!(invoker.calls.lock().unwrap().len(), 2);
    assert_eq!(contributions.len(), 2);
    assert!(hooks
        .contributions(&address, "turn-a")
        .await
        .iter()
        .all(|item| item.event == "tool.failed"));
}
