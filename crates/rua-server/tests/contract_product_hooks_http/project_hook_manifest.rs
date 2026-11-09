use std::fs;
use std::path::PathBuf;

use axum::http::Method;
use rua_core::ChatAddress;
use rua_server::services::product_hooks::{
    ChatConfigurationMaterialization, CreateProjectHookDefinition, HookHandlerView, HookSource,
    ProductHookService,
};
use rua_server::services::project_hook_manifest::{discover, RESERVED_PROJECT_HOOK_MANIFEST};
use rua_server::services::scope_composition::{HookBinding, SelectionSource};
use serde_json::json;

use super::support::{build_app, read_json, request};

#[tokio::test]
async fn reserved_manifest_registers_inactive_project_definition() {
    let project_root = temp_project_root();
    let manifest_path = project_root.join(RESERVED_PROJECT_HOOK_MANIFEST);
    fs::create_dir_all(manifest_path.parent().unwrap()).unwrap();
    fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&json!({
            "version": 1,
            "hooks": [{
                "id": "audit",
                "display_name": "Project audit",
                "event": "turn.terminal",
                "capabilities": ["identity"],
                "handler": {
                    "kind": "command",
                    "program": "/bin/sh",
                    "args": ["hooks/audit.sh"],
                    "available": true,
                    "working_directory": ".",
                    "timeout_seconds": 30,
                    "required_resources": []
                }
            }]
        }))
        .unwrap(),
    )
    .unwrap();
    let hooks = ProductHookService::new();

    let report = discover("project-a", &project_root, &hooks).await;

    assert!(report.diagnostics.is_empty());
    assert_eq!(report.registered, vec!["project:project-a/audit"]);
    let definitions = hooks.definitions_for_project("project-a").await;
    assert_eq!(definitions.len(), 1);
    assert_eq!(definitions[0].source, HookSource::Project);
    assert_eq!(definitions[0].owner_namespace, "project:project-a");
    assert_eq!(definitions[0].revision, "v1");
    assert!(!definitions[0].active);
    hooks
        .set_project_activation("project-a", &definitions[0].id, true)
        .await
        .unwrap();
    let configuration = hooks
        .resolve_complete_chat_configuration(
            &ChatAddress::project("project-a", "chat-a"),
            ChatConfigurationMaterialization {
                selected_agent_id: "agent-a".to_string(),
                running_config: Default::default(),
                security_config: Default::default(),
                agent_assignment: None,
                prompt_file_content: None,
                tool_config_snapshot: Default::default(),
                capability_configuration: None,
                model: None,
                thinking: None,
                collaboration_mode: json!({}),
                harness_configuration: None,
                authority_device_id: "device-a".to_string(),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        configuration.execution_requirements[0]
            .working_directory
            .as_deref(),
        Some(project_root.to_string_lossy().as_ref())
    );

    fs::remove_dir_all(project_root).unwrap();
}

#[tokio::test]
async fn project_manifest_rejects_working_directory_escape() {
    let project_root = temp_project_root();
    write_manifest(
        &project_root,
        json!({
            "version": 1,
            "hooks": [{
                "id": "escape",
                "display_name": "Escape",
                "event": "turn.terminal",
                "capabilities": ["identity"],
                "handler": {
                    "kind": "command",
                    "program": "/bin/sh",
                    "args": ["hooks/audit.sh"],
                    "working_directory": "../outside",
                    "timeout_seconds": 30,
                    "available": true
                }
            }]
        }),
    );
    let hooks = ProductHookService::new();

    let report = discover("project-escape", &project_root, &hooks).await;

    assert!(report.registered.is_empty());
    assert_eq!(report.diagnostics.len(), 1);
    assert!(report.diagnostics[0].contains("cannot escape the Project root"));
    assert!(hooks
        .definitions_for_project("project-escape")
        .await
        .is_empty());
    fs::remove_dir_all(project_root).unwrap();
}

#[tokio::test]
async fn similarly_named_manifest_is_not_discovered() {
    let project_root = temp_project_root();
    let manifest_path = project_root.join(".rua/hooks.local.json");
    fs::create_dir_all(manifest_path.parent().unwrap()).unwrap();
    fs::write(
        manifest_path,
        serde_json::to_vec_pretty(&json!({
            "version": 1,
            "hooks": [{
                "id": "ignored",
                "display_name": "Ignored",
                "event": "turn.terminal",
                "capabilities": ["identity"],
                "handler": {
                    "kind": "command",
                    "program": "/bin/sh",
                    "args": ["hooks/ignored.sh"],
                    "available": true,
                    "working_directory": ".",
                    "timeout_seconds": 30,
                    "required_resources": []
                }
            }]
        }))
        .unwrap(),
    )
    .unwrap();
    let hooks = ProductHookService::new();

    let report = discover("project-a", &project_root, &hooks).await;

    assert!(report.registered.is_empty());
    assert!(report.diagnostics.is_empty());
    assert!(hooks.definitions_for_project("project-a").await.is_empty());

    fs::remove_dir_all(project_root).unwrap();
}

