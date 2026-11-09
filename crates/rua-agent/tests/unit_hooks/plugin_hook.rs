use std::sync::Arc;

use agent_types::ToolCall;
use react_agent::hooks::AgentHook;
use react_agent::ToolDecision;
use rua_agent::hooks::plugin_hook::{PluginHook, PluginHookOutcome};
use rua_plugin_runtime::{HookPoint, HookResult, HookSpec, PluginRegistry};

fn hook_result(output: impl Into<String>) -> HookResult {
    HookResult {
        plugin_id: "p1".into(),
        hook_name: "guard".into(),
        output: output.into(),
    }
}

#[test]
fn parse_hook_outcome_empty_results_continue() {
    let outcome = PluginHook::parse_hook_outcome(&[]);
    assert_eq!(outcome, PluginHookOutcome::Continue);
}

#[test]
fn parse_hook_outcome_deny_uses_plugin_reason() {
    let results = vec![hook_result(
        r#"{"action":"deny","reason":"blocked by policy"}"#,
    )];

    let outcome = PluginHook::parse_hook_outcome(&results);

    assert_eq!(outcome, PluginHookOutcome::Deny("blocked by policy".into()));
}

#[test]
fn parse_hook_outcome_deny_uses_default_reason_when_missing() {
    let results = vec![hook_result(r#"{"action":"deny"}"#)];

    let outcome = PluginHook::parse_hook_outcome(&results);

    assert_eq!(outcome, PluginHookOutcome::Deny("denied by plugin".into()));
}

#[test]
fn parse_hook_outcome_modify_keeps_patch_payload() {
    let results = vec![hook_result(
        r#"{"action":"modify","patch":{"key":"value"}}"#,
    )];

    let outcome = PluginHook::parse_hook_outcome(&results);

    assert_eq!(
        outcome,
        PluginHookOutcome::Modify(serde_json::json!({"key": "value"}))
    );
}

#[test]
fn parse_hook_outcome_ignores_empty_garbage_and_unknown_actions() {
    let results = vec![
        hook_result(""),
        hook_result("not json at all"),
        hook_result(r#"{"action":"log","message":"ok"}"#),
    ];

    let outcome = PluginHook::parse_hook_outcome(&results);

    assert_eq!(outcome, PluginHookOutcome::Continue);
}

#[tokio::test]
async fn plugin_hook_no_plugins_allows_tool_call() {
    let registry = Arc::new(PluginRegistry::new());
    let hook = PluginHook::new(registry);

    let call = ToolCall {
        id: "tc_1".into(),
        name: "test_tool".into(),
        arguments: serde_json::json!({}),
        raw_arguments: None,
        provider_call_id: None,
        metadata: Default::default(),
    };

    let decision = hook.before_tool_call(&call, 0).await.unwrap();

    assert!(matches!(decision, ToolDecision::Allow));
}

#[tokio::test]
async fn on_finish_keeps_conversation_end_and_shutdown_hooks_separate() {
    let registry = Arc::new(PluginRegistry::new());

    registry
        .register_hook(
            "test-p",
            HookSpec {
                hook_point: HookPoint::OnConversationEnd,
                hook_name: "conv-end".into(),
                callback: "cb".into(),
                priority: 100,
            },
        )
        .await
        .unwrap();
    registry
        .register_hook(
            "test-p",
            HookSpec {
                hook_point: HookPoint::Shutdown,
                hook_name: "shutdown".into(),
                callback: "cb".into(),
                priority: 100,
            },
        )
        .await
        .unwrap();

    let hook = PluginHook::new(Arc::clone(&registry));
    hook.on_finish(5, &Ok(())).await;

    let conv_hooks = registry
        .get_hooks_for_point(HookPoint::OnConversationEnd)
        .await;
    assert_eq!(conv_hooks.len(), 1);
    assert_eq!(conv_hooks[0].1.hook_name, "conv-end");

    let shutdown_hooks = registry.get_hooks_for_point(HookPoint::Shutdown).await;
    assert_eq!(shutdown_hooks.len(), 1);
    assert_eq!(shutdown_hooks[0].1.hook_name, "shutdown");
}

#[tokio::test]
async fn on_finish_no_crash_with_empty_registry() {
    let registry = Arc::new(PluginRegistry::new());
    let hook = PluginHook::new(registry);

    hook.on_finish(5, &Ok(())).await;
}
