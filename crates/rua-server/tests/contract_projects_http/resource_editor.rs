use super::support::{build_app, local_device_id, TestApp};
use rua_core::ProjectId;
use rua_device_network::DeviceSessionCommandHandler;
use rua_server::services::device_command_bus::{ProjectDeviceAction, ProjectDeviceCommand};
use rua_server::services::project_lifecycle::ProjectResourceEditor;
use serde_json::json;
use std::sync::atomic::Ordering;

fn project(test: &TestApp) -> (ProjectId, std::path::PathBuf, std::path::PathBuf) {
    let first = test._temp.path().join("first");
    let second = test._temp.path().join("second");
    std::fs::create_dir(&first).unwrap();
    std::fs::create_dir(&second).unwrap();
    let created = test
        .state
        .project_catalog
        .create_bound(rua_project::ProjectCreateRequest {
            resource_device_id: local_device_id(&test.state),
            resource_root: first.to_string_lossy().into(),
            display_name: None,
        })
        .unwrap();
    (created.project.project_id, first, second)
}

#[tokio::test]
async fn rebind_missing_directory_preserves_existing_chat_and_rejects_stale_edits() {
    use super::support::{json as response_json, request};
    use axum::http::{Method, StatusCode};
    use rua_core::{ChatAddress, ChatScopeStore};
    use rua_session::{ChatMetadata, ChatSessionHeader, WorkspaceSessionStore};
    let test = build_app();
    let (id, first, second) = project(&test);
    let address = ChatAddress::project(id.clone(), "00000000-0000-4000-8000-000000000591");
    let scope = ChatScopeStore::resolve(&test.state.rua_root, address.clone());
    let store = WorkspaceSessionStore::new(scope.root());
    let mut header = ChatSessionHeader::new(ChatMetadata {
        chat_id: address.chat_id().to_string(),
        agent_id: "default".into(),
        title: "Keep this chat".into(),
        created_at: "2026-11-07T00:00:00Z".into(),
        last_activity_at: "2026-11-07T00:00:00Z".into(),
        pinned: true,
        archived: true,
        preview: None,
    });
    header
        .extra
        .insert("project_working_directory_id".into(), json!("initial"));
    store.save_chat_header(&header).await.unwrap();
    rua_session::chat_index::insert(
        scope.root(),
        rua_session::chat_index::ChatIndexRecord {
            id: address.chat_id().to_string(),
            name: header.metadata.title.clone(),
            user_id: "default".into(),
            channel: "console".into(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            meta: json!({}),
            status: "idle".into(),
            pinned: true,
            pin_order: 0,
            tag: Default::default(),
            archived: true,
        },
    )
    .await
    .unwrap();
    std::fs::remove_dir(&first).unwrap();
    let uri = format!("/api/projects/{id}/resources");
    let body = json!({"expected_revision":1,"resource_roots":[second],"primary_index":0,"directory_ids":["initial"]});
    let admission = test.state.project_catalog.begin_admission(&id).unwrap();
    assert!(!request(&test.app, Method::PATCH, &uri, Some(body.clone()))
        .await
        .status()
        .is_success());
    drop(admission);
    let response = request(&test.app, Method::PATCH, &uri, Some(body.clone())).await;
    let status = response.status();
    let result = response_json(response).await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(result["project"]["project_id"], id.as_str());
    assert_eq!(
        result["project"]["resources"]["primary_directory_id"],
        "initial"
    );
    let binding = test.state.project_bindings.get(&id).unwrap().unwrap();
    assert_eq!(
        binding
            .resources
            .directory("initial")
            .unwrap()
            .resource_root,
        second
    );
    assert_eq!(
        rua_server::services::project_chat::working_directory(&binding, Some(&header), None)
            .unwrap(),
        second
    );
    assert_eq!(
        store
            .load_chat_header(address.chat_id().as_str())
            .await
            .unwrap()
            .unwrap(),
        header
    );
    let stale = request(&test.app, Method::PATCH, &uri, Some(body)).await;
    assert!(!stale.status().is_success());
    assert_eq!(
        test.state.project_bindings.get(&id).unwrap().unwrap(),
        binding
    );
    assert!(!first.exists());
}

#[tokio::test]
async fn device_command_rebinds_missing_directory_without_replacing_its_identity() {
    let test = build_app();
    let (id, first, second) = project(&test);
    std::fs::remove_dir(&first).unwrap();
    let handler = test.state.device_command_handler().unwrap();
    let command = ProjectDeviceCommand {
        command_id: "rebind-resource".into(),
        source_device_id: "controller".into(),
        target_device_id: local_device_id(&test.state),
        action: ProjectDeviceAction::EditResources {
            project_id: id.clone(),
            expected_revision: 1,
            resource_roots: vec![second.clone()],
            primary_index: 0,
            directory_ids: vec![Some("initial".into())],
        },
    };
    let payload = serde_json::to_vec(&json!({"command":"project", "payload":command})).unwrap();
    let response: serde_json::Value =
        serde_json::from_slice(&handler.handle_command(&payload).await.unwrap()).unwrap();
    assert_eq!(response["status"], "project", "{response}");
    let binding = test.state.project_bindings.get(&id).unwrap().unwrap();
    assert_eq!(
        binding
            .resources
            .directory("initial")
            .unwrap()
            .resource_root,
        second
    );
    assert_eq!(binding.resources.revision, 2);
    assert!(!first.exists());
}

#[tokio::test]
async fn failed_edits_release_admission_and_unbound_sync_preserves_local_commit() {
    let test = build_app();
    let (id, first, second) = project(&test);
    let editor = ProjectResourceEditor::from_app_state(&test.state);
    let admission = test.state.project_catalog.begin_admission(&id).unwrap();
    assert!(editor
        .edit_resources(&id, 1, &[first.clone(), second.clone()], 1, &[])
        .await
        .is_err());
    drop(admission);
    assert!(editor.edit_resources(&id, 1, &[], 0, &[]).await.is_err());
    drop(test.state.project_catalog.begin_admission(&id).unwrap());
    test.state.sync_enabled.store(true, Ordering::Relaxed);
    assert!(test.state.sync_node.lock().await.is_none());
    let changed = editor
        .edit_resources(&id, 1, &[first, second], 1, &[])
        .await
        .unwrap();
    let binding = test.state.project_bindings.get(&id).unwrap().unwrap();
    assert_eq!(
        changed.project.resources.unwrap().revision,
        binding.resources.revision
    );
    assert_eq!(binding.resources.revision, 2);
    assert!(editor
        .set_primary_directory(&id, 1, "initial")
        .await
        .is_err());
    drop(test.state.project_catalog.begin_admission(&id).unwrap());
    assert!(editor
        .set_primary_directory(&id, 2, "missing")
        .await
        .is_err());
    drop(test.state.project_catalog.begin_admission(&id).unwrap());
    editor
        .set_primary_directory(&id, 2, "initial")
        .await
        .unwrap();
}

#[tokio::test]
async fn cancelling_an_edit_releases_admission_without_committing() {
    let test = build_app();
    let (id, first, second) = project(&test);
    let editor = ProjectResourceEditor::from_app_state(&test.state);
    let paths = [first, second];
    let mut edit = Box::pin(editor.edit_resources(&id, 1, &paths, 1, &[]));
    assert!(futures::poll!(edit.as_mut()).is_pending());
    assert!(test.state.project_catalog.begin_admission(&id).is_err());
    drop(edit);
    drop(test.state.project_catalog.begin_admission(&id).unwrap());
    assert_eq!(
        test.state
            .project_bindings
            .get(&id)
            .unwrap()
            .unwrap()
            .resources
            .revision,
        1
    );
    editor.edit_resources(&id, 1, &paths, 1, &[]).await.unwrap();
}

#[tokio::test]
async fn primary_change_does_not_revalidate_unselected_missing_directory() {
    let test = build_app();
    let (id, first, second) = project(&test);
    let editor = ProjectResourceEditor::from_app_state(&test.state);
    editor
        .edit_resources(&id, 1, &[first.clone(), second], 0, &[])
        .await
        .unwrap();
    let binding = test.state.project_bindings.get(&id).unwrap().unwrap();
    let second_id = &binding.resources.directories[1].directory_id;
    std::fs::remove_dir(first).unwrap();
    editor
        .set_primary_directory(&id, 2, second_id)
        .await
        .unwrap();
    assert_eq!(
        test.state
            .project_bindings
            .get(&id)
            .unwrap()
            .unwrap()
            .resources
            .primary_directory_id,
        *second_id
    );
}

#[tokio::test]
async fn device_commands_use_resource_editor_and_reject_stale_revisions() {
    let test = build_app();
    let (id, first, second) = project(&test);
    let handler = test.state.device_command_handler().unwrap();
    let local = local_device_id(&test.state);
    let actions = [
        ProjectDeviceAction::EditResources {
            project_id: id.clone(),
            expected_revision: 1,
            resource_roots: vec![first, second],
            primary_index: 1,
            directory_ids: vec![],
        },
        ProjectDeviceAction::SetPrimaryDirectory {
            project_id: id.clone(),
            expected_revision: 2,
            directory_id: "initial".into(),
        },
        ProjectDeviceAction::SetPrimaryDirectory {
            project_id: id.clone(),
            expected_revision: 2,
            directory_id: "initial".into(),
        },
    ];
    for (index, action) in actions.into_iter().enumerate() {
        let command = ProjectDeviceCommand {
            command_id: format!("edit-{index}"),
            source_device_id: "controller".into(),
            target_device_id: local.clone(),
            action,
        };
        let payload = serde_json::to_vec(&json!({"command":"project", "payload":command})).unwrap();
        let response: serde_json::Value =
            serde_json::from_slice(&handler.handle_command(&payload).await.unwrap()).unwrap();
        if index < 2 {
            assert_eq!(response["status"], "project", "{response}");
        } else {
            assert_eq!(response["status"], "rejected", "{response}");
            assert!(response["detail"].as_str().unwrap().contains("reload"));
        }
    }
    assert_eq!(
        test.state
            .project_bindings
            .get(&id)
            .unwrap()
            .unwrap()
            .resources
            .revision,
        3
    );
}
