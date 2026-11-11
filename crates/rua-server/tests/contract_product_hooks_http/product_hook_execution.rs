use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use rua_core::{ChatAddress, ChatScope};
#[cfg(unix)]
use rua_server::services::product_hook_execution::{
    CommandExecutionResource, StrictCommandHandler,
};
use rua_server::services::product_hook_execution::{
    CommandHandler, HandlerAdapterError, HandlerAdapterInvocation, HandlerAdapterResult,
    HandlerExecutionFailureKind, HandlerExecutionResult, HandlerExecutor, HandlerExposure,
    HandlerInvocation, HandlerKind, NativeHandler, PluginHandler,
};
use serde_json::json;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct RecordingNativeHandler {
    invocations: Mutex<Vec<HandlerAdapterInvocation>>,
}

#[async_trait]
impl NativeHandler for RecordingNativeHandler {
    async fn execute(
        &self,
        invocation: HandlerAdapterInvocation,
    ) -> Result<HandlerAdapterResult, HandlerAdapterError> {
        self.invocations.lock().unwrap().push(invocation);
        Ok(HandlerAdapterResult {
            outcome: json!({"decision": "continue"}),
            diagnostics: Vec::new(),
        })
    }
}

struct UnusedPluginHandler;

#[derive(Default)]
struct RecordingPluginHandler {
    invocations: Mutex<Vec<(String, HandlerAdapterInvocation)>>,
}

#[async_trait]
impl PluginHandler for RecordingPluginHandler {
    async fn execute(
        &self,
        plugin_id: &str,
        invocation: HandlerAdapterInvocation,
    ) -> Result<HandlerAdapterResult, HandlerAdapterError> {
        self.invocations
            .lock()
            .unwrap()
            .push((plugin_id.to_string(), invocation));
        Ok(HandlerAdapterResult {
            outcome: json!({"decision": "allow"}),
            diagnostics: Vec::new(),
        })
    }
}

struct UnusedCommandHandler;

#[async_trait]
impl CommandHandler for UnusedCommandHandler {
    async fn execute(
        &self,
        _invocation: HandlerAdapterInvocation,
    ) -> Result<HandlerAdapterResult, HandlerAdapterError> {
        panic!("command adapter must not run for a native Handler")
    }
}

#[async_trait]
impl PluginHandler for UnusedPluginHandler {
    async fn execute(
        &self,
        _plugin_id: &str,
        _invocation: HandlerAdapterInvocation,
    ) -> Result<HandlerAdapterResult, HandlerAdapterError> {
        panic!("plugin adapter must not run for a native Handler")
    }
}

struct DelayedNativeHandler {
    calls: AtomicUsize,
    delay: Duration,
}

struct FailingNativeHandler {
    error: HandlerAdapterError,
}

#[async_trait]
impl NativeHandler for FailingNativeHandler {
    async fn execute(
        &self,
        _invocation: HandlerAdapterInvocation,
    ) -> Result<HandlerAdapterResult, HandlerAdapterError> {
        Err(self.error.clone())
    }
}

struct DiagnosticNativeHandler;

#[async_trait]
impl NativeHandler for DiagnosticNativeHandler {
    async fn execute(
        &self,
        _invocation: HandlerAdapterInvocation,
    ) -> Result<HandlerAdapterResult, HandlerAdapterError> {
        Ok(HandlerAdapterResult {
            outcome: json!({"decision": "continue"}),
            diagnostics: vec![
                "token=super-sensitive-value bearer abcdefghijklmnopqrstuvwxyz".to_string(),
            ],
        })
    }
}

fn native_invocation(exposure: HandlerExposure) -> HandlerInvocation {
    HandlerInvocation {
        invocation_id: "hook-invocation-failure".to_string(),
        kind: HandlerKind::Native,
        authority_device_id: "device-authority".to_string(),
        chat_address: ChatAddress::agent("agent-a", "chat-a"),
        scope: ChatScope::agent("agent-a"),
        resource_id: "agent:agent-a".to_string(),
        executable: Some("native_handler".to_string()),
        program: None,
        args: Vec::new(),
        working_directory: PathBuf::from("/agent/workspace"),
        timeout: Duration::from_secs(5),
        exposure,
        cancellation: CancellationToken::new(),
        remaining_budget: Duration::from_secs(30),
        available_resources: Vec::new(),
        required_resources: Vec::new(),
    }
}

