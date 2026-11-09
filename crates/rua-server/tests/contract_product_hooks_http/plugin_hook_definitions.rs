use rua_core::ChatAddress;
use rua_plugin::PluginRecord;
use rua_server::services::plugin_hook_definitions::register_from_record;
use rua_server::services::product_hooks::{HookSource, ProductHookService};
use serde_json::json;

use super::support::{build_app, read_json, request};

fn plugin_record(enabled: bool, hook_definitions: serde_json::Value) -> PluginRecord {
    PluginRecord {
        id: "sample".to_string(),
        name: "Sample".to_string(),
        version: "1.0.0".to_string(),
        enabled,
        installed: true,
        extra: serde_json::from_value(json!({
            "meta": {
                "hook_definitions": hook_definitions
            }
        }))
        .unwrap(),
        ..PluginRecord::default()
    }
}

fn audit_declaration() -> serde_json::Value {
    json!({
        "logical_id": "audit",
        "display_name": "Sample audit",
        "event": "turn.terminal",
        "capabilities": ["identity"],
        "handler": {
            "kind": "plugin",
            "executable": "audit"
        }
    })
}

#[tokio::test]
async fn plugin_record_registers_qualified_inactive_definition_without_changing_defaults() {
    let hooks = ProductHookService::new();
    let agent_address = ChatAddress::agent("agent-a", "chat-a");
    let project_address = ChatAddress::project("project-a", "chat-b");
    let agent_before = hooks
        .resolve_chat_configuration(&agent_address, "agent-a")
        .await
        .unwrap();
    let project_before = hooks
        .resolve_chat_configuration(&project_address, "agent-a")
        .await
        .unwrap();
    let record = plugin_record(true, json!([audit_declaration()]));

    register_from_record(&hooks, &record).await.unwrap();

    let definitions = hooks.definitions_for_plugin("sample").await;
    assert_eq!(definitions.len(), 1);
    assert_eq!(definitions[0].id, "plugin:sample/audit");
    assert_eq!(definitions[0].revision, "v1");
    assert_eq!(definitions[0].source, HookSource::Plugin);
    assert!(!definitions[0].active);
    assert!(definitions[0].disabled);

    let agent_after = hooks
        .resolve_chat_configuration(&agent_address, "agent-a")
        .await
        .unwrap();
    let project_after = hooks
        .resolve_chat_configuration(&project_address, "agent-a")
        .await
        .unwrap();
    assert_eq!(
        agent_after.definition_revisions,
        agent_before.definition_revisions
    );
    assert_eq!(
        project_after.definition_revisions,
        project_before.definition_revisions
    );
    assert!(hooks.chat_configuration(&agent_address).await.is_none());
    assert!(hooks.chat_configuration(&project_address).await.is_none());
}

#[tokio::test]
async fn plugin_definition_requires_explicit_agent_or_project_activation() {
    let hooks = ProductHookService::new();
    register_from_record(&hooks, &plugin_record(true, json!([audit_declaration()])))
        .await
        .unwrap();

    hooks
        .set_agent_activation("agent-a", "plugin:sample/audit", true)
        .await
        .unwrap();
    hooks
        .set_project_activation("project-a", "plugin:sample/audit", true)
        .await
        .unwrap();

    let agent = hooks
        .resolve_chat_configuration(&ChatAddress::agent("agent-a", "chat-a"), "agent-a")
        .await
        .unwrap();
    assert_eq!(agent.definition_revisions, vec!["plugin:sample/audit@v1"]);
    let project = hooks
        .resolve_chat_configuration(
            &ChatAddress::project("project-a", "chat-b"),
            "agent-without-default",
        )
        .await
        .unwrap();
    assert_eq!(project.definition_revisions, vec!["plugin:sample/audit@v1"]);
}

