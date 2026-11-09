use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rua_core::ChatAddress;
use rua_server::services::conversation_authority::ConversationTurnTerminalFact;
use rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch;
use rua_server::services::product_hook_execution::HandlerExecutionResult;
use rua_server::services::product_hooks::{
    ChatConfigurationMaterialization, CreateUserHookDefinition, HookExecutionRequirement,
    HookHandlerView, ProductHookService,
};
use rua_server::services::terminal_observer::{TerminalObserverInvoker, TerminalObserverRuntime};
use tokio_util::sync::CancellationToken;

struct RecordingInvoker {
    calls: Mutex<Vec<String>>,
}

#[async_trait]
impl TerminalObserverInvoker for RecordingInvoker {
    async fn invoke(
        &self,
        requirement: &HookExecutionRequirement,
        _terminal: &ConversationTurnTerminalFact,
        _cancellation: CancellationToken,
    ) -> Result<HandlerExecutionResult, String> {
        self.calls
            .lock()
            .unwrap()
            .push(requirement.definition_revision.clone());
        if requirement.definition_revision.contains("first") {
            Err("observer failed".to_string())
        } else {
            Ok(HandlerExecutionResult {
                outcome: serde_json::json!({"observed": true}),
                diagnostics: Vec::new(),
                attempts: 1,
            })
        }
    }
}

async fn runtime() -> (
    TerminalObserverRuntime,
    Arc<RecordingInvoker>,
    ConversationTurnTerminalFact,
) {
    let hooks = Arc::new(ProductHookService::new());
    let agent_id = "terminal-agent";
    for logical_id in ["first", "second"] {
        let definition = hooks
            .create_user_definition(
                agent_id,
                CreateUserHookDefinition {
                    logical_id: logical_id.to_string(),
                    display_name: logical_id.to_string(),
                    event: "turn.terminal".to_string(),
                    capabilities: vec!["identity".to_string()],
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
        TerminalObserverRuntime::new(hooks, invoker.clone()),
        invoker,
        ConversationTurnTerminalFact {
            address,
            turn_id: "turn-a".to_string(),
            status: "completed".to_string(),
            completed_at: Some("2026-08-24T00:00:00Z".to_string()),
            authority_device_id: "device-a".to_string(),
            authority_epoch: ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
            device_delivery_id: None,
        },
    )
}

#[tokio::test]
async fn all_terminal_observers_run_and_failure_cannot_change_the_fact() {
    let (runtime, invoker, terminal) = runtime().await;
    let original = terminal.clone();

    let diagnostics = runtime.observe(&terminal, CancellationToken::new()).await;

    assert_eq!(terminal, original);
    assert_eq!(invoker.calls.lock().unwrap().len(), 2);
    assert_eq!(diagnostics[0].status, "failed");
    assert_eq!(diagnostics[1].status, "completed");
}

#[tokio::test]
async fn cancellation_cancels_hook_work_without_touching_terminal_fact() {
    let (runtime, invoker, terminal) = runtime().await;
    let original = terminal.clone();
    let cancellation = CancellationToken::new();
    cancellation.cancel();

    let diagnostics = runtime.observe(&terminal, cancellation).await;

    assert_eq!(terminal, original);
    assert!(invoker.calls.lock().unwrap().is_empty());
    assert!(diagnostics.iter().all(|item| item.status == "cancelled"));
}