#[async_trait]
impl NativeHandler for DelayedNativeHandler {
    async fn execute(
        &self,
        _invocation: HandlerAdapterInvocation,
    ) -> Result<HandlerAdapterResult, HandlerAdapterError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(self.delay).await;
        Ok(HandlerAdapterResult {
            outcome: json!({"decision": "continue"}),
            diagnostics: Vec::new(),
        })
    }
}

#[tokio::test]
async fn native_handler_executes_once_with_the_declared_context() {
    let native = Arc::new(RecordingNativeHandler::default());
    let executor = HandlerExecutor::new(
        Arc::new(UnusedCommandHandler),
        native.clone(),
        Arc::new(UnusedPluginHandler),
    );
    let cancel = CancellationToken::new();
    let invocation = HandlerInvocation {
        invocation_id: "hook-invocation-native".to_string(),
        kind: HandlerKind::Native,
        authority_device_id: "device-authority".to_string(),
        chat_address: ChatAddress::agent("agent-a", "chat-a"),
        scope: ChatScope::agent("agent-a"),
        resource_id: "agent:agent-a".to_string(),
        executable: Some("turn_terminal_audit".to_string()),
        program: None,
        args: Vec::new(),
        working_directory: PathBuf::from("/agent/workspace"),
        timeout: Duration::from_secs(5),
        exposure: HandlerExposure {
            data: json!({"turn_id": "turn-1"}),
            capabilities: vec!["identity".to_string()],
            sensitive_values: Vec::new(),
        },
        cancellation: cancel.clone(),
        remaining_budget: Duration::from_secs(30),
        available_resources: Vec::new(),
        required_resources: Vec::new(),
    };

    let result = executor.execute(invocation.clone()).await.unwrap();

    assert_eq!(
        result,
        HandlerExecutionResult {
            outcome: json!({"decision": "continue"}),
            diagnostics: Vec::new(),
            attempts: 1,
        }
    );
    let recorded = native.invocations.lock().unwrap();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].invocation_id, invocation.invocation_id);
    assert_eq!(
        recorded[0].authority_device_id,
        invocation.authority_device_id
    );
    assert_eq!(recorded[0].scope, invocation.scope);
    assert_eq!(recorded[0].resource_id, invocation.resource_id);
    assert_eq!(recorded[0].executable, invocation.executable);
    assert_eq!(recorded[0].program, invocation.program);
    assert_eq!(recorded[0].args, invocation.args);
    assert_eq!(recorded[0].working_directory, invocation.working_directory);
    assert_eq!(recorded[0].timeout, invocation.timeout);
    assert_eq!(recorded[0].exposure, invocation.exposure);
    assert_eq!(recorded[0].remaining_budget, invocation.remaining_budget);
    assert_eq!(
        recorded[0].available_resources,
        invocation.available_resources
    );
    assert_eq!(
        recorded[0].required_resources,
        invocation.required_resources
    );
    assert!(!recorded[0].cancellation.is_cancelled());
    assert!(!cancel.is_cancelled());
}