#[tokio::test]
async fn ambiguous_manifest_registers_nothing_and_reports_the_duplicate() {
    let project_root = temp_project_root();
    let manifest_path = project_root.join(RESERVED_PROJECT_HOOK_MANIFEST);
    fs::create_dir_all(manifest_path.parent().unwrap()).unwrap();
    fs::write(
        manifest_path,
        serde_json::to_vec_pretty(&json!({
            "version": 1,
            "hooks": [
                project_hook("duplicate", "First", "hooks/first.sh"),
                project_hook("duplicate", "Second", "hooks/second.sh")
            ]
        }))
        .unwrap(),
    )
    .unwrap();
    let hooks = ProductHookService::new();

    let report = discover("project-a", &project_root, &hooks).await;

    assert!(report.registered.is_empty());
    assert!(report.diagnostics.iter().any(|diagnostic| {
        diagnostic.contains("duplicate") && diagnostic.contains("logical ID")
    }));
    assert!(hooks.definitions_for_project("project-a").await.is_empty());

    fs::remove_dir_all(project_root).unwrap();
}

#[tokio::test]
async fn executable_change_creates_disabled_revision_without_mutating_chat_snapshot() {
    let project_root = temp_project_root();
    write_manifest(
        &project_root,
        json!({
            "version": 1,
            "hooks": [project_hook("audit", "Project audit", "hooks/audit.sh")]
        }),
    );
    let hooks = ProductHookService::new();
    assert!(discover("project-a", &project_root, &hooks)
        .await
        .diagnostics
        .is_empty());
    hooks
        .set_project_bindings(
            "project-a",
            vec![HookBinding {
                definition_revision: "project:project-a/audit@v1".to_string(),
                event: "turn.terminal".to_string(),
                source: SelectionSource::Project,
                configured_order: 0,
                parameters: Default::default(),
                before: Vec::new(),
                after: Vec::new(),
                requires: Vec::new(),
            }],
        )
        .await;
    let address = ChatAddress::project("project-a", "chat-a");
    let captured = hooks
        .resolve_chat_configuration(&address, "agent-a")
        .await
        .unwrap();
    hooks
        .adopt_chat_configuration(&address, captured.clone())
        .await;

    write_manifest(
        &project_root,
        json!({
            "version": 1,
            "hooks": [project_hook("audit", "Renamed audit", "hooks/audit.sh")]
        }),
    );
    let presentation = discover("project-a", &project_root, &hooks).await;
    assert!(presentation.diagnostics.is_empty());
    let definitions = hooks.definitions_for_project("project-a").await;
    assert_eq!(definitions[0].revision, "v1");
    assert_eq!(definitions[0].display_name, "Renamed audit");
    assert!(definitions[0].active);
    assert!(!definitions[0].revision_changed);
    assert!(!definitions[0].disabled);

    write_manifest(
        &project_root,
        json!({
            "version": 1,
            "hooks": [project_hook("audit", "Renamed audit", "hooks/audit-v2.sh")]
        }),
    );
    let material = discover("project-a", &project_root, &hooks).await;
    assert!(material.diagnostics.is_empty());
    let definitions = hooks.definitions_for_project("project-a").await;
    assert_eq!(definitions[0].revision, "v2");
    assert!(definitions[0].revision_changed);
    assert!(definitions[0].disabled);
    assert!(!definitions[0].active);
    assert_eq!(hooks.chat_configuration(&address).await.unwrap(), captured);
    assert_eq!(
        captured.definition_revisions,
        vec!["project:project-a/audit@v1"]
    );

    fs::remove_dir_all(project_root).unwrap();
}