#[tokio::test]
async fn new_chat_adopts_plugin_default_while_existing_chat_requires_update() {
    let hooks = ProductHookService::new();
    register_from_record(&hooks, &plugin_record(true, json!([audit_declaration()])))
        .await
        .unwrap();
    let existing_address = ChatAddress::agent("agent-a", "existing");
    let existing = hooks
        .resolve_chat_configuration(&existing_address, "agent-a")
        .await
        .unwrap();
    hooks
        .adopt_chat_configuration(&existing_address, existing.clone())
        .await;

    hooks
        .set_agent_activation("agent-a", "plugin:sample/audit", true)
        .await
        .unwrap();

    let new_chat = hooks
        .resolve_chat_configuration(&ChatAddress::agent("agent-a", "new"), "agent-a")
        .await
        .unwrap();
    assert_eq!(
        new_chat.definition_revisions,
        vec!["plugin:sample/audit@v1"]
    );
    assert_eq!(
        hooks
            .chat_configuration(&existing_address)
            .await
            .unwrap()
            .definition_revisions,
        existing.definition_revisions
    );

    let updated = hooks
        .resolve_chat_configuration(&existing_address, "agent-a")
        .await
        .unwrap();
    hooks
        .adopt_chat_configuration(&existing_address, updated.clone())
        .await;
    assert_eq!(
        hooks
            .chat_configuration(&existing_address)
            .await
            .unwrap()
            .definition_revisions,
        vec!["plugin:sample/audit@v1"]
    );
}

#[tokio::test]
async fn plugin_unavailability_preserves_bindings_chat_snapshot_and_frozen_turn() {
    let hooks = ProductHookService::new();
    register_from_record(&hooks, &plugin_record(true, json!([audit_declaration()])))
        .await
        .unwrap();
    hooks
        .set_agent_activation("agent-a", "plugin:sample/audit", true)
        .await
        .unwrap();
    let address = ChatAddress::agent("agent-a", "chat-a");
    let available = hooks
        .resolve_chat_configuration(&address, "agent-a")
        .await
        .unwrap();
    hooks
        .adopt_chat_configuration(&address, available.clone())
        .await;
    hooks.freeze_turn(&address, "turn-a").await;

    hooks
        .mark_plugin_unavailable("sample", "Plugin disabled")
        .await;

    let definition = &hooks.definitions_for_plugin("sample").await[0];
    assert_eq!(definition.availability.status, "unavailable");
    assert_eq!(definition.diagnostics, vec!["Plugin disabled"]);
    assert!(!definition.active);
    assert!(
        hooks
            .chat_configuration(&address)
            .await
            .unwrap()
            .execution_requirements[0]
            .available
    );
    assert!(
        hooks
            .frozen_turn_configuration(&address, "turn-a")
            .await
            .unwrap()
            .execution_requirements[0]
            .available
    );

    let refreshed = hooks
        .resolve_chat_configuration(&address, "agent-a")
        .await
        .unwrap();
    assert_eq!(
        refreshed.definition_revisions,
        vec!["plugin:sample/audit@v1"]
    );
    assert!(!refreshed.execution_requirements[0].available);
}

#[tokio::test]
async fn plugin_reregistration_keeps_revisions_immutable_and_existing_selection_pinned() {
    let hooks = ProductHookService::new();
    register_from_record(&hooks, &plugin_record(true, json!([audit_declaration()])))
        .await
        .unwrap();
    hooks
        .set_agent_activation("agent-a", "plugin:sample/audit", true)
        .await
        .unwrap();

    register_from_record(&hooks, &plugin_record(false, json!([audit_declaration()])))
        .await
        .unwrap();
    assert_eq!(
        hooks.definitions_for_plugin("sample").await[0].latest_revision,
        "v1"
    );

    let changed = json!({
        "logical_id": "audit",
        "display_name": "Sample audit",
        "event": "tool.after",
        "capabilities": ["identity", "tool_result"],
        "handler": {
            "kind": "plugin",
            "executable": "audit"
        }
    });
    register_from_record(&hooks, &plugin_record(true, json!([changed])))
        .await
        .unwrap();
    assert_eq!(
        hooks.definitions_for_plugin("sample").await[0].latest_revision,
        "v2"
    );
    let selected = hooks
        .resolve_chat_configuration(&ChatAddress::agent("agent-a", "chat-a"), "agent-a")
        .await
        .unwrap();
    assert_eq!(
        selected.definition_revisions,
        vec!["plugin:sample/audit@v1"]
    );
}