#[tokio::test]
async fn installed_plugin_handler_executes_once_with_its_explicit_plugin_identity() {
    let plugin = Arc::new(RecordingPluginHandler::default());
    let executor = HandlerExecutor::new(
        Arc::new(UnusedCommandHandler),
        Arc::new(RecordingNativeHandler::default()),
        plugin.clone(),
    );
    let invocation = HandlerInvocation {
        invocation_id: "hook-invocation-plugin".to_string(),
        kind: HandlerKind::Plugin {
            plugin_id: "installed-plugin".to_string(),
        },
        authority_device_id: "device-authority".to_string(),
        chat_address: ChatAddress::project("project-a", "chat-a"),
        scope: ChatScope::project("project-a"),
        resource_id: "project:project-a".to_string(),
        executable: Some("before_tool".to_string()),
        program: None,
        args: Vec::new(),
        working_directory: PathBuf::from("/project/root"),
        timeout: Duration::from_secs(5),
        exposure: HandlerExposure {
            data: json!({"tool": "read_file"}),
            capabilities: vec!["tool_arguments".to_string()],
            sensitive_values: Vec::new(),
        },
        cancellation: CancellationToken::new(),
        remaining_budget: Duration::from_secs(30),
        available_resources: vec!["plugin:installed-plugin".to_string()],
        required_resources: vec!["plugin:installed-plugin".to_string()],
    };

    let result = executor.execute(invocation).await.unwrap();

    assert_eq!(result.outcome, json!({"decision": "allow"}));
    assert_eq!(result.attempts, 1);
    let recorded = plugin.invocations.lock().unwrap();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].0, "installed-plugin");
    assert_eq!(recorded[0].1.executable.as_deref(), Some("before_tool"));
    assert_eq!(recorded[0].1.scope, ChatScope::project("project-a"));
}

#[tokio::test]
async fn unavailable_plugin_requirement_fails_without_invoking_the_adapter() {
    let plugin = Arc::new(RecordingPluginHandler::default());
    let executor = HandlerExecutor::new(
        Arc::new(UnusedCommandHandler),
        Arc::new(RecordingNativeHandler::default()),
        plugin.clone(),
    );
    let invocation = HandlerInvocation {
        invocation_id: "hook-invocation-unavailable".to_string(),
        kind: HandlerKind::Plugin {
            plugin_id: "installed-plugin".to_string(),
        },
        authority_device_id: "device-authority".to_string(),
        chat_address: ChatAddress::project("project-a", "chat-a"),
        scope: ChatScope::project("project-a"),
        resource_id: "project:project-a".to_string(),
        executable: Some("before_tool".to_string()),
        program: None,
        args: Vec::new(),
        working_directory: PathBuf::from("/project/root"),
        timeout: Duration::from_secs(5),
        exposure: HandlerExposure {
            data: json!({}),
            capabilities: Vec::new(),
            sensitive_values: Vec::new(),
        },
        cancellation: CancellationToken::new(),
        remaining_budget: Duration::from_secs(30),
        available_resources: Vec::new(),
        required_resources: vec!["plugin:installed-plugin".to_string()],
    };

    let failure = executor.execute(invocation).await.unwrap_err();

    assert_eq!(
        failure.kind,
        HandlerExecutionFailureKind::UnavailableRequirements
    );
    assert_eq!(failure.attempts, 0);
    assert!(failure.diagnostic.contains("plugin:installed-plugin"));
    assert!(plugin.invocations.lock().unwrap().is_empty());
}

