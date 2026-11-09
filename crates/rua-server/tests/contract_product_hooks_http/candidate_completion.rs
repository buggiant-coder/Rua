use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use rua_core::ChatAddress;
use rua_server::services::candidate_completion::{
    CandidateCompletion, CandidateCompletionInvoker, CandidateCompletionRuntime,
    CompletionDisposition, DriverCompletionCapability,
};
use rua_server::services::console_chat::{
    candidate_completion_capability, candidate_continuation_request,
};
use rua_server::services::conversation_authority::ConversationDriverKind;
use rua_server::services::product_hook_execution::HandlerExecutionResult;
use rua_server::services::product_hooks::{
    ChatConfigurationMaterialization, CreateUserHookDefinition, HookExecutionRequirement,
    HookHandlerView, ProductHookService,
};
use tokio_util::sync::CancellationToken;

struct RecordingInvoker {
    calls: Mutex<Vec<String>>,
    outcomes: Vec<Result<serde_json::Value, String>>,
}

#[async_trait]
impl CandidateCompletionInvoker for RecordingInvoker {
    async fn invoke(
        &self,
        requirement: &HookExecutionRequirement,
        _completion: &CandidateCompletion,
    ) -> Result<HandlerExecutionResult, String> {
        let mut calls = self.calls.lock().unwrap();
        let index = calls.len();
        calls.push(requirement.definition_revision.clone());
        let action = self.outcomes[index].clone()?;
        Ok(HandlerExecutionResult {
            outcome: action,
            diagnostics: Vec::new(),
            attempts: 1,
        })
    }
}