#[tokio::test]
async fn plugin_declarations_cannot_claim_native_handlers_or_broader_capabilities() {
    let hooks = ProductHookService::new();
    let native = json!({
        "logical_id": "native",
        "display_name": "Not native",
        "event": "turn.terminal",
        "capabilities": ["identity"],
        "handler": {
            "kind": "native",
            "executable": "turn_terminal_audit"
        }
    });
    assert!(
        register_from_record(&hooks, &plugin_record(true, json!([native])))
            .await
            .is_err()
    );

    let broad = json!({
        "logical_id": "broad",
        "display_name": "Too broad",
        "event": "turn.terminal",
        "capabilities": ["tool_arguments"],
        "handler": {
            "kind": "plugin",
            "executable": "broad"
        }
    });
    assert!(
        register_from_record(&hooks, &plugin_record(true, json!([broad])))
            .await
            .unwrap_err()
            .contains("not allowed")
    );
    assert!(hooks.definitions_for_plugin("sample").await.is_empty());
}

#[tokio::test]
async fn plugin_http_install_and_uninstall_update_definition_availability_only() {
    let fixture = tempfile::TempDir::new().unwrap();
    std::fs::write(
        fixture.path().join("plugin.json"),
        serde_json::to_vec_pretty(&json!({
            "id": "sample-hook-plugin",
            "name": "Sample Hook Plugin",
            "version": "1.0.0",
            "entry_point": "plugin.wasm",
            "min_rua_version": "0.1.0",
            "meta": {
                "hook_definitions": [{
                    "logical_id": "audit",
                    "display_name": "Sample audit",
                    "event": "turn.terminal",
                    "capabilities": ["identity"],
                    "handler": {"kind": "plugin", "executable": "audit"}
                }]
            }
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(
        fixture.path().join("plugin.wasm"),
        [0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00],
    )
    .unwrap();
    let test = build_app();

    let response = request(
        &test.app,
        axum::http::Method::POST,
        "/api/plugins/install",
        Some(json!({"source": fixture.path()})),
    )
    .await;
    let status = response.status();
    let body = read_json(response).await;
    assert!(
        status == axum::http::StatusCode::OK || status == axum::http::StatusCode::MULTI_STATUS,
        "{body}"
    );
    let definitions = test
        .state
        .product_hooks
        .definitions_for_plugin("sample-hook-plugin")
        .await;
    assert_eq!(definitions.len(), 1);
    assert_eq!(definitions[0].id, "plugin:sample-hook-plugin/audit");
    assert_eq!(definitions[0].availability.status, "available");

    test.state
        .product_hooks
        .set_agent_activation("agent-a", "plugin:sample-hook-plugin/audit", true)
        .await
        .unwrap();
    let address = ChatAddress::agent("agent-a", "chat-a");
    let snapshot = test
        .state
        .product_hooks
        .resolve_chat_configuration(&address, "agent-a")
        .await
        .unwrap();
    test.state
        .product_hooks
        .adopt_chat_configuration(&address, snapshot.clone())
        .await;

    let response = request(
        &test.app,
        axum::http::Method::DELETE,
        "/api/plugins/sample-hook-plugin",
        None,
    )
    .await;
    let status = response.status();
    let body = read_json(response).await;
    assert_eq!(status, axum::http::StatusCode::OK, "{body}");
    let definitions = test
        .state
        .product_hooks
        .definitions_for_plugin("sample-hook-plugin")
        .await;
    assert_eq!(definitions.len(), 1);
    assert_eq!(definitions[0].availability.status, "unavailable");
    assert_eq!(
        test.state
            .product_hooks
            .chat_configuration(&address)
            .await
            .unwrap(),
        snapshot
    );
}