#[tokio::test]
async fn invalid_and_unavailable_declarations_return_actionable_diagnostics() {
    let malformed_root = temp_project_root();
    let malformed_path = malformed_root.join(RESERVED_PROJECT_HOOK_MANIFEST);
    fs::create_dir_all(malformed_path.parent().unwrap()).unwrap();
    fs::write(&malformed_path, b"{not-json").unwrap();
    let hooks = ProductHookService::new();
    let malformed = discover("malformed", &malformed_root, &hooks).await;
    assert!(malformed.registered.is_empty());
    assert!(malformed.diagnostics[0].contains("failed to parse"));

    let unsupported_root = temp_project_root();
    write_manifest(
        &unsupported_root,
        json!({
            "version": 2,
            "hooks": [project_hook("audit", "Audit", "hooks/audit.sh")]
        }),
    );
    let unsupported = discover("unsupported", &unsupported_root, &hooks).await;
    assert!(unsupported.registered.is_empty());
    assert!(unsupported.diagnostics[0].contains("expected version 1"));

    let invalid_root = temp_project_root();
    let mut invalid = project_hook("invalid", "Invalid", "hooks/invalid.sh");
    invalid["event"] = json!("model.before");
    write_manifest(
        &invalid_root,
        json!({
            "version": 1,
            "hooks": [
                project_hook("valid", "Valid", "hooks/valid.sh"),
                invalid
            ]
        }),
    );
    let invalid = discover("invalid", &invalid_root, &hooks).await;
    assert!(invalid.registered.is_empty());
    assert!(invalid.diagnostics[0].contains("unsupported Hook event"));
    assert!(hooks.definitions_for_project("invalid").await.is_empty());

    let unavailable_root = temp_project_root();
    let mut unavailable = project_hook("offline", "Offline", "hooks/offline.sh");
    unavailable["handler"]["available"] = json!(false);
    write_manifest(
        &unavailable_root,
        json!({"version": 1, "hooks": [unavailable]}),
    );
    let unavailable = discover("unavailable", &unavailable_root, &hooks).await;
    assert_eq!(unavailable.registered, vec!["project:unavailable/offline"]);
    assert!(unavailable.diagnostics[0].contains("unavailable"));
    let definitions = hooks.definitions_for_project("unavailable").await;
    assert_eq!(definitions[0].availability.status, "unavailable");
    assert!(!definitions[0].diagnostics.is_empty());
    assert!(definitions[0].disabled);
    assert!(!definitions[0].active);

    for root in [
        malformed_root,
        unsupported_root,
        invalid_root,
        unavailable_root,
    ] {
        fs::remove_dir_all(root).unwrap();
    }
}

#[tokio::test]
async fn project_definitions_api_exposes_registry_state_without_discovery() {
    let test = build_app();
    test.state
        .product_hooks
        .register_project_definition(
            "project-api",
            CreateProjectHookDefinition {
                logical_id: "offline".to_string(),
                display_name: "Offline".to_string(),
                event: "turn.terminal".to_string(),
                capabilities: vec!["identity".to_string()],
                handler: HookHandlerView {
                    kind: "command".to_string(),
                    executable: None,
                    program: Some("/bin/sh".to_string()),
                    args: vec!["hooks/offline.sh".to_string()],
                    available: false,
                    working_directory: Some("/workspace".to_string()),
                    timeout_seconds: Some(30),
                    required_resources: Vec::new(),
                },
            },
        )
        .await
        .unwrap();

    let response = request(
        &test.app,
        Method::GET,
        "/api/projects/project-api/hooks/definitions",
        None,
    )
    .await;

    let status = response.status();
    let body = read_json(response).await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(body[0]["source"], "project");
    assert_eq!(body[0]["revision"], "v1");
    assert_eq!(body[0]["revision_changed"], false);
    assert_eq!(body[0]["disabled"], true);
    assert_eq!(body[0]["availability"]["status"], "unavailable");
    assert!(body[0]["availability"]["diagnostics"][0]
        .as_str()
        .unwrap()
        .contains("unavailable"));
    assert_eq!(body[0]["validation"]["status"], "valid");
    assert_eq!(body[0]["validation"]["diagnostics"], json!([]));
    assert!(test
        .state
        .product_hooks
        .definitions_for_project("unrelated")
        .await
        .is_empty());
}

fn project_hook(id: &str, display_name: &str, script: &str) -> serde_json::Value {
    json!({
        "id": id,
        "display_name": display_name,
        "event": "turn.terminal",
        "capabilities": ["identity"],
        "handler": {
            "kind": "command",
            "program": "/bin/sh",
            "args": [script],
            "available": true,
            "working_directory": ".",
            "timeout_seconds": 30,
            "required_resources": []
        }
    })
}

fn write_manifest(project_root: &std::path::Path, manifest: serde_json::Value) {
    let manifest_path = project_root.join(RESERVED_PROJECT_HOOK_MANIFEST);
    fs::create_dir_all(manifest_path.parent().unwrap()).unwrap();
    fs::write(manifest_path, serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();
}

fn temp_project_root() -> PathBuf {
    std::env::temp_dir().join(format!(
        "rua-project-hook-manifest-{}",
        uuid::Uuid::new_v4()
    ))
}