#[tokio::test]
#[cfg(unix)]
async fn command_handler_uses_exact_program_args_and_agent_resource_in_a_project_chat() {
    let temp = tempfile::tempdir().unwrap();
    let agent_root = temp.path().join("agent-root");
    let working_directory = agent_root.join("hooks");
    std::fs::create_dir_all(&working_directory).unwrap();
    let executable = working_directory.join("echo-invocation.sh");
    std::fs::write(
        &executable,
        "#!/bin/sh\npayload=$(cat)\nprintf '{\"outcome\":{\"received\":%s},\"diagnostics\":[]}' \"$payload\"\n",
    )
    .unwrap();
    let command = Arc::new(StrictCommandHandler::new(
        "device-authority",
        vec![CommandExecutionResource {
            scope: ChatScope::agent("agent-a"),
            resource_id: "agent:agent-a".to_string(),
            root: agent_root.clone(),
        }],
    ));
    let executor = HandlerExecutor::new(
        command,
        Arc::new(RecordingNativeHandler::default()),
        Arc::new(UnusedPluginHandler),
    );
    let invocation = HandlerInvocation {
        invocation_id: "hook-invocation-command".to_string(),
        kind: HandlerKind::Command,
        authority_device_id: "device-authority".to_string(),
        chat_address: ChatAddress::project("project-a", "chat-a"),
        scope: ChatScope::agent("agent-a"),
        resource_id: "agent:agent-a".to_string(),
        executable: None,
        program: Some("/bin/sh".to_string()),
        args: vec![executable.to_string_lossy().into_owned()],
        working_directory: working_directory.clone(),
        timeout: Duration::from_secs(5),
        exposure: HandlerExposure {
            data: json!({"turn_id": "turn-1"}),
            capabilities: vec!["identity".to_string()],
            sensitive_values: Vec::new(),
        },
        cancellation: CancellationToken::new(),
        remaining_budget: Duration::from_secs(30),
        available_resources: Vec::new(),
        required_resources: Vec::new(),
    };

    let result = executor.execute(invocation).await.unwrap();
    let received = &result.outcome["received"];

    assert_eq!(received["authority_device_id"], "device-authority");
    assert_eq!(received["chat_address"]["scope"]["kind"], "project");
    assert_eq!(received["scope"]["kind"], "agent");
    assert_eq!(received["resource_id"], "agent:agent-a");
    assert_eq!(received["program"], "/bin/sh");
    assert_eq!(received["args"], json!([executable.to_string_lossy()]));
    assert_eq!(
        received["working_directory"],
        working_directory.to_string_lossy().as_ref()
    );
    assert_eq!(received["exposure"]["data"]["turn_id"], "turn-1");
    assert_eq!(received["exposure"]["capabilities"], json!(["identity"]));
}