async fn frozen_runtime(
    outcomes: Vec<Result<serde_json::Value, String>>,
) -> (
    Arc<ProductHookService>,
    Arc<RecordingInvoker>,
    CandidateCompletionRuntime,
    ChatAddress,
) {
    let hooks = Arc::new(ProductHookService::new());
    let agent_id = "candidate-agent";
    for logical_id in ["first", "second"] {
        let definition = hooks
            .create_user_definition(
                agent_id,
                CreateUserHookDefinition {
                    logical_id: logical_id.to_string(),
                    display_name: logical_id.to_string(),
                    event: "turn.before_completion".to_string(),
                    capabilities: vec!["candidate_response".to_string()],
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
    let runtime = CandidateCompletionRuntime::new(hooks.clone(), invoker.clone());
    (hooks, invoker, runtime, address)
}

fn completion(
    address: &ChatAddress,
    driver_capability: DriverCompletionCapability,
) -> CandidateCompletion {
    CandidateCompletion {
        address: address.clone(),
        turn_id: "turn-a".to_string(),
        driver_capability,
        candidate: serde_json::json!({"response": "Done"}),
        cancellation: CancellationToken::new(),
        remaining_budget: Duration::from_secs(30),
    }
}

#[tokio::test]
async fn all_frozen_hooks_run_in_order_and_any_continuation_keeps_the_same_turn() {
    let (hooks, invoker, runtime, address) = frozen_runtime(vec![
        Ok(serde_json::json!({"action": "accept"})),
        Ok(serde_json::json!({
            "action": "continue",
            "context": {"instruction": "Verify the final answer."}
        })),
    ])
    .await;
    let chat_before = hooks.chat_configuration(&address).await;
    let turn_before = hooks.frozen_turn_configuration(&address, "turn-a").await;

    let result = runtime
        .evaluate(completion(
            &address,
            DriverCompletionCapability::Continuable,
        ))
        .await;

    let CompletionDisposition::Continue { contexts } = result else {
        panic!("expected Hook-authored continuation");
    };
    assert_eq!(invoker.calls.lock().unwrap().len(), 2);
    assert!(invoker.calls.lock().unwrap()[0].ends_with("/first@v1"));
    assert!(invoker.calls.lock().unwrap()[1].ends_with("/second@v1"));
    assert_eq!(contexts.len(), 1);
    assert_eq!(contexts[0].address, address);
    assert_eq!(contexts[0].turn_id, "turn-a");
    assert!(contexts[0].definition_revision.ends_with("/second@v1"));
    assert_eq!(contexts[0].source, "agent");
    assert_eq!(contexts[0].resolved_order, 1);
    assert_eq!(
        contexts[0].content,
        serde_json::json!({"instruction": "Verify the final answer."})
    );
    assert_eq!(hooks.chat_configuration(&address).await, chat_before);
    assert_eq!(
        hooks.frozen_turn_configuration(&address, "turn-a").await,
        turn_before
    );
}

#[tokio::test]
async fn accept_only_driver_rejects_continuation_capable_configuration_before_invocation() {
    let (_hooks, invoker, runtime, address) = frozen_runtime(vec![
        Ok(serde_json::json!({"action": "accept"})),
        Ok(serde_json::json!({"action": "accept"})),
    ])
    .await;

    let result = runtime
        .evaluate(completion(&address, DriverCompletionCapability::AcceptOnly))
        .await;

    let CompletionDisposition::Failed { diagnostics } = result else {
        panic!("expected incompatible Driver configuration failure");
    };
    assert!(diagnostics[0].contains("accept-only"));
    assert!(invoker.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn hook_failure_is_diagnostic_and_never_accept_or_continuation() {
    let (_hooks, invoker, runtime, address) = frozen_runtime(vec![
        Err("hook crashed".to_string()),
        Ok(serde_json::json!({
            "action": "continue",
            "context": {"instruction": "should not win"}
        })),
    ])
    .await;

    let result = runtime
        .evaluate(completion(
            &address,
            DriverCompletionCapability::Continuable,
        ))
        .await;

    let CompletionDisposition::Failed { diagnostics } = result else {
        panic!("expected Hook failure");
    };
    assert_eq!(invoker.calls.lock().unwrap().len(), 2);
    assert_eq!(diagnostics.len(), 1);
    assert!(diagnostics[0].contains("hook crashed"));
}

#[tokio::test]
async fn cancellation_and_exhausted_budget_override_hooks_without_invocation() {
    for exhausted in [false, true] {
        let (_hooks, invoker, runtime, address) = frozen_runtime(vec![
            Ok(serde_json::json!({"action": "accept"})),
            Ok(serde_json::json!({"action": "accept"})),
        ])
        .await;
        let mut candidate = completion(&address, DriverCompletionCapability::Continuable);
        if exhausted {
            candidate.remaining_budget = Duration::ZERO;
        } else {
            candidate.cancellation.cancel();
        }

        let result = runtime.evaluate(candidate).await;

        let CompletionDisposition::Failed { diagnostics } = result else {
            panic!("expected authority override failure");
        };
        assert_eq!(diagnostics.len(), 1);
        assert!(invoker.calls.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn all_accept_outcomes_accept_candidate_completion() {
    let (_hooks, invoker, runtime, address) = frozen_runtime(vec![
        Ok(serde_json::json!({"action": "accept"})),
        Ok(serde_json::json!({"action": "accept"})),
    ])
    .await;

    let result = runtime
        .evaluate(completion(
            &address,
            DriverCompletionCapability::Continuable,
        ))
        .await;

    assert_eq!(result, CompletionDisposition::Accept);
    assert_eq!(invoker.calls.lock().unwrap().len(), 2);
}

#[test]
fn conversation_drivers_declare_candidate_completion_capability() {
    assert_eq!(
        candidate_completion_capability(ConversationDriverKind::Rua),
        DriverCompletionCapability::Continuable
    );
    assert_eq!(
        candidate_completion_capability(ConversationDriverKind::Acp),
        DriverCompletionCapability::AcceptOnly
    );
}

#[test]
fn continuation_request_reenters_the_same_turn_without_user_input_or_new_epoch() {
    let context = rua_server::services::candidate_completion::HookContinuationContext {
        address: ChatAddress::agent("candidate-agent", "chat-a"),
        turn_id: "turn-a".to_string(),
        definition_revision: "agent:candidate-agent/review@v1".to_string(),
        source: "agent".to_string(),
        resolved_order: 0,
        content: serde_json::json!({"instruction": "Review the answer."}),
    };
    let mut original = rua_server::endpoints::dto::ChatRequest {
        message: "original user request".to_string(),
        chat_id: Some("chat-a".to_string()),
        input: Some(vec![serde_json::json!({"role": "user"})]),
        messages: Some(vec![serde_json::json!({"role": "user"})]),
        user_id: None,
        channel: Some("console".to_string()),
        stream: Some(true),
        model: None,
        attachments: vec![rua_server::endpoints::dto::AttachmentDto {
            url: "file".to_string(),
            name: "file".to_string(),
            mime_type: None,
            stored_name: None,
            size: None,
            extra: serde_json::Map::new(),
        }],
        extra: serde_json::Map::new(),
    };
    original
        .extra
        .insert("turn_id".to_string(), serde_json::json!("turn-a"));
    original.extra.insert(
        rua_server::services::product_hooks::CHAT_HOOK_CONFIGURATION_META_KEY.to_string(),
        serde_json::json!({"epoch": "hook-config-a"}),
    );

    let continuation = candidate_continuation_request(&original, vec![context.clone()]);

    assert_eq!(continuation.chat_id, original.chat_id);
    assert_eq!(continuation.extra["turn_id"], "turn-a");
    assert_eq!(
        continuation
            .extra
            .get(rua_server::services::product_hooks::CHAT_HOOK_CONFIGURATION_META_KEY),
        original
            .extra
            .get(rua_server::services::product_hooks::CHAT_HOOK_CONFIGURATION_META_KEY)
    );
    assert_eq!(
        continuation.extra[rua_agent::runtime::HOOK_CONTINUATION_CONTEXT_METADATA_KEY],
        serde_json::to_value(vec![context]).unwrap()
    );
    assert!(continuation.message.is_empty());
    assert!(continuation.input.is_none());
    assert!(continuation.messages.is_none());
    assert!(continuation.attachments.is_empty());
}