#[tokio::test]
async fn handler_timeout_is_structured_and_never_retried() {
    let native = Arc::new(DelayedNativeHandler {
        calls: AtomicUsize::new(0),
        delay: Duration::from_millis(200),
    });
    let executor = HandlerExecutor::new(
        Arc::new(UnusedCommandHandler),
        native.clone(),
        Arc::new(UnusedPluginHandler),
    );
    let invocation = HandlerInvocation {
        invocation_id: "hook-invocation-timeout".to_string(),
        kind: HandlerKind::Native,
        authority_device_id: "device-authority".to_string(),
        chat_address: ChatAddress::agent("agent-a", "chat-a"),
        scope: ChatScope::agent("agent-a"),
        resource_id: "agent:agent-a".to_string(),
        executable: Some("slow_native".to_string()),
        program: None,
        args: Vec::new(),
        working_directory: PathBuf::from("/agent/workspace"),
        timeout: Duration::from_millis(20),
        exposure: HandlerExposure {
            data: json!({}),
            capabilities: Vec::new(),
            sensitive_values: Vec::new(),
        },
        cancellation: CancellationToken::new(),
        remaining_budget: Duration::from_secs(1),
        available_resources: Vec::new(),
        required_resources: Vec::new(),
    };

    let failure = executor.execute(invocation).await.unwrap_err();

    assert_eq!(failure.kind, HandlerExecutionFailureKind::Timeout);
    assert_eq!(failure.attempts, 1);
    assert_eq!(native.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn cancellation_interrupts_in_flight_handler_without_retry() {
    let native = Arc::new(DelayedNativeHandler {
        calls: AtomicUsize::new(0),
        delay: Duration::from_secs(1),
    });
    let executor = HandlerExecutor::new(
        Arc::new(UnusedCommandHandler),
        native.clone(),
        Arc::new(UnusedPluginHandler),
    );
    let cancellation = CancellationToken::new();
    let cancel = cancellation.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(20)).await;
        cancel.cancel();
    });
    let invocation = HandlerInvocation {
        invocation_id: "hook-invocation-cancelled".to_string(),
        kind: HandlerKind::Native,
        authority_device_id: "device-authority".to_string(),
        chat_address: ChatAddress::agent("agent-a", "chat-a"),
        scope: ChatScope::agent("agent-a"),
        resource_id: "agent:agent-a".to_string(),
        executable: Some("slow_native".to_string()),
        program: None,
        args: Vec::new(),
        working_directory: PathBuf::from("/agent/workspace"),
        timeout: Duration::from_secs(5),
        exposure: HandlerExposure {
            data: json!({}),
            capabilities: Vec::new(),
            sensitive_values: Vec::new(),
        },
        cancellation,
        remaining_budget: Duration::from_secs(30),
        available_resources: Vec::new(),
        required_resources: Vec::new(),
    };

    let failure = executor.execute(invocation).await.unwrap_err();

    assert_eq!(failure.kind, HandlerExecutionFailureKind::Cancelled);
    assert_eq!(failure.attempts, 1);
    assert_eq!(native.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn zero_remaining_turn_budget_fails_before_handler_execution() {
    let native = Arc::new(DelayedNativeHandler {
        calls: AtomicUsize::new(0),
        delay: Duration::ZERO,
    });
    let executor = HandlerExecutor::new(
        Arc::new(UnusedCommandHandler),
        native.clone(),
        Arc::new(UnusedPluginHandler),
    );
    let invocation = HandlerInvocation {
        invocation_id: "hook-invocation-budget-empty".to_string(),
        kind: HandlerKind::Native,
        authority_device_id: "device-authority".to_string(),
        chat_address: ChatAddress::agent("agent-a", "chat-a"),
        scope: ChatScope::agent("agent-a"),
        resource_id: "agent:agent-a".to_string(),
        executable: Some("native".to_string()),
        program: None,
        args: Vec::new(),
        working_directory: PathBuf::from("/agent/workspace"),
        timeout: Duration::from_secs(5),
        exposure: HandlerExposure {
            data: json!({}),
            capabilities: Vec::new(),
            sensitive_values: Vec::new(),
        },
        cancellation: CancellationToken::new(),
        remaining_budget: Duration::ZERO,
        available_resources: Vec::new(),
        required_resources: Vec::new(),
    };

    let failure = executor.execute(invocation).await.unwrap_err();

    assert_eq!(failure.kind, HandlerExecutionFailureKind::BudgetExhausted);
    assert_eq!(failure.attempts, 0);
    assert_eq!(native.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn launch_and_malformed_outcome_are_distinct_structured_failures() {
    let cases = [
        (
            HandlerAdapterError::Launch("spawn failed".to_string()),
            HandlerExecutionFailureKind::Launch,
        ),
        (
            HandlerAdapterError::MalformedOutcome("invalid JSON".to_string()),
            HandlerExecutionFailureKind::MalformedOutcome,
        ),
    ];

    for (error, expected_kind) in cases {
        let executor = HandlerExecutor::new(
            Arc::new(UnusedCommandHandler),
            Arc::new(FailingNativeHandler { error }),
            Arc::new(UnusedPluginHandler),
        );
        let failure = executor
            .execute(native_invocation(HandlerExposure {
                data: json!({}),
                capabilities: Vec::new(),
                sensitive_values: Vec::new(),
            }))
            .await
            .unwrap_err();

        assert_eq!(failure.kind, expected_kind);
        assert_eq!(failure.attempts, 1);
    }
}

#[tokio::test]
async fn success_and_failure_diagnostics_are_redacted_at_the_boundary() {
    let exposure = HandlerExposure {
        data: json!({}),
        capabilities: Vec::new(),
        sensitive_values: vec!["super-sensitive-value".to_string()],
    };
    let success = HandlerExecutor::new(
        Arc::new(UnusedCommandHandler),
        Arc::new(DiagnosticNativeHandler),
        Arc::new(UnusedPluginHandler),
    )
    .execute(native_invocation(exposure.clone()))
    .await
    .unwrap();
    assert_eq!(success.diagnostics, vec!["token=*** bearer ***"]);

    let failure = HandlerExecutor::new(
        Arc::new(UnusedCommandHandler),
        Arc::new(FailingNativeHandler {
            error: HandlerAdapterError::Launch(
                "password=super-sensitive-value bearer abcdefghijklmnopqrstuvwxyz".to_string(),
            ),
        }),
        Arc::new(UnusedPluginHandler),
    )
    .execute(native_invocation(exposure))
    .await
    .unwrap_err();
    assert_eq!(failure.kind, HandlerExecutionFailureKind::Launch);
    assert_eq!(failure.diagnostic, "password=*** bearer ***");
}
