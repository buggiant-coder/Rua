use axum::http::{Method, StatusCode};
use chrono::Utc;
use rua_core::{ChatAddress, ChatStore, ProjectId};
use rua_project::{ProjectDescriptor, ProjectLifecycle};
use rua_provider::config_loader::{DefaultModel, ProviderConfig};
use rua_security::{
    ApprovalDecision, ApprovalRequestInput, ApprovalReuseMatcher, ApprovalTrigger, ToolCallSnapshot,
};
use rua_server::services::conversation_authority::{
    ConversationActiveTurnStart, ConversationDriverKind, ConversationInputPayload,
};
use serde_json::json;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use super::support::{
    build_app, json as response_json, local_device_id, request, wait_for_chat_idle,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conflicting_project_writes_preserve_saved_membership_and_history() {
    let test = build_app();
    let mut projects = Vec::new();
    for name in ["A", "B"] {
        let root = test._temp.path().join(name);
        std::fs::create_dir_all(&root).unwrap();
        let response = request(
            &test.app,
            Method::POST,
            "/api/projects",
            Some(json!({
                "directories": [{"device_id": local_device_id(&test.state), "resource_root": root}]
            })),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CREATED);
        projects.push(
            response_json(response).await["project"]["project_id"]
                .as_str()
                .unwrap()
                .to_owned(),
        );
    }
    for project_id in [None, Some(&projects[0])] {
        let chat_id = uuid::Uuid::new_v4().to_string();
        let path = format!("/api/chats/{chat_id}");
        let submit_path = format!("{path}/console/submit");
        let submitted = request(
            &test.app,
            Method::POST,
            &submit_path,
            Some(json!({
                "agent_id":"default", "project_id":project_id, "message":"first accepted input",
            })),
        )
        .await;
        assert_eq!(response_json(submitted).await["accepted"], true);
        wait_for_chat_idle(&test.state, &chat_id).await;
        let history = response_json(request(&test.app, Method::GET, &path, None).await).await;
        assert_eq!(history["project_id"], json!(project_id));

        for proposed in [Some(&projects[1]), None]
            .into_iter()
            .filter(|p| *p != project_id)
        {
            for (method, uri, body) in [
                (
                    Method::POST,
                    &submit_path,
                    json!({"project_id":proposed,"agent_id":"default","message":"conflicting first submit"}),
                ),
                (
                    Method::PUT,
                    &path,
                    json!({"project_id":proposed,"name":"unwanted rename"}),
                ),
                (
                    Method::PATCH,
                    &path,
                    json!({"project_id":proposed,"name":"unwanted rename"}),
                ),
            ] {
                let response = request(&test.app, method.clone(), uri, Some(body)).await;
                let status = response.status();
                let error = response_json(response).await;
                assert_eq!(status, StatusCode::CONFLICT, "{method} {uri}: {error}");
                assert!(error["detail"]
                    .as_str()
                    .unwrap()
                    .contains("Project membership"));
            }
        }
        let reopened = response_json(request(&test.app, Method::GET, &path, None).await).await;
        assert_eq!(reopened["project_id"], history["project_id"]);
        assert_eq!(reopened["messages"], history["messages"]);
        assert_eq!(reopened["title"], history["title"]);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn first_project_input_starts_acp_in_the_chat_execution_directory() {
    let test = build_app();
    test.state.wait_platform_initialization().await.unwrap();
    let resource = test._temp.path().join("first-acp-resource");
    std::fs::create_dir_all(&resource).unwrap();
    let project = request(&test.app, Method::POST, "/api/projects", Some(json!({
        "directories": [{"device_id": local_device_id(&test.state), "resource_root": resource}], "display_name": "ACP resource"
    }))).await;
    assert_eq!(project.status(), StatusCode::CREATED);
    let project_id = response_json(project).await["project"]["project_id"]
        .as_str()
        .unwrap()
        .to_string();
    let mock = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../rua-acp/tests/fixtures/mock_acp_agent.py");
    let created = request(&test.app, Method::POST, "/api/agents/default/acp", Some(json!({
        "key": "first-project-acp", "enabled": true, "command": "python3", "args": [mock], "env": {}, "trusted": true
    }))).await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let address = ChatAddress::new("00000000-0000-4000-8000-000000000907");
    let scope = ChatStore::resolve(&test.state.rua_root, address.clone());
    let store = rua_session::WorkspaceSessionStore::new(scope.root());
    assert!(store
        .load_chat_header(address.chat_id().as_str())
        .await
        .unwrap()
        .is_none());
    let submitted = request(&test.app, Method::POST, &format!("/api/chats/{}/console/submit", address.chat_id()), Some(json!({ "project_id": project_id,
        "agent_id": "default", "message": "project ACP input", "initial_acp": {"runner": "first-project-acp", "cwd": "/source/not-project"}, "collaboration_mode":"plan", "harness_configuration":{"mode":"strict"}
    }))).await;
    assert_eq!(submitted.status(), StatusCode::OK);
    assert_eq!(response_json(submitted).await["accepted"], true);
    let header = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            if let Some(header) = store
                .load_chat_header(address.chat_id().as_str())
                .await
                .unwrap()
            {
                if header.extra.contains_key("acp_binding") {
                    break header;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("Project first input binds ACP after acceptance");
    assert_eq!(header.extra["acp_binding"]["runner"], "first-project-acp");
    assert_eq!(
        header.extra["hook_configuration"]["collaboration_mode"],
        "plan"
    );
    assert_eq!(
        header.extra["hook_configuration"]["harness_configuration"],
        json!({"mode":"strict"})
    );
    assert_eq!(
        header.extra["acp_binding"]["cwd"],
        header.extra[rua_session::EXECUTION_DIRECTORY_KEY]["path"]
    );
    assert_ne!(header.extra["acp_binding"]["cwd"], json!(resource));
    let exited = request(
        &test.app,
        Method::POST,
        &format!("/api/chats/{}/acp/exit", address.chat_id()),
        None,
    )
    .await;
    assert_eq!(exited.status(), StatusCode::OK);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn directory_create_and_bind_are_explicit_and_non_destructive() {
    let test = build_app();
    let device_id = local_device_id(&test.state);
    let parent = test._temp.path().join("folder-parent");
    std::fs::create_dir(&parent).unwrap();
    let body = json!({ "device_id": device_id, "parent": parent, "name": "my" });
    let created = request(
        &test.app,
        Method::POST,
        "/api/projects/directory/create",
        Some(body.clone()),
    )
    .await;
    assert_eq!(created.status(), StatusCode::OK);
    let listing = response_json(created).await;
    assert_eq!(
        listing["path"],
        parent.join("my").to_string_lossy().as_ref()
    );
    assert_eq!(listing["proposed_name"], "my");
    assert!(listing["roots"].is_array());
    std::fs::write(parent.join("my/notes"), "preserve").unwrap();
    let duplicate = request(
        &test.app,
        Method::POST,
        "/api/projects/directory/create",
        Some(body),
    )
    .await;
    assert_eq!(duplicate.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        std::fs::read_to_string(parent.join("my/notes")).unwrap(),
        "preserve"
    );
    let invalid = request(
        &test.app,
        Method::POST,
        "/api/projects/directory/create",
        Some(json!({
            "device_id": device_id, "parent": parent, "name": "../escape"
        })),
    )
    .await;
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
    let project = request(&test.app, Method::POST, "/api/projects", Some(json!({
        "directories": [{"device_id": device_id, "resource_root": parent.join("my")}], "display_name": "my"
    }))).await;
    assert_eq!(project.status(), StatusCode::CREATED);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_create_browse_list_get_rename_and_shared_directory_contract() {
    let test = build_app();
    let local_device_id = local_device_id(&test.state);
    let repo = test._temp.path().join("repo");
    std::fs::create_dir_all(repo.join("package")).unwrap();
    std::fs::write(repo.join("README.md"), b"repo").unwrap();

    let browse = request(
        &test.app,
        Method::POST,
        "/api/projects/directory",
        Some(json!({
            "device_id": local_device_id,
            "path": repo,
        })),
    )
    .await;
    assert_eq!(browse.status(), StatusCode::OK);
    let browse = response_json(browse).await;
    assert_eq!(browse["proposed_name"], "repo");
    assert_eq!(browse["entries"][0]["name"], "package");
    assert_eq!(browse["entries"][0]["kind"], "directory");

    run_git(&repo, &["init", "-b", "main"]);
    run_git(&repo, &["config", "user.email", "test@example.com"]);
    run_git(&repo, &["config", "user.name", "Test"]);
    run_git(&repo, &["add", "README.md"]);
    run_git(&repo, &["commit", "-m", "initial"]);
    std::fs::write(repo.join("README.md"), b"changed").unwrap();

    let removed_path_field = request(
        &test.app,
        Method::POST,
        "/api/projects",
        Some(json!({
            "resource_device_id": local_device_id,
            "path": repo,
        })),
    )
    .await;
    assert_eq!(
        removed_path_field.status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert!(test.state.project_catalog.list().unwrap().is_empty());

    let create = request(
        &test.app,
        Method::POST,
        "/api/projects",
        Some(json!({
            "directories": [{"device_id": local_device_id, "resource_root": repo}],
            "display_name": "Main repo",
        })),
    )
    .await;
    assert_eq!(create.status(), StatusCode::CREATED);
    let create = response_json(create).await;
    let project_id = create["project"]["project_id"].as_str().unwrap();
    assert_eq!(create["project"]["display_name"], "Main repo");
    assert!(create["project"].get("resource_root").is_none());
    assert!(create["overlap_warnings"].as_array().unwrap().is_empty());

    let list = request(&test.app, Method::GET, "/api/projects", None).await;
    assert_eq!(list.status(), StatusCode::OK);
    let list = response_json(list).await;
    assert_eq!(list["projects"].as_array().unwrap().len(), 1);
    assert!(list["projects"][0].get("availability").is_none());

    let detail = request(
        &test.app,
        Method::GET,
        &format!("/api/projects/{project_id}"),
        None,
    )
    .await;
    assert_eq!(detail.status(), StatusCode::OK);
    assert_eq!(
        response_json(detail).await["display_name"],
        json!("Main repo")
    );

    let rename = request(
        &test.app,
        Method::PATCH,
        &format!("/api/projects/{project_id}"),
        Some(json!({"display_name": "Renamed repo"})),
    )
    .await;
    assert_eq!(rename.status(), StatusCode::OK);
    assert_eq!(
        response_json(rename).await["display_name"],
        json!("Renamed repo")
    );
    let descriptor = test
        .state
        .project_catalog
        .get(&ProjectId::new(project_id))
        .unwrap()
        .unwrap();
    assert_eq!(descriptor.resources.directories[0].resource_root, repo);

    let git = request(
        &test.app,
        Method::GET,
        &format!(
            "/api/projects/{project_id}/workspace/git?{}",
            resource_query(&descriptor.resources.directories[0].device_id, &repo, None)
        ),
        None,
    )
    .await;
    assert_eq!(git.status(), StatusCode::OK);
    let git = response_json(git).await;
    assert_eq!(git["repository"], true);
    assert_eq!(git["branch"], "main");
    assert_eq!(git["head"]["summary"], "initial");
    assert_eq!(git["changes"][0]["path"], "README.md");
    assert_eq!(git["changes"][0]["status"], "M");

    let duplicate = request(
        &test.app,
        Method::POST,
        "/api/projects",
        Some(json!({
            "directories": [{"device_id": local_device_id, "resource_root": repo}],
            "display_name": "Duplicate",
        })),
    )
    .await;
    assert_eq!(duplicate.status(), StatusCode::CREATED);
    let duplicate = response_json(duplicate).await;
    assert_ne!(duplicate["project"]["project_id"], project_id);
    assert_eq!(duplicate["overlap_warnings"][0]["project_id"], project_id);
    assert_eq!(
        duplicate["overlap_warnings"][0]["relation"],
        "shared_directory"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn project_list_activity_comes_from_chat_index_without_reading_history() {
    let test = build_app();
    let project = ProjectDescriptor {
        execution_default: None,
        project_id: ProjectId::new("activity-project"),
        display_name: "Activity Project".to_string(),
        lifecycle: ProjectLifecycle::Active,
        resources: rua_project::ProjectResources::create(vec![
            rua_project::ProjectResourceDirectoryInput::new(
                "1111111111111111",
                "/remote/display/path",
            ),
        ])
        .unwrap(),
        created_at: "2026-09-21T00:00:00Z".parse().unwrap(),
        updated_at: "2026-09-22T00:00:00Z".parse().unwrap(),
        extra: BTreeMap::new(),
    };
    test.state
        .project_catalog
        .apply_descriptor(project.clone())
        .unwrap();
    let scope = ChatStore::resolve(
        &test.state.rua_root,
        ChatAddress::new("00000000-0000-4000-8000-000000000812"),
    );

    let list = request(&test.app, Method::GET, "/api/projects", None).await;
    assert_eq!(list.status(), StatusCode::OK);
    let list = response_json(list).await;
    assert_eq!(list["projects"][0]["project_id"], "activity-project");
    assert_eq!(
        list["projects"][0].get("last_activity_at"),
        Some(&serde_json::Value::Null)
    );
    assert!(
        !scope.root().exists(),
        "listing descriptors must not materialize remote workspaces"
    );

    let activity_at = "2026-09-23T00:00:00Z".parse().unwrap();
    let chat = rua_session::chat_index::ChatIndexRecord {
        id: scope.address().chat_id().to_string(),
        name: "Synced conversation".to_string(),
        user_id: "default".to_string(),
        channel: "console".to_string(),
        created_at: project.created_at,
        updated_at: activity_at,
        meta: json!({"project_id": project.project_id}),
        status: "idle".to_string(),
        pinned: false,
        pin_order: 0,
        tag: rua_session::ChatTag::None,
        archived: false,
    };
    rua_session::chat_index::insert(
        scope.root(),
        rua_session::chat_index::ChatIndexRecord {
            id: "00000000-0000-4000-8000-000000000813".to_string(),
            pinned: true,
            updated_at: project.created_at,
            ..chat.clone()
        },
    )
    .await
    .unwrap();
    rua_session::chat_index::insert(scope.root(), chat)
        .await
        .unwrap();
    test.state
        .project_catalog
        .rename(&project.project_id, "Renamed Project")
        .unwrap();
    let list = request(&test.app, Method::GET, "/api/projects", None).await;
    assert_eq!(list.status(), StatusCode::OK);
    let list = response_json(list).await;
    assert_eq!(list["projects"][0]["display_name"], "Renamed Project");
    assert_eq!(list["projects"][0]["last_activity_at"], json!(activity_at));
    assert!(!scope.root().join("sessions").exists());
    let stored = test
        .state
        .project_catalog
        .get(&project.project_id)
        .unwrap()
        .unwrap();
    assert!(!stored.extra.contains_key("last_activity_at"));
}

fn run_git(root: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .status()
        .unwrap();
    assert!(status.success(), "git command failed: {args:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ordinary_and_project_chats_share_the_chat_id_route() {
    let test = build_app();
    let local_device_id = local_device_id(&test.state);
    let repo = test._temp.path().join("unified-chat-repo");
    std::fs::create_dir_all(&repo).unwrap();

    let project = request(
        &test.app,
        Method::POST,
        "/api/projects",
        Some(json!({
            "directories": [{"device_id": local_device_id, "resource_root": repo}],
            "display_name": "Unified Chat Project",
        })),
    )
    .await;
    assert_eq!(project.status(), StatusCode::CREATED);
    let project_id = response_json(project).await["project"]["project_id"]
        .as_str()
        .unwrap()
        .to_string();

    let agent_chat_id = "00000000-0000-4000-8000-000000000421";
    let project_chat_id = "00000000-0000-4000-8000-000000000422";
    let agent_submit = request(
        &test.app,
        Method::POST,
        &format!("/api/chats/{agent_chat_id}/console/submit"),
        Some(json!({
            "agent_id": "default",
            "chat_id": agent_chat_id,
            "input": [{
                "role": "user",
                "content": "Agent route acceptance"
            }]
        })),
    )
    .await;
    assert_eq!(agent_submit.status(), StatusCode::OK);

    let project_submit = request(
        &test.app,
        Method::POST,
        &format!("/api/chats/{project_chat_id}/console/submit"),
        Some(json!({ "project_id": project_id,
            "agent_id": "default",
            "chat_id": project_chat_id,
            "message": "Project route acceptance",
        })),
    )
    .await;
    assert_eq!(project_submit.status(), StatusCode::OK);
    let project_submit = response_json(project_submit).await;
    assert_eq!(project_submit["mode"], "started");
    assert!(project_submit["control"]["active_turn"]["turn_id"]
        .as_str()
        .is_some_and(|turn_id| !turn_id.is_empty()));
    assert_eq!(project_submit["control"]["queue"]["entries"], json!([]));
    wait_for_chat_idle(&test.state, project_chat_id).await;

    let agent_update = request(
        &test.app,
        Method::PUT,
        &format!("/api/chats/{agent_chat_id}"),
        Some(json!({
            "name": "Unified Agent Chat",
            "pinned": true,
        })),
    )
    .await;
    assert_eq!(agent_update.status(), StatusCode::OK);

    let project_update = request(
        &test.app,
        Method::PATCH,
        &format!("/api/chats/{project_chat_id}"),
        Some(json!({
            "name": "Unified Project Chat",
            "pinned": true,
        })),
    )
    .await;
    assert_eq!(project_update.status(), StatusCode::OK);

    for uri in [
        format!("/api/chats/{agent_chat_id}"),
        format!("/api/chats/{project_chat_id}"),
    ] {
        let history = request(&test.app, Method::GET, &uri, None).await;
        assert_eq!(history.status(), StatusCode::OK, "history route: {uri}");
        let history = response_json(history).await;
        assert!(!history["messages"].as_array().unwrap().is_empty());
        assert!(
            history["durable_coverage"]["latest_message_sequence"]
                .as_u64()
                .is_some(),
            "Agent and Project History must expose the same durable coverage contract: {history}"
        );
        assert!(
            history["durable_coverage"]["manifest_hash"]
                .as_str()
                .is_some_and(|hash| hash.starts_with("sha256:")),
            "History must expose the canonical manifest identity: {history}"
        );
        let turn_id = history["messages"][0]["turn_id"].as_str().unwrap();
        let located = request(
            &test.app,
            Method::GET,
            &format!("{uri}?turn_limit=1&around_turn_id={turn_id}"),
            None,
        )
        .await;
        assert_eq!(located.status(), StatusCode::OK);
        let located = response_json(located).await;
        assert_eq!(located["messages"][0]["turn_id"], turn_id);
    }

    let create_agent = request(
        &test.app,
        Method::POST,
        "/api/agents",
        Some(json!({
            "id": "unified-code",
            "name": "Unified Code Agent",
        })),
    )
    .await;
    assert_eq!(create_agent.status(), StatusCode::CREATED);
    let switch = request(
        &test.app,
        Method::PATCH,
        &format!("/api/chats/{project_chat_id}/agent-selection"),
        Some(json!({
            "expected_agent_id": "default",
            "agent_id": "unified-code",
        })),
    )
    .await;
    assert_eq!(switch.status(), StatusCode::OK);

    let agent_address = ChatAddress::new(agent_chat_id);
    let project_address = ChatAddress::new(project_chat_id);
    let agent_scope = ChatStore::resolve(&test.state.rua_root, agent_address);
    let project_scope = ChatStore::resolve(&test.state.rua_root, project_address);
    assert_eq!(agent_scope.root(), project_scope.root());

    let agent_header = rua_session::WorkspaceSessionStore::new(agent_scope.root())
        .load_chat_header(agent_chat_id)
        .await
        .unwrap()
        .unwrap();
    let project_header = rua_session::WorkspaceSessionStore::new(project_scope.root())
        .load_chat_header(project_chat_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(agent_header.agent_id, "default");
    assert!(agent_header.project_id.is_none());
    assert_eq!(
        project_header.project_id.as_deref(),
        Some(project_id.as_str())
    );
    assert_eq!(
        agent_header.extra["chat_policy"],
        json!({
            "agent_assignment": "selectable",
            "resource": "none",
            "authority_placement": "selectable",
        })
    );
    assert_eq!(project_header.agent_id, "unified-code");
    assert_eq!(
        project_header.extra["chat_policy"],
        json!({
            "agent_assignment": "selectable",
            "resource": "project_binding",
            "authority_placement": "fixed",
        })
    );

    let agent_list = request(&test.app, Method::GET, "/api/chats?agent_id=default", None).await;
    assert_eq!(agent_list.status(), StatusCode::OK);
    assert!(response_json(agent_list)
        .await
        .as_array()
        .unwrap()
        .iter()
        .any(|chat| chat["id"] == agent_chat_id && chat["name"] == "Unified Agent Chat"));

    let project_list = request(
        &test.app,
        Method::GET,
        &format!("/api/projects/{project_id}/chats"),
        None,
    )
    .await;
    assert_eq!(project_list.status(), StatusCode::OK);
    assert!(response_json(project_list)
        .await
        .as_array()
        .unwrap()
        .iter()
        .any(|chat| chat["id"] == project_chat_id && chat["name"] == "Unified Project Chat"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_project_chat_first_submit_materializes_history_by_chat_id() {
    let test = build_app();
    test.state
        .provider_cells
        .cell("fixture")
        .write(
            serde_json::from_value::<ProviderConfig>(json!({
                "id": "fixture",
                "name": "Fixture",
                "base_url": "https://example.invalid/v1",
                "api_key": "test-key",
                "chat_model": "OpenAIChatModel",
                "models": [{
                    "id": "project-first-turn-model",
                    "name": "Project First Turn Model",
                    "thinking_param_style": "budget",
                    "thinking_budget_range": [1, 8192]
                }]
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    test.state
        .provider_manager
        .reload_provider("fixture")
        .await
        .unwrap();
    let local_device_id = local_device_id(&test.state);
    let first_repo = test._temp.path().join("first-repo");
    let second_repo = test._temp.path().join("second-repo");
    std::fs::create_dir_all(&first_repo).unwrap();
    std::fs::create_dir_all(&second_repo).unwrap();

    let first = request(
        &test.app,
        Method::POST,
        "/api/projects",
        Some(json!({
            "directories": [{"device_id": local_device_id, "resource_root": first_repo}],
            "display_name": "First",
        })),
    )
    .await;
    assert_eq!(first.status(), StatusCode::CREATED);
    let first_id = response_json(first).await["project"]["project_id"]
        .as_str()
        .unwrap()
        .to_string();
    let second = request(
        &test.app,
        Method::POST,
        "/api/projects",
        Some(json!({
            "directories": [{"device_id": local_device_id, "resource_root": second_repo}],
            "display_name": "Second",
        })),
    )
    .await;
    assert_eq!(second.status(), StatusCode::CREATED);
    let second_id = response_json(second).await["project"]["project_id"]
        .as_str()
        .unwrap()
        .to_string();

    let chat_id = "00000000-0000-4000-8000-000000000403";
    let submit = request(
        &test.app,
        Method::POST,
        &format!("/api/chats/{chat_id}/console/submit"),
        Some(json!({ "project_id": first_id,
            "agent_id": "default",
            "chat_id": chat_id,
            "input": [{
                "role": "user",
                "content": [{"type": "text", "text": "Inspect this repository"}],
            }],
            "model": {
                "provider_id": "fixture",
                "model": "project-first-turn-model",
                "thinking_strength": "auto"
            },
            "stream": true,
        })),
    )
    .await;
    assert_eq!(submit.status(), StatusCode::OK);
    let submit = response_json(submit).await;
    assert_eq!(submit["accepted"], true);
    assert_eq!(submit["mode"], "started");
    assert_eq!(submit["chat_id"], chat_id);
    assert_eq!(submit["device_delivery_id"], serde_json::Value::Null);
    assert_eq!(submit["target_device_id"], serde_json::Value::Null);
    assert_eq!(submit["device_delivery_sequence"], serde_json::Value::Null);

    let history = request(
        &test.app,
        Method::GET,
        &format!("/api/chats/{chat_id}"),
        None,
    )
    .await;
    assert_eq!(history.status(), StatusCode::OK);
    let history = response_json(history).await;
    assert_eq!(history["messages"][0]["role"], "user");
    assert_eq!(
        history["messages"][0]["content"][0]["text"],
        "Inspect this repository"
    );
    assert!(history["messages"][0]["metadata"]["device_delivery_id"].is_null());
    assert!(history["messages"][0]["metadata"]["target_device_id"].is_null());
    assert_eq!(history["model"]["provider_id"], "fixture");
    assert_eq!(history["model"]["model"], "project-first-turn-model");

    let address = rua_core::ChatAddress::new(chat_id);
    let scope = rua_core::ChatStore::resolve(&test.state.rua_root, address);
    assert!(scope.chat_index_path().is_file());
    assert!(scope.evergreen_chat_root().join("session.json").is_file());
    assert!(!first_repo.join("sessions").exists());
    let header = rua_session::WorkspaceSessionStore::new(scope.root())
        .load_chat_header(chat_id)
        .await
        .expect("load Project Session File")
        .expect("Project Session File");
    assert_eq!(header.metadata.title, "Inspect this repository");
    let indexed = rua_server::services::chat_index::get(scope.root(), chat_id)
        .await
        .expect("read Project Scope Chat Index")
        .expect("indexed Project Chat");
    assert_eq!(indexed.name, "Inspect this repository");
    assert_eq!(header.agent_id, "default");
    assert_eq!(
        header.extra["chat_policy"]["agent_assignment"],
        "selectable"
    );
    assert_eq!(header.extra["chat_policy"]["resource"], "project_binding");
    assert_eq!(header.extra["chat_policy"]["authority_placement"], "fixed");
    assert_eq!(
        header.extra["authority_placement"]["device_id"],
        local_device_id
    );

    let create_agent = request(
        &test.app,
        Method::POST,
        "/api/agents",
        Some(json!({
            "id": "code",
            "name": "Code Agent",
        })),
    )
    .await;
    assert_eq!(create_agent.status(), StatusCode::CREATED);
    wait_for_chat_idle(&test.state, chat_id).await;
    let switch = request(
        &test.app,
        Method::PATCH,
        &format!("/api/chats/{chat_id}/agent-selection"),
        Some(json!({
            "expected_agent_id": "default",
            "agent_id": "code",
        })),
    )
    .await;
    assert_eq!(switch.status(), StatusCode::OK);
    let switch = response_json(switch).await;
    assert_eq!(switch["selection"]["agent_id"], "code");
    assert_eq!(switch["selection"]["revision"], 2);
    let switched_header = rua_session::WorkspaceSessionStore::new(scope.root())
        .load_chat_header(chat_id)
        .await
        .expect("load switched Project Session File")
        .expect("switched Project Session File");
    assert_eq!(switched_header.agent_id, "code");
    assert_eq!(
        switched_header.extra["chat_policy"]["agent_assignment"],
        "selectable"
    );
    assert_eq!(scope.address().chat_id().as_str(), chat_id);
    let index_before_model_update = rua_session::chat_index::get(scope.root(), chat_id)
        .await
        .expect("read Project Chat Index")
        .expect("Project Chat Index row");
    let model_update = request(
        &test.app,
        Method::PATCH,
        &format!("/api/chats/{chat_id}"),
        Some(json!({
            "model": {
                "provider_id": "fixture",
                "model": "project-first-turn-model",
                "thinking_strength": "high"
            }
        })),
    )
    .await;
    let model_update_status = model_update.status();
    let model_update_body = response_json(model_update).await;
    assert_eq!(
        model_update_status,
        StatusCode::OK,
        "model update response: {model_update_body}"
    );
    let index_after_model_update = rua_session::chat_index::get(scope.root(), chat_id)
        .await
        .expect("read unchanged Project Chat Index")
        .expect("Project Chat Index row");
    assert_eq!(index_after_model_update, index_before_model_update);

    let second_submit = request(
        &test.app,
        Method::POST,
        &format!("/api/chats/{chat_id}/console/submit"),
        Some(json!({ "project_id": first_id,
            "agent_id": "default",
            "chat_id": chat_id,
            "message": "Use the durable Project Chat Agent selection",
        })),
    )
    .await;
    assert_eq!(second_submit.status(), StatusCode::OK);
    wait_for_chat_idle(&test.state, chat_id).await;
    let turns = rua_session::WorkspaceSessionStore::new(scope.root())
        .read_turn_range(chat_id, 1, usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        turns[0].metadata["turn_agent_assignment"]["agent_id"],
        "default"
    );
    assert_eq!(
        turns[0].metadata[rua_chat::TURN_MODEL_KEY]["model"],
        "project-first-turn-model"
    );
    assert_eq!(
        turns[0].metadata[rua_chat::TURN_THINKING_KEY]["thinking_strength"],
        "auto"
    );
    assert_eq!(
        turns[1].metadata["turn_agent_assignment"]["agent_id"],
        "code"
    );
    assert_eq!(
        turns[1].metadata[rua_chat::TURN_MODEL_KEY]["model"],
        "project-first-turn-model"
    );
    assert_eq!(
        turns[1].metadata[rua_chat::TURN_THINKING_KEY]["thinking_strength"],
        "high"
    );
    let switched_history = request(
        &test.app,
        Method::GET,
        &format!("/api/chats/{chat_id}"),
        None,
    )
    .await;
    assert_eq!(
        response_json(switched_history).await["meta"]["selected_agent_id"],
        "code"
    );
    let stale_switch = request(
        &test.app,
        Method::PATCH,
        &format!("/api/chats/{chat_id}/agent-selection"),
        Some(json!({
            "expected_agent_id": "default",
            "agent_id": "default",
        })),
    )
    .await;
    assert_eq!(stale_switch.status(), StatusCode::CONFLICT);
    let disable = request(
        &test.app,
        Method::PATCH,
        "/api/agents/code/toggle",
        Some(json!({"enabled": false})),
    )
    .await;
    assert_eq!(disable.status(), StatusCode::OK);
    let unavailable_history = request(
        &test.app,
        Method::GET,
        &format!("/api/chats/{chat_id}"),
        None,
    )
    .await;
    assert_eq!(
        response_json(unavailable_history).await["meta"]["chat_agent_state"]
            ["selected_agent_available"],
        false
    );
    let unavailable_submit = request(
        &test.app,
        Method::POST,
        &format!("/api/chats/{chat_id}/console/submit"),
        Some(json!({ "project_id": first_id,
            "agent_id": "default",
            "chat_id": chat_id,
            "message": "Do not fall back silently",
        })),
    )
    .await;
    assert_eq!(unavailable_submit.status(), StatusCode::OK);
    assert_eq!(response_json(unavailable_submit).await["mode"], "rejected");
    let replace = request(
        &test.app,
        Method::PATCH,
        &format!("/api/chats/{chat_id}/agent-selection"),
        Some(json!({
            "expected_agent_id": "code",
            "agent_id": "default",
        })),
    )
    .await;
    assert_eq!(replace.status(), StatusCode::OK);
    let enable = request(
        &test.app,
        Method::PATCH,
        "/api/agents/code/toggle",
        Some(json!({"enabled": true})),
    )
    .await;
    assert_eq!(enable.status(), StatusCode::OK);
    let select_for_delete = request(
        &test.app,
        Method::PATCH,
        &format!("/api/chats/{chat_id}/agent-selection"),
        Some(json!({
            "expected_agent_id": "default",
            "agent_id": "code",
        })),
    )
    .await;
    assert_eq!(select_for_delete.status(), StatusCode::OK);
    let delete_address = rua_core::ChatAddress::new(chat_id);
    let delete_placement =
        rua_server::services::conversation_authority_target::load_chat_authority_placement_target(
            scope.root(),
            chat_id,
        )
        .await
        .unwrap();
    test.state
        .conversation_authority
        .start_active_turn(
            rua_server::services::conversation_authority::ConversationActiveTurnStart {
                address: delete_address.clone(),
                driver: rua_server::services::conversation_authority::ConversationDriverKind::Rua,
                authority_device_id: delete_placement.device_id,
                authority_epoch: delete_placement.authority_epoch,
                driver_detail: None,
            },
        )
        .await;
    let blocked_history = request(
        &test.app,
        Method::GET,
        &format!("/api/chats/{chat_id}"),
        None,
    )
    .await;
    let blocked_history = response_json(blocked_history).await;
    assert_eq!(
        blocked_history["meta"]["chat_agent_state"]["blockers"],
        json!(["active_turn"])
    );
    let authority = std::sync::Arc::clone(&test.state.conversation_authority);
    let finish_address = delete_address.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        authority.finish_active_turn(&finish_address).await;
    });
    let delete_agent = request(&test.app, Method::DELETE, "/api/agents/code", None).await;
    assert_eq!(delete_agent.status(), StatusCode::OK);
    assert!(test
        .state
        .conversation_authority
        .active_turn(&delete_address)
        .await
        .is_none());
    let history_after_delete = request(
        &test.app,
        Method::GET,
        &format!("/api/chats/{chat_id}"),
        None,
    )
    .await;
    let history_after_delete = response_json(history_after_delete).await;
    assert_eq!(
        history_after_delete["meta"]["chat_agent_state"]["selection"]["agent_id"],
        "code"
    );
    assert_eq!(
        history_after_delete["meta"]["chat_agent_state"]["selected_agent_available"],
        false
    );
    assert!(!history_after_delete["messages"]
        .as_array()
        .unwrap()
        .is_empty());

    std::fs::remove_dir_all(&first_repo).unwrap();
    let offline_history = request(
        &test.app,
        Method::GET,
        &format!("/api/chats/{chat_id}"),
        None,
    )
    .await;
    assert_eq!(offline_history.status(), StatusCode::OK);
    assert_eq!(
        response_json(offline_history).await["messages"][0]["role"],
        "user"
    );
    let new_chat_id = "00000000-0000-4000-8000-000000000406";
    let submit = request(
        &test.app,
        Method::POST,
        &format!("/api/chats/{new_chat_id}/console/submit"),
        Some(json!({ "project_id": first_id,
            "agent_id": "default",
            "chat_id": new_chat_id,
            "message": "Continue while the resource is unavailable",
        })),
    )
    .await;
    assert_eq!(submit.status(), StatusCode::OK);
    assert_eq!(response_json(submit).await["mode"], "started");
    let new_chat = request(
        &test.app,
        Method::GET,
        &format!("/api/chats/{new_chat_id}"),
        None,
    )
    .await;
    assert_eq!(new_chat.status(), StatusCode::OK);
    assert_eq!(response_json(new_chat).await["project_id"], first_id);
    assert!(!first_repo.exists());

    let lazy_chat_id = "00000000-0000-4000-8000-000000000407";
    for suffix in ["", "/approvals/snapshot", "/acp/permissions"] {
        let response = request(
            &test.app,
            Method::GET,
            &format!("/api/chats/{lazy_chat_id}{suffix}"),
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    let with_other_project = request(
        &test.app,
        Method::GET,
        &format!("/api/chats/{chat_id}?project_id={second_id}"),
        None,
    )
    .await;
    assert_eq!(with_other_project.status(), StatusCode::OK);
    assert_eq!(
        response_json(with_other_project).await["project_id"],
        first_id
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_project_chat_first_submit_without_model_persists_default_model() {
    let test = build_app();
    test.state
        .provider_cells
        .cell("fixture")
        .write(
            serde_json::from_value::<ProviderConfig>(json!({
                "id": "fixture",
                "name": "Fixture",
                "base_url": "https://example.invalid/v1",
                "api_key": "test-key",
                "chat_model": "OpenAIChatModel",
                "models": [{
                    "id": "project-default-model",
                    "name": "Project Default Model"
                }]
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    test.state
        .provider_manager
        .reload_provider("fixture")
        .await
        .unwrap();
    test.state
        .provider_manager
        .store_default_model(DefaultModel {
            provider_id: "fixture".to_string(),
            model: "project-default-model".to_string(),
            source: None,
            extra: Default::default(),
        })
        .await;

    let repo = test._temp.path().join("project-default-model");
    std::fs::create_dir_all(&repo).unwrap();
    let create = request(
        &test.app,
        Method::POST,
        "/api/projects",
        Some(json!({
            "directories": [{"device_id": local_device_id(&test.state), "resource_root": repo}],
            "display_name": "Default Model Project",
        })),
    )
    .await;
    assert_eq!(create.status(), StatusCode::CREATED);
    let project_id = response_json(create).await["project"]["project_id"]
        .as_str()
        .unwrap()
        .to_string();
    let chat_id = "00000000-0000-4000-8000-000000000404";

    let submit = request(
        &test.app,
        Method::POST,
        &format!("/api/chats/{chat_id}/console/submit"),
        Some(json!({ "project_id": project_id,
            "agent_id": "default",
            "chat_id": chat_id,
            "input": [{
                "role": "user",
                "content": [{"type": "text", "text": "Use the default model"}],
            }],
            "stream": true,
        })),
    )
    .await;
    assert_eq!(submit.status(), StatusCode::OK);

    let history = request(
        &test.app,
        Method::GET,
        &format!("/api/chats/{chat_id}"),
        None,
    )
    .await;
    assert_eq!(history.status(), StatusCode::OK);
    let history = response_json(history).await;
    assert_eq!(history["model"]["provider_id"], "fixture");
    assert_eq!(history["model"]["model"], "project-default-model");
    assert_eq!(history["model"]["thinking_strength"], "auto");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn chat_deletion_requires_saved_membership_before_removing_a_projection() {
    let test = build_app();
    let repo = test._temp.path().join("index-only-project");
    std::fs::create_dir_all(&repo).unwrap();
    let create = request(
        &test.app,
        Method::POST,
        "/api/projects",
        Some(json!({
            "directories": [{"device_id": local_device_id(&test.state), "resource_root": repo}],
            "display_name": "Index-only",
        })),
    )
    .await;
    assert_eq!(create.status(), StatusCode::CREATED);
    let project_id = response_json(create).await["project"]["project_id"]
        .as_str()
        .unwrap()
        .to_string();
    let chat_id = "00000000-0000-4000-8000-000000000414";
    let path = format!("/api/chats/{chat_id}");
    let submit = request(
        &test.app,
        Method::POST,
        &format!("{path}/console/submit"),
        Some(json!({"project_id": project_id, "agent_id": "default", "chat_id": chat_id, "message": "Disposable"})),
    )
    .await;
    assert_eq!(submit.status(), StatusCode::OK);
    wait_for_chat_idle(&test.state, chat_id).await;
    let address = ChatAddress::new(chat_id);
    let scope = ChatStore::resolve(&test.state.rua_root, address.clone());
    rua_session::WorkspaceSessionStore::new(scope.root())
        .delete_chat_storage_by_chat_id(chat_id)
        .await
        .unwrap();
    let list = request(
        &test.app,
        Method::GET,
        &format!("/api/projects/{project_id}/chats"),
        None,
    )
    .await;
    assert_eq!(response_json(list).await[0]["id"], chat_id);
    let deleted = request(&test.app, Method::DELETE, &path, None).await;
    assert_eq!(deleted.status(), StatusCode::NOT_FOUND);
    assert!(rua_server::services::chat_index::get(scope.root(), chat_id)
        .await
        .unwrap()
        .is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn project_acp_collaboration_can_be_force_deleted_without_changing_executor() {
    let test = build_app();
    let repo = test._temp.path().join("collaboration-delete-project");
    std::fs::create_dir_all(&repo).unwrap();
    let create = request(
        &test.app,
        Method::POST,
        "/api/projects",
        Some(json!({
            "directories": [{"device_id": local_device_id(&test.state), "resource_root": repo}],
            "display_name": "Collaboration deletion",
        })),
    )
    .await;
    assert_eq!(create.status(), StatusCode::CREATED);
    let project_id = response_json(create).await["project"]["project_id"]
        .as_str()
        .unwrap()
        .to_string();
    let chat_id = "00000000-0000-4000-8000-000000000415";
    let path = format!("/api/chats/{chat_id}");
    let submit = request(
        &test.app, Method::POST, &format!("{path}/console/submit"),
        Some(json!({"project_id": project_id, "agent_id": "default", "chat_id": chat_id, "message": "Disposable collaboration"})),
    ).await;
    assert_eq!(submit.status(), StatusCode::OK);
    wait_for_chat_idle(&test.state, chat_id).await;
    let address = ChatAddress::new(chat_id);
    let scope = ChatStore::resolve(&test.state.rua_root, address.clone());
    let store = rua_session::WorkspaceSessionStore::new(scope.root());
    store
        .update_chat_header(chat_id, |header| {
            header.extra.insert(
                "collaboration".into(),
                json!({
                    "source": ChatAddress::new("missing-source"),
                    "executor": {"kind": "acp", "runner": "claude"},
                    "executor_agent_id": "default",
                }),
            );
            header.extra.insert(
                "acp_binding".into(),
                json!({"kind": "acp_passthrough", "runner": "claude"}),
            );
        })
        .await
        .unwrap();
    let rejected = request(&test.app, Method::DELETE, &path, None).await;
    assert_eq!(rejected.status(), StatusCode::CONFLICT);
    assert_eq!(response_json(rejected).await["code"], "project_chat_busy");
    let exit = test
        .state
        .conversation_authority
        .exit_acp(None, scope.root(), &address, "default")
        .await;
    assert!(
        exit.is_err(),
        "Collaboration executor must remain fixed until deletion"
    );
    let deleted = request(
        &test.app,
        Method::DELETE,
        &format!("{path}?force=true"),
        None,
    )
    .await;
    let status = deleted.status();
    let body = response_json(deleted).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["was_busy"], true);
    assert!(store.load_chat_header(chat_id).await.unwrap().is_none());
    assert!(rua_server::services::chat_index::get(scope.root(), chat_id)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn project_chat_list_update_delete_and_busy_isolation_contract() {
    let test = build_app();
    let local_device_id = local_device_id(&test.state);
    let repo = test._temp.path().join("managed-project");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::write(
        repo.join("KEEP.md"),
        b"Project Chat delete must not touch this.\n",
    )
    .unwrap();
    let repository_before = snapshot_directory(&repo);
    let create = request(
        &test.app,
        Method::POST,
        "/api/projects",
        Some(json!({
            "directories": [{"device_id": local_device_id, "resource_root": repo}],
            "display_name": "Managed",
        })),
    )
    .await;
    let project_id = response_json(create).await["project"]["project_id"]
        .as_str()
        .unwrap()
        .to_string();
    let chat_ids = [
        "00000000-0000-4000-8000-000000000411",
        "00000000-0000-4000-8000-000000000412",
        "00000000-0000-4000-8000-000000000413",
    ];
    for (chat_id, message) in chat_ids
        .iter()
        .zip(["Alpha task", "Beta task", "Disposable task"])
    {
        let submit = request(
            &test.app,
            Method::POST,
            &format!("/api/chats/{chat_id}/console/submit"),
            Some(json!({ "project_id": project_id,
                "agent_id": "default",
                "chat_id": chat_id,
                "message": message,
            })),
        )
        .await;
        assert_eq!(submit.status(), StatusCode::OK);
        wait_for_chat_idle(&test.state, chat_id).await;
    }

    let rename = request(
        &test.app,
        Method::PATCH,
        &format!("/api/chats/{}", chat_ids[0]),
        Some(json!({
            "name": "Alpha renamed",
            "pinned": true,
            "pin_order": 0,
            "collaboration_mode": "execute",
        })),
    )
    .await;
    assert_eq!(rename.status(), StatusCode::OK);
    let rename = response_json(rename).await;
    assert_eq!(rename["name"], "Alpha renamed");
    assert_eq!(rename["pinned"], true);
    assert_eq!(rename["collaboration_mode"], "execute");

    let list = request(
        &test.app,
        Method::GET,
        &format!("/api/projects/{project_id}/chats?search=alpha&offset=0&limit=1"),
        None,
    )
    .await;
    assert_eq!(list.status(), StatusCode::OK);
    assert_eq!(list.headers()["X-Total-Count"], "1");
    let list = response_json(list).await;
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(list[0]["id"], chat_ids[0]);
    assert_eq!(list[0]["name"], "Alpha renamed");

    let archive_path = format!("/api/chats/{}", chat_ids[0]);
    let archive = request(
        &test.app,
        Method::PATCH,
        &archive_path,
        Some(json!({"archived": true, "pinned": false})),
    )
    .await;
    assert_eq!(archive.status(), StatusCode::OK);
    assert_eq!(response_json(archive).await["archived"], true);
    let active = request(
        &test.app,
        Method::GET,
        &format!("/api/projects/{project_id}/chats"),
        None,
    )
    .await;
    assert!(response_json(active)
        .await
        .as_array()
        .unwrap()
        .iter()
        .all(|chat| chat["id"] != chat_ids[0]));
    let archived = request(
        &test.app,
        Method::GET,
        &format!("/api/projects/{project_id}/chats?archived=true"),
        None,
    )
    .await;
    let archived = response_json(archived).await;
    assert_eq!(archived.as_array().unwrap().len(), 1);
    assert_eq!(archived[0]["id"], chat_ids[0]);
    let hidden_search = request(
        &test.app,
        Method::GET,
        &format!("/api/projects/{project_id}/chats/search?q=Alpha%20task"),
        None,
    )
    .await;
    assert_eq!(response_json(hidden_search).await["total"], 0);
    let restore = request(
        &test.app,
        Method::PATCH,
        &archive_path,
        Some(json!({"archived": false, "pinned": true})),
    )
    .await;
    assert_eq!(restore.status(), StatusCode::OK);
    assert_eq!(response_json(restore).await["archived"], false);
    let clear = request(
        &test.app,
        Method::POST,
        &format!("/api/projects/{project_id}/chats/delete-archived"),
        None,
    )
    .await;
    assert_eq!(clear.status(), StatusCode::OK);
    assert_eq!(response_json(clear).await["deleted"], json!([]));

    let search = request(
        &test.app,
        Method::GET,
        &format!("/api/projects/{project_id}/chats/search?q=Beta%20task&limit=50"),
        None,
    )
    .await;
    assert_eq!(search.status(), StatusCode::OK);
    let search = response_json(search).await;
    assert_eq!(search["total"], 1);
    assert_eq!(search["results"][0]["chat_id"], chat_ids[1]);
    assert!(search["results"][0]["snippet"]
        .as_str()
        .unwrap()
        .contains("Beta task"));

    let unpin = request(
        &test.app,
        Method::PATCH,
        &format!("/api/chats/{}", chat_ids[0]),
        Some(json!({
            "name": "Alpha renamed",
            "pinned": false,
            "collaboration_mode": "execute",
        })),
    )
    .await;
    assert_eq!(unpin.status(), StatusCode::OK);

    let idle_address = ChatAddress::new(chat_ids[2]);
    let idle_approval = test
        .state
        .security
        .approval_authority()
        .create_request(ApprovalRequestInput {
            agent_id: "default".to_string(),
            owner: rua_security::ApprovalOwner::chat(idle_address.clone()),
            turn_id: "turn-idle-delete".to_string(),
            tool_call_id: "call-idle-delete".to_string(),
            tool_name: "bash".to_string(),
            trigger: ApprovalTrigger::approval_mode("approval required"),
            tool_call: ToolCallSnapshot {
                display: "bash pwd".to_string(),
                args: json!({"command": "pwd"}),
            },
        })
        .await;
    test.state
        .security
        .approval_authority()
        .decide(
            &idle_approval.id,
            ApprovalDecision::AllowChat {
                decided_by: "test".to_string(),
                matcher: ApprovalReuseMatcher::ToolCall {
                    tool_name: "bash".to_string(),
                    input_params: Some("pwd".to_string()),
                },
            },
        )
        .await
        .unwrap();
    assert!(test
        .state
        .security
        .approval_authority()
        .find_reuse_rule_at("default", &idle_address, "bash", &json!({"command": "pwd"}),)
        .await
        .is_some());

    let archived_idle = request(
        &test.app,
        Method::PATCH,
        &format!("/api/chats/{}", chat_ids[2]),
        Some(json!({"archived": true})),
    )
    .await;
    assert_eq!(archived_idle.status(), StatusCode::OK);
    let idle_delete = request(
        &test.app,
        Method::POST,
        &format!("/api/projects/{project_id}/chats/delete-archived"),
        None,
    )
    .await;
    assert_eq!(idle_delete.status(), StatusCode::OK);
    let cleared = response_json(idle_delete).await;
    assert_eq!(cleared["deleted"], json!([chat_ids[2]]));
    assert_eq!(cleared["skipped"], json!([]));
    assert!(test
        .state
        .security
        .approval_authority()
        .find_reuse_rule_at("default", &idle_address, "bash", &json!({"command": "pwd"}),)
        .await
        .is_none());
    let scope = ChatStore::resolve(&test.state.rua_root, idle_address);
    assert!(
        rua_server::services::chat_index::get(scope.root(), chat_ids[2])
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(snapshot_directory(&repo), repository_before);

    let first_address = ChatAddress::new(chat_ids[0]);
    let second_address = ChatAddress::new(chat_ids[1]);
    let first_scope = ChatStore::resolve(&test.state.rua_root, first_address.clone());
    let first_placement =
        rua_server::services::conversation_authority_target::load_chat_authority_placement_target(
            first_scope.root(),
            chat_ids[0],
        )
        .await
        .unwrap();
    let second_scope = ChatStore::resolve(&test.state.rua_root, second_address.clone());
    let second_placement =
        rua_server::services::conversation_authority_target::load_chat_authority_placement_target(
            second_scope.root(),
            chat_ids[1],
        )
        .await
        .unwrap();
    for (address, placement) in [
        (first_address.clone(), first_placement),
        (second_address.clone(), second_placement),
    ] {
        test.state
            .conversation_authority
            .start_active_turn(
                rua_server::services::conversation_authority::ConversationActiveTurnStart {
                    address,
                    driver:
                        rua_server::services::conversation_authority::ConversationDriverKind::Rua,
                    authority_device_id: placement.device_id,
                    authority_epoch: placement.authority_epoch,
                    driver_detail: None,
                },
            )
            .await;
    }
    assert!(test
        .state
        .conversation_authority
        .active_turn(&first_address)
        .await
        .is_some());
    assert!(test
        .state
        .conversation_authority
        .active_turn(&second_address)
        .await
        .is_some());

    let busy_delete = request(
        &test.app,
        Method::DELETE,
        &format!("/api/chats/{}", chat_ids[0]),
        None,
    )
    .await;
    assert_eq!(busy_delete.status(), StatusCode::CONFLICT);
    let busy_body = response_json(busy_delete).await;
    assert!(
        busy_body["code"] == "project_chat_busy"
            || busy_body["detail"]
                .as_str()
                .is_some_and(|detail| detail.contains("busy")),
        "unexpected busy delete response: {busy_body}"
    );

    let authority = std::sync::Arc::clone(&test.state.conversation_authority);
    let finish_first = first_address.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        authority.finish_active_turn(&finish_first).await;
    });
    let force_delete = request(
        &test.app,
        Method::DELETE,
        &format!("/api/chats/{}?force=true", chat_ids[0]),
        None,
    )
    .await;
    assert_eq!(force_delete.status(), StatusCode::OK);
    assert_eq!(response_json(force_delete).await["was_busy"], true);
    assert!(test
        .state
        .conversation_authority
        .active_turn(&second_address)
        .await
        .is_some());
    assert!(
        rua_server::services::chat_index::get(first_scope.root(), chat_ids[0])
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        rua_server::services::chat_index::get(second_scope.root(), chat_ids[1])
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(snapshot_directory(&repo), repository_before);

    std::fs::remove_dir_all(&repo).unwrap();
    let offline_delete = request(
        &test.app,
        Method::DELETE,
        &format!("/api/chats/{}?force=true", chat_ids[1]),
        None,
    )
    .await;
    let offline_status = offline_delete.status();
    let offline_body = response_json(offline_delete).await;
    assert_eq!(
        offline_status,
        StatusCode::OK,
        "unexpected local Project Chat delete response: {offline_body}"
    );
    assert!(
        rua_server::services::chat_index::get(second_scope.root(), chat_ids[1])
            .await
            .unwrap()
            .is_none()
    );
    assert!(test
        .state
        .conversation_authority
        .active_turn(&second_address)
        .await
        .is_none());
    let intent = rua_server::services::chat_deletion::ChatDeleteIntentStore::from_rua_root(
        &test.state.rua_root,
    )
    .load(&second_address)
    .unwrap()
    .expect("Project Chat deletion must persist a local Delete Intent");
    assert_eq!(intent.address, second_address);
    assert!(test
        .state
        .project_catalog
        .get(&ProjectId::new(&project_id))
        .unwrap()
        .is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn project_chat_control_routes_share_fixed_chat_authority() {
    let test = build_app();
    test.state
        .provider_cells
        .cell("fixture")
        .write(
            serde_json::from_value::<ProviderConfig>(json!({
                "id": "fixture",
                "name": "Fixture",
                "base_url": "https://example.invalid/v1",
                "api_key": "test-key",
                "chat_model": "OpenAIChatModel",
                "models": [{
                    "id": "project-control-model",
                    "name": "Project Control Model"
                }]
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    test.state
        .provider_manager
        .reload_provider("fixture")
        .await
        .unwrap();
    let local_device_id = local_device_id(&test.state);
    let repo = test._temp.path().join("controlled-project");
    std::fs::create_dir_all(&repo).unwrap();
    let create = request(
        &test.app,
        Method::POST,
        "/api/projects",
        Some(json!({
            "directories": [{"device_id": local_device_id, "resource_root": repo}],
            "display_name": "Controlled",
        })),
    )
    .await;
    let project_id = response_json(create).await["project"]["project_id"]
        .as_str()
        .unwrap()
        .to_string();
    let removed_project_options = request(
        &test.app,
        Method::GET,
        &format!("/api/projects/{project_id}/running-config"),
        None,
    )
    .await;
    assert_eq!(removed_project_options.status(), StatusCode::NOT_FOUND);

    let chat_id = "00000000-0000-4000-8000-000000000421";
    let submit = request(
        &test.app,
        Method::POST,
        &format!("/api/chats/{chat_id}/console/submit"),
        Some(json!({ "project_id": project_id,
            "agent_id": "default",
            "chat_id": chat_id,
            "message": "prepare controls",
        })),
    )
    .await;
    assert_eq!(submit.status(), StatusCode::OK);
    wait_for_chat_idle(&test.state, chat_id).await;

    let options_path = format!("/api/chats/{chat_id}/running-config");
    let options = request(&test.app, Method::GET, &options_path, None).await;
    assert_eq!(options.status(), StatusCode::OK);
    let mut options = response_json(options).await;
    options["max_input_length"] = json!(12_345);
    options["context_compact"]["active_work_plan_injection_enabled"] = json!(true);
    let update_options = request(&test.app, Method::PUT, &options_path, Some(options)).await;
    assert_eq!(update_options.status(), StatusCode::OK);
    let updated_options = response_json(update_options).await;
    assert_eq!(updated_options["max_input_length"], 12_345);
    assert_eq!(
        updated_options["context_compact"]["active_work_plan_injection_enabled"],
        true
    );
    let persisted_options = request(&test.app, Method::GET, &options_path, None).await;
    assert_eq!(persisted_options.status(), StatusCode::OK);
    assert_eq!(
        response_json(persisted_options).await["max_input_length"],
        12_345
    );

    let workspace = request(
        &test.app,
        Method::GET,
        &format!("/api/chats/{chat_id}/resource-policy"),
        None,
    )
    .await;
    assert_eq!(workspace.status(), StatusCode::OK);
    let workspace = response_json(workspace).await;
    assert!(workspace["policy"].is_null());
    assert_eq!(workspace["pending_approval_count"], 0);
    assert_eq!(workspace["chat_rule_count"], 0);

    let mut security_config = test.state.security.config_store().load("default").unwrap();
    security_config.extra.insert(
        rua_security::PROJECT_RESOURCE_POLICY_KEY.to_string(),
        json!({"profile": "danger-full-access"}),
    );
    test.state
        .security
        .save_and_invalidate("default", &security_config)
        .unwrap();
    let unchanged = request(
        &test.app,
        Method::GET,
        &format!("/api/chats/{chat_id}/resource-policy"),
        None,
    )
    .await;
    assert_eq!(unchanged.status(), StatusCode::OK);
    assert!(response_json(unchanged).await["policy"].is_null());
    let configuration_path = format!("/api/chats/{chat_id}/configuration");
    let preview = request(&test.app, Method::GET, &configuration_path, None).await;
    assert_eq!(preview.status(), StatusCode::OK);
    let preview = response_json(preview).await;
    let updated = request(&test.app, Method::PUT, &configuration_path, Some(json!({
        "expected_epoch": preview["current"]["epoch"], "candidate_epoch": preview["candidate"]["epoch"],
    }))).await;
    assert_eq!(updated.status(), StatusCode::OK);
    let workspace = request(
        &test.app,
        Method::GET,
        &format!("/api/chats/{chat_id}/resource-policy"),
        None,
    )
    .await;
    assert_eq!(workspace.status(), StatusCode::OK);
    let workspace = response_json(workspace).await;
    assert_eq!(workspace["policy"]["profile"], "danger-full-access");
    assert_eq!(workspace["policy"]["source"], "agent_security");
    assert_eq!(workspace["policy"]["writable_roots"], json!([]));
    assert_eq!(workspace["policy"]["tool_guard_enabled"], true);

    security_config.extra.insert(
        rua_security::PROJECT_RESOURCE_POLICY_KEY.into(),
        json!({"profile":"resource-write"}),
    );
    test.state
        .security
        .save_and_invalidate("default", &security_config)
        .unwrap();
    let preview =
        response_json(request(&test.app, Method::GET, &configuration_path, None).await).await;
    let apply = request(&test.app, Method::PUT, &configuration_path, Some(json!({
        "expected_epoch":preview["current"]["epoch"], "candidate_epoch":preview["candidate"]["epoch"],
    }))).await;
    assert_eq!(apply.status(), StatusCode::OK);
    let new_cwd = test._temp.path().join("external-work");
    std::fs::create_dir(&new_cwd).unwrap();
    let changed = request(
        &test.app,
        Method::PATCH,
        &format!("/api/chats/{chat_id}/execution-directory"),
        Some(json!({"device_id": local_device_id, "path":new_cwd})),
    )
    .await;
    assert_eq!(changed.status(), StatusCode::OK);
    let view = response_json(
        request(
            &test.app,
            Method::GET,
            &format!("/api/chats/{chat_id}/resource-policy"),
            None,
        )
        .await,
    )
    .await;
    let policy: rua_security::EffectiveProjectResourcePolicy =
        serde_json::from_value(view["policy"].clone()).unwrap();
    let managed = rua_core::RuaRoot::new(&test.state.rua_root)
        .managed_chat_directory(chat_id, &local_device_id)
        .unwrap();
    assert_eq!(Path::new(&policy.cwd), new_cwd.as_path());
    assert!(policy.permits_write(&managed.join("allowed.txt")));
    assert!(!policy.permits_write(&new_cwd.join("external.txt")));
    assert!(!policy.permits_write(&repo.join("declared-resource.txt")));

    let model = json!({
        "provider_id": "fixture",
        "model": "project-control-model",
    });
    let update_model = request(
        &test.app,
        Method::PATCH,
        &format!("/api/chats/{chat_id}"),
        Some(json!({"model": model.clone()})),
    )
    .await;
    assert_eq!(update_model.status(), StatusCode::OK);
    let updated_model = response_json(update_model).await["model"].clone();
    assert_eq!(updated_model["provider_id"], model["provider_id"]);
    assert_eq!(updated_model["model"], model["model"]);
    assert_eq!(updated_model["thinking_strength"], "auto");

    let runners = request(
        &test.app,
        Method::GET,
        &format!("/api/chats/{chat_id}/acp/runners"),
        None,
    )
    .await;
    assert_eq!(runners.status(), StatusCode::OK);
    assert!(response_json(runners).await.is_array());
    let acp_status = request(
        &test.app,
        Method::GET,
        &format!("/api/chats/{chat_id}/acp/status"),
        None,
    )
    .await;
    assert_eq!(acp_status.status(), StatusCode::OK);
    let acp_status = response_json(acp_status).await;
    assert_eq!(acp_status["active"], false);
    assert_eq!(acp_status["workspace_dir"], json!(new_cwd));
    assert_ne!(acp_status["workspace_dir"], json!(repo));

    let address = ChatAddress::new(chat_id);
    let scope = ChatStore::resolve(&test.state.rua_root, address.clone());
    let placement =
        rua_server::services::conversation_authority_target::load_chat_authority_placement_target(
            scope.root(),
            chat_id,
        )
        .await
        .unwrap();
    test.state
        .conversation_authority
        .start_active_turn(ConversationActiveTurnStart {
            address: address.clone(),
            driver: ConversationDriverKind::Rua,
            authority_device_id: placement.device_id,
            authority_epoch: placement.authority_epoch,
            driver_detail: None,
        })
        .await;
    let first = test
        .state
        .conversation_authority
        .enqueue_input(
            &address,
            ConversationInputPayload {
                message: "first queued".to_string(),
                model_override: None,
                input: None,
                messages: None,
                extra: serde_json::Map::new(),
                attachments: Vec::new(),
                enqueued_at: std::time::Instant::now(),
            },
            Some("controller".to_string()),
        )
        .await
        .unwrap();
    let second = test
        .state
        .conversation_authority
        .enqueue_input(
            &address,
            ConversationInputPayload {
                message: "second queued".to_string(),
                model_override: None,
                input: None,
                messages: None,
                extra: serde_json::Map::new(),
                attachments: Vec::new(),
                enqueued_at: std::time::Instant::now(),
            },
            Some("controller".to_string()),
        )
        .await
        .unwrap();
    let edit = request(
        &test.app,
        Method::PATCH,
        &format!("/api/chats/{chat_id}/queue/{}", first.input_id),
        Some(json!({"message": "first edited"})),
    )
    .await;
    assert_eq!(edit.status(), StatusCode::OK);
    assert_eq!(response_json(edit).await["entry"]["text"], "first edited");
    let send_now = request(
        &test.app,
        Method::POST,
        &format!("/api/chats/{chat_id}/queue/{}/send-now", first.input_id),
        Some(json!({
            "command_id": "send-now-project-control",
            "authority_epoch": placement.authority_epoch,
        })),
    )
    .await;
    assert_eq!(send_now.status(), StatusCode::OK);
    assert_eq!(response_json(send_now).await["applied"], true);
    let delete = request(
        &test.app,
        Method::DELETE,
        &format!("/api/chats/{chat_id}/queue/{}", second.input_id),
        None,
    )
    .await;
    assert_eq!(delete.status(), StatusCode::OK);
    assert_eq!(
        response_json(delete).await["deleted_input_id"],
        second.input_id
    );

    let approval = test
        .state
        .security
        .approval_authority()
        .create_request(ApprovalRequestInput {
            agent_id: "default".to_string(),
            owner: rua_security::ApprovalOwner::chat(address.clone()),
            turn_id: "turn-project-control".to_string(),
            tool_call_id: "call-project-control".to_string(),
            tool_name: "bash".to_string(),
            trigger: ApprovalTrigger::approval_mode("approval required"),
            tool_call: ToolCallSnapshot {
                display: "bash".to_string(),
                args: json!({"command": "pwd"}),
            },
        })
        .await;
    let snapshot = request(
        &test.app,
        Method::GET,
        &format!("/api/chats/{chat_id}/approvals/snapshot"),
        None,
    )
    .await;
    assert_eq!(snapshot.status(), StatusCode::OK);
    assert_eq!(
        response_json(snapshot).await["pending"][0]["id"],
        approval.id
    );
    let decide = request(
        &test.app,
        Method::POST,
        &format!("/api/chats/{chat_id}/approvals/{}/decide", approval.id),
        Some(json!({
            "decision": "run_once",
            "decided_by": "test",
        })),
    )
    .await;
    assert_eq!(decide.status(), StatusCode::OK);
    assert_eq!(response_json(decide).await["status"], "approved");

    let authority = std::sync::Arc::clone(&test.state.conversation_authority);
    let mut control_events = authority.subscribe_control_events(&address).await;
    let finishing_address = address.clone();
    let observed = test
        .state
        .conversation_authority
        .active_turn(&address)
        .await
        .unwrap();
    let stale = request(
        &test.app,
        Method::POST,
        &format!("/api/chats/{chat_id}/console/stop"),
        Some(json!({"turn_id": "old-turn", "authority_epoch": observed.authority_epoch})),
    )
    .await;
    assert_eq!(stale.status(), StatusCode::OK);
    assert_eq!(response_json(stale).await["status"], "target_changed");
    assert_eq!(
        test.state
            .conversation_authority
            .active_turn(&address)
            .await
            .unwrap()
            .turn_id,
        observed.turn_id
    );
    let finisher = tokio::spawn(async move {
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let event: serde_json::Value = serde_json::from_str(
                    &control_events
                        .recv()
                        .await
                        .expect("Project Stop control event"),
                )
                .expect("Project Stop control event JSON");
                if event["reason"] == "stop_requested" {
                    break;
                }
            }
        })
        .await
        .expect("Project Stop request must reach Conversation Authority");
        authority.finish_active_turn(&finishing_address).await;
    });
    let stop = request(
        &test.app,
        Method::POST,
        &format!("/api/chats/{chat_id}/console/stop"),
        Some(json!({"turn_id": observed.turn_id, "authority_epoch": observed.authority_epoch})),
    )
    .await;
    finisher.await.expect("Project Stop finisher");
    assert_eq!(stop.status(), StatusCode::OK);
    assert_eq!(response_json(stop).await["stopped"], true);
    assert!(!test
        .state
        .conversation_authority
        .queue_snapshot(&address)
        .await
        .is_empty());

    test.state
        .conversation_authority
        .enqueue_input(
            &address,
            ConversationInputPayload {
                message: "orphaned queued input".to_string(),
                model_override: None,
                input: None,
                messages: None,
                extra: serde_json::Map::new(),
                attachments: Vec::new(),
                enqueued_at: std::time::Instant::now(),
            },
            Some("controller".to_string()),
        )
        .await
        .unwrap();
    let idle_stop = request(
        &test.app,
        Method::POST,
        &format!("/api/chats/{chat_id}/console/stop"),
        Some(json!({"turn_id": observed.turn_id, "authority_epoch": observed.authority_epoch})),
    )
    .await;
    assert_eq!(idle_stop.status(), StatusCode::OK);
    assert_eq!(response_json(idle_stop).await["stopped"], false);
    assert!(!test
        .state
        .conversation_authority
        .queue_snapshot(&address)
        .await
        .is_empty());

    let placement_mutation = request(
        &test.app,
        Method::PUT,
        &format!("/api/chats/{chat_id}/authority-placement"),
        Some(json!({"device_id": "other-device"})),
    )
    .await;
    assert_eq!(placement_mutation.status(), StatusCode::CONFLICT);
    assert!(response_json(placement_mutation).await["detail"]
        .as_str()
        .unwrap()
        .contains("fixed on its execution Device"));
}

struct ProjectLifecycleBlockingRunner {
    started: tokio::sync::Notify,
}

#[async_trait::async_trait]
impl rua_task::TaskRunner for ProjectLifecycleBlockingRunner {
    fn kind(&self) -> rua_task::TaskRunnerKind {
        rua_task::TaskRunnerKind::Tool
    }

    async fn run(
        &self,
        request: rua_task::TaskRunRequest,
    ) -> Result<rua_task::TaskRunOutput, rua_task::TaskRunnerError> {
        self.started.notify_one();
        request.cancel.cancelled().await;
        Ok(rua_task::TaskRunOutput::cancelled())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn project_task_routes_use_typed_project_chat_ownership() {
    let test = build_app();
    let local_device_id = local_device_id(&test.state);
    let repo = test._temp.path().join("project-task-owner");
    std::fs::create_dir_all(&repo).unwrap();
    let create = request(
        &test.app,
        Method::POST,
        "/api/projects",
        Some(json!({
            "directories": [{"device_id": local_device_id, "resource_root": repo}],
            "display_name": "Project Task Owner",
        })),
    )
    .await;
    assert_eq!(create.status(), StatusCode::CREATED);
    let project_id = response_json(create).await["project"]["project_id"]
        .as_str()
        .unwrap()
        .to_string();
    let chat_id = "00000000-0000-4000-8000-000000000425";
    let submit = request(
        &test.app,
        Method::POST,
        &format!("/api/chats/{chat_id}/console/submit"),
        Some(json!({ "project_id": project_id,
            "agent_id": "default",
            "chat_id": chat_id,
            "message": "Materialize Project Task owner",
        })),
    )
    .await;
    assert_eq!(submit.status(), StatusCode::OK);
    wait_for_chat_idle(&test.state, chat_id).await;

    let owner = rua_task::TaskOwner::from_address(ChatAddress::new(chat_id));
    let runner = Arc::new(ProjectLifecycleBlockingRunner {
        started: tokio::sync::Notify::new(),
    });
    test.state.task_runtime.register_runner(runner.clone());
    let task_id = test
        .state
        .task_runtime
        .create(
            rua_task::TaskSpec {
                owner: owner.clone(),
                instruction: "Project task route".to_string(),
                runner: rua_task::TaskRunnerSpec::Tool {
                    tool_name: "Fixture_ProjectTaskRoute".to_string(),
                    arguments: json!({}),
                },
                context_refs: Vec::new(),
                depends_on: Vec::new(),
                origin: Default::default(),
            },
            rua_task::TaskAttemptAuthorization::new("controller").unwrap(),
        )
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), runner.started.notified())
        .await
        .unwrap();

    let create_agent = request(
        &test.app,
        Method::POST,
        "/api/agents",
        Some(json!({
            "id": "agent-b",
            "name": "Agent B",
        })),
    )
    .await;
    assert_eq!(create_agent.status(), StatusCode::CREATED);
    let switch = request(
        &test.app,
        Method::PATCH,
        &format!("/api/chats/{chat_id}/agent-selection"),
        Some(json!({
            "expected_agent_id": "default",
            "agent_id": "agent-b",
        })),
    )
    .await;
    assert_eq!(switch.status(), StatusCode::OK);

    let list = request(
        &test.app,
        Method::GET,
        &format!("/api/chats/{chat_id}/tasks"),
        None,
    )
    .await;
    assert_eq!(list.status(), StatusCode::OK);
    let list = response_json(list).await;
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(list[0]["task_id"], task_id);
    assert_eq!(
        list[0]["spec"]["owner"]["chat_address"],
        json!({"chat_id": chat_id})
    );
    let detail = request(
        &test.app,
        Method::GET,
        &format!("/api/chats/{chat_id}/tasks/{task_id}"),
        None,
    )
    .await;
    assert_eq!(detail.status(), StatusCode::OK);
    assert_eq!(
        response_json(detail).await["attempts"][0]["authorization_agent_id"],
        "controller"
    );

    let same_chat = request(
        &test.app,
        Method::GET,
        &format!("/api/chats/{chat_id}/tasks?agent_id=default"),
        None,
    )
    .await;
    assert_eq!(same_chat.status(), StatusCode::OK);
    assert_eq!(response_json(same_chat).await[0]["task_id"], task_id);

    test.state
        .conversation_input_intake
        .deliver_task_watch_output(rua_task::TaskWatchOutputNotification {
            notification_id: "project-task-watch-after-switch".to_string(),
            owner: owner.clone(),
            task_id: task_id.clone(),
            attempt_no: 1,
            origin_turn_id: None,
            projection: rua_task::TaskLiveOutputProjection::ModelVisible,
            records: vec![rua_task::TaskLiveOutputRecord {
                source: "stdout".to_string(),
                text: "routed-to-current-agent".to_string(),
            }],
        })
        .await
        .unwrap();
    wait_for_chat_idle(&test.state, chat_id).await;
    let address = ChatAddress::new(chat_id);
    let scope = rua_core::ChatStore::resolve(&test.state.rua_root, address);
    let store = rua_session::WorkspaceSessionStore::new(scope.root());
    let notification_turn_id = store
        .read_message_range(chat_id, 1, usize::MAX)
        .await
        .unwrap()
        .into_iter()
        .find_map(|message| {
            let note = rua_session::RuntimeNote::from_message_log_entry(&message)?;
            matches!(
                note.payload,
                rua_session::RuntimeNotePayload::TaskWatchOutput(ref output)
                    if output.task_id == task_id
            )
            .then_some(message.turn_id)
        })
        .expect("Project Task Watch notification message");
    let turns = store.read_turn_range(chat_id, 1, usize::MAX).await.unwrap();
    let notification_turn = turns
        .iter()
        .find(|turn| turn.turn_id == notification_turn_id)
        .expect("Project Task Watch notification Turn");
    assert_eq!(
        notification_turn.metadata["turn_agent_assignment"]["agent_id"],
        "agent-b"
    );

    let cancel = request(
        &test.app,
        Method::POST,
        &format!("/api/chats/{chat_id}/tasks/{task_id}/cancel"),
        None,
    )
    .await;
    assert_eq!(cancel.status(), StatusCode::OK);

    let retry = request(
        &test.app,
        Method::POST,
        &format!("/api/chats/{chat_id}/tasks/{task_id}/retry"),
        Some(json!({ "authorization_agent_id": "agent-b" })),
    )
    .await;
    assert_eq!(retry.status(), StatusCode::OK);
    let retried = response_json(retry).await;
    assert_eq!(retried["spec"]["owner"]["chat_address"]["chat_id"], chat_id);
    assert_eq!(
        retried["attempts"][0]["authorization_agent_id"],
        "controller"
    );
    assert_eq!(retried["attempts"][1]["authorization_agent_id"], "agent-b");

    let cancel_retry = request(
        &test.app,
        Method::POST,
        &format!("/api/chats/{chat_id}/tasks/{task_id}/cancel"),
        None,
    )
    .await;
    assert_eq!(cancel_retry.status(), StatusCode::OK);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn project_archive_and_reopen_ordered_hard_stop_contract() {
    let test = build_app();
    let local_device_id = local_device_id(&test.state);
    let repo = test._temp.path().join("lifecycle-project");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::write(
        repo.join("KEEP.md"),
        b"Project lifecycle must not touch this.\n",
    )
    .unwrap();
    let repository_before = snapshot_directory(&repo);
    let create = request(
        &test.app,
        Method::POST,
        "/api/projects",
        Some(json!({
            "directories": [{"device_id": local_device_id, "resource_root": repo}],
            "display_name": "Lifecycle",
        })),
    )
    .await;
    let project_id = response_json(create).await["project"]["project_id"]
        .as_str()
        .unwrap()
        .to_string();
    let project_id_typed = ProjectId::new(project_id.clone());
    let chat_id = "00000000-0000-4000-8000-000000000431";
    let submit = request(
        &test.app,
        Method::POST,
        &format!("/api/chats/{chat_id}/console/submit"),
        Some(json!({ "project_id": project_id,
            "agent_id": "default",
            "chat_id": chat_id,
            "message": "prepare lifecycle",
        })),
    )
    .await;
    assert_eq!(submit.status(), StatusCode::OK);
    wait_for_chat_idle(&test.state, chat_id).await;
    let address = ChatAddress::new(chat_id);
    let scope = ChatStore::resolve(&test.state.rua_root, address.clone());
    let placement =
        rua_server::services::conversation_authority_target::load_chat_authority_placement_target(
            scope.root(),
            chat_id,
        )
        .await
        .unwrap();

    let reusable = test
        .state
        .security
        .approval_authority()
        .create_request(ApprovalRequestInput {
            agent_id: "default".to_string(),
            owner: rua_security::ApprovalOwner::chat(address.clone()),
            turn_id: "turn-reusable".to_string(),
            tool_call_id: "call-reusable".to_string(),
            tool_name: "bash".to_string(),
            trigger: ApprovalTrigger::approval_mode("approval required"),
            tool_call: ToolCallSnapshot {
                display: "bash pwd".to_string(),
                args: json!({"command": "pwd"}),
            },
        })
        .await;
    test.state
        .security
        .approval_authority()
        .decide(
            &reusable.id,
            ApprovalDecision::AllowChat {
                decided_by: "test".to_string(),
                matcher: ApprovalReuseMatcher::ToolCall {
                    tool_name: "bash".to_string(),
                    input_params: Some("pwd".to_string()),
                },
            },
        )
        .await
        .unwrap();
    test.state
        .security
        .approval_authority()
        .create_request(ApprovalRequestInput {
            agent_id: "default".to_string(),
            owner: rua_security::ApprovalOwner::chat(address.clone()),
            turn_id: "turn-pending".to_string(),
            tool_call_id: "call-pending".to_string(),
            tool_name: "bash".to_string(),
            trigger: ApprovalTrigger::approval_mode("approval required"),
            tool_call: ToolCallSnapshot {
                display: "bash git diff".to_string(),
                args: json!({"command": "git diff"}),
            },
        })
        .await;

    test.state
        .conversation_authority
        .start_active_turn(ConversationActiveTurnStart {
            address: address.clone(),
            driver: ConversationDriverKind::Rua,
            authority_device_id: placement.device_id,
            authority_epoch: placement.authority_epoch,
            driver_detail: None,
        })
        .await;
    test.state
        .conversation_authority
        .enqueue_input(
            &address,
            ConversationInputPayload {
                message: "queued before archive".to_string(),
                model_override: None,
                input: None,
                messages: None,
                extra: serde_json::Map::new(),
                attachments: Vec::new(),
                enqueued_at: std::time::Instant::now(),
            },
            Some("controller".to_string()),
        )
        .await
        .unwrap();
    let runner = Arc::new(ProjectLifecycleBlockingRunner {
        started: tokio::sync::Notify::new(),
    });
    test.state.task_runtime.register_runner(runner.clone());
    let owner = rua_task::TaskOwner::from_address(address.clone());
    let task_id = test
        .state
        .task_runtime
        .create(
            rua_task::TaskSpec {
                owner: owner.clone(),
                instruction: "block until Archive".to_string(),
                runner: rua_task::TaskRunnerSpec::Tool {
                    tool_name: "Fixture_ProjectArchive".to_string(),
                    arguments: json!({}),
                },
                context_refs: Vec::new(),
                depends_on: Vec::new(),
                origin: Default::default(),
            },
            rua_task::TaskAttemptAuthorization::new("controller").unwrap(),
        )
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), runner.started.notified())
        .await
        .unwrap();

    let preview = request(
        &test.app,
        Method::GET,
        &format!("/api/projects/{project_id}/lifecycle-preview"),
        None,
    )
    .await;
    assert_eq!(preview.status(), StatusCode::OK);
    let preview = response_json(preview).await;
    assert_eq!(preview["impact"]["affected_chat_count"], 1);
    assert_eq!(preview["impact"]["active_turn_count"], 1);
    assert_eq!(preview["impact"]["queued_input_count"], 1);
    assert_eq!(preview["impact"]["pending_approval_count"], 1);
    assert_eq!(preview["impact"]["background_task_count"], 1);

    let unconfirmed = request(
        &test.app,
        Method::POST,
        &format!("/api/projects/{project_id}/archive"),
        Some(json!({"confirmed": false})),
    )
    .await;
    assert_eq!(unconfirmed.status(), StatusCode::BAD_REQUEST);

    test.state
        .project_catalog
        .begin_lifecycle_transition(&project_id_typed)
        .unwrap();
    let session = rua_session::WorkspaceSessionStore::new(scope.root())
        .load_chat_header(chat_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(session.project_id.as_deref(), Some(project_id.as_str()));
    let blocked_submit = request(
        &test.app,
        Method::POST,
        &format!("/api/chats/{chat_id}/console/submit"),
        Some(json!({ "project_id": project_id,
            "agent_id": "default",
            "chat_id": chat_id,
            "message": "must be blocked",
        })),
    )
    .await;
    assert_eq!(blocked_submit.status(), StatusCode::CONFLICT);
    assert!(response_json(blocked_submit).await["detail"]
        .as_str()
        .unwrap()
        .contains("Project lifecycle transition"));
    test.state
        .project_catalog
        .end_lifecycle_transition(&project_id_typed);

    let authority = Arc::clone(&test.state.conversation_authority);
    let finishing_address = address.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        authority.finish_active_turn(&finishing_address).await;
    });
    let archive = request(
        &test.app,
        Method::POST,
        &format!("/api/projects/{project_id}/archive"),
        Some(json!({"confirmed": true})),
    )
    .await;
    assert_eq!(archive.status(), StatusCode::OK);
    let archive = response_json(archive).await;
    assert_eq!(archive["project"]["lifecycle"], "archived");
    assert_eq!(archive["impact"]["affected_chat_count"], 1);
    assert_eq!(
        test.state
            .project_catalog
            .get(&project_id_typed)
            .unwrap()
            .unwrap()
            .lifecycle,
        ProjectLifecycle::Archived
    );
    assert!(test
        .state
        .conversation_authority
        .active_turn(&address)
        .await
        .is_none());
    assert!(test
        .state
        .conversation_authority
        .queue_snapshot(&address)
        .await
        .is_empty());
    assert!(test
        .state
        .security
        .approval_authority()
        .list_pending_for_address(&address)
        .await
        .is_empty());
    assert!(test
        .state
        .security
        .approval_authority()
        .find_reuse_rule_at("code", &address, "bash", &json!({"command": "pwd"}),)
        .await
        .is_some());
    assert!(test
        .state
        .task_runtime
        .get(&owner, &task_id)
        .await
        .unwrap()
        .state
        .is_terminal());
    assert_eq!(snapshot_directory(&repo), repository_before);
    let archived_history = request(
        &test.app,
        Method::GET,
        &format!("/api/chats/{chat_id}"),
        None,
    )
    .await;
    assert_eq!(archived_history.status(), StatusCode::OK);

    let archived_submit = request(
        &test.app,
        Method::POST,
        &format!("/api/chats/{chat_id}/console/submit"),
        Some(json!({ "project_id": project_id,
            "agent_id": "default",
            "chat_id": chat_id,
            "message": "must stay archived",
        })),
    )
    .await;
    assert_eq!(archived_submit.status(), StatusCode::CONFLICT);
    assert!(response_json(archived_submit).await["detail"]
        .as_str()
        .unwrap()
        .contains("Archived Project"));

    std::fs::remove_dir_all(&repo).unwrap();
    let reopened = request(
        &test.app,
        Method::POST,
        &format!("/api/projects/{project_id}/reopen"),
        None,
    )
    .await;
    assert_eq!(reopened.status(), StatusCode::OK);
    assert_eq!(
        response_json(reopened).await["project"]["lifecycle"],
        "active"
    );
    assert!(test
        .state
        .conversation_authority
        .active_turn(&address)
        .await
        .is_none());
    assert!(test
        .state
        .conversation_authority
        .queue_snapshot(&address)
        .await
        .is_empty());
    assert!(test
        .state
        .security
        .approval_authority()
        .list_pending_for_address(&address)
        .await
        .is_empty());
    assert!(!repo.exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn project_lifecycle_metadata_ignores_offline_or_removed_resource_devices() {
    let test = build_app();
    let now = Utc::now();
    let offline_device_id = "0102030405060708";
    let offline_project = ProjectDescriptor {
        execution_default: None,
        project_id: ProjectId::new("project-offline"),
        display_name: "Offline".to_string(),
        lifecycle: ProjectLifecycle::Active,
        resources: rua_project::ProjectResources::create(vec![
            rua_project::ProjectResourceDirectoryInput::new(offline_device_id, "/remote/offline"),
        ])
        .unwrap(),
        created_at: now,
        updated_at: now,
        extra: BTreeMap::new(),
    };
    test.state
        .project_catalog
        .apply_descriptor(offline_project.clone())
        .unwrap();
    assert!(
        !test
            .state
            .rua_root
            .join("projects/project-offline")
            .exists(),
        "synchronized Project metadata alone must not create a Workspace"
    );
    let sync_dir = test.state.rua_root.join("sync");
    std::fs::create_dir_all(&sync_dir).unwrap();
    std::fs::write(
        sync_dir.join("device_list.json"),
        serde_json::to_vec_pretty(&rua_sync::device_list::DeviceList {
            devices: vec![rua_sync::device_list::DeviceEntry {
                device_id: hex::decode(offline_device_id).unwrap().try_into().unwrap(),
                public_key: [7; 32],
                label: "Offline".to_string(),
                friendly_name: "Offline".to_string(),
                added_at_ms: 1,
                last_seen_ms: 1,
                direct_addrs: Vec::new(),
            }],
            ..Default::default()
        })
        .unwrap(),
    )
    .unwrap();

    let offline_archive = request(
        &test.app,
        Method::POST,
        "/api/projects/project-offline/archive",
        Some(json!({"confirmed": true})),
    )
    .await;
    assert_eq!(offline_archive.status(), StatusCode::OK);
    assert_eq!(
        test.state
            .project_catalog
            .get(&offline_project.project_id)
            .unwrap()
            .unwrap()
            .lifecycle,
        ProjectLifecycle::Archived
    );

    let revoked_project = ProjectDescriptor {
        execution_default: None,
        project_id: ProjectId::new("project-revoked"),
        display_name: "Revoked".to_string(),
        lifecycle: ProjectLifecycle::Active,
        resources: rua_project::ProjectResources::create(vec![
            rua_project::ProjectResourceDirectoryInput::new("1112131415161718", "/remote/revoked"),
        ])
        .unwrap(),
        created_at: now,
        updated_at: now,
        extra: BTreeMap::new(),
    };
    test.state
        .project_catalog
        .apply_descriptor(revoked_project.clone())
        .unwrap();
    assert!(
        !test
            .state
            .rua_root
            .join("projects/project-revoked")
            .exists(),
        "metadata-only Project must remain without a local Workspace"
    );
    let preview = request(
        &test.app,
        Method::GET,
        "/api/projects/project-revoked/lifecycle-preview",
        None,
    )
    .await;
    assert_eq!(preview.status(), StatusCode::OK);
    let preview = response_json(preview).await;
    assert_eq!(preview["impact"]["affected_chat_count"], 0);

    let revoked_archive = request(
        &test.app,
        Method::POST,
        "/api/projects/project-revoked/archive",
        Some(json!({"confirmed": true})),
    )
    .await;
    assert_eq!(revoked_archive.status(), StatusCode::OK);
    let revoked_archive = response_json(revoked_archive).await;
    assert_eq!(revoked_archive["project"]["lifecycle"], "archived");
    assert_eq!(revoked_archive["impact"]["affected_chat_count"], 0);

    let revoked_reopen = request(
        &test.app,
        Method::POST,
        "/api/projects/project-revoked/reopen",
        None,
    )
    .await;
    assert_eq!(revoked_reopen.status(), StatusCode::OK);
    let archived_again = request(
        &test.app,
        Method::POST,
        "/api/projects/project-revoked/archive",
        Some(json!({"confirmed":true})),
    )
    .await;
    assert_eq!(archived_again.status(), StatusCode::OK);

    let revoked_delete = request(
        &test.app,
        Method::POST,
        "/api/projects/project-revoked/permanent-delete",
        Some(json!({"confirmed": true})),
    )
    .await;
    assert_eq!(revoked_delete.status(), StatusCode::OK);
    assert!(test
        .state
        .project_catalog
        .get(&revoked_project.project_id)
        .unwrap()
        .is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn project_permanent_delete_cleans_rua_state_and_preserves_repository() {
    let test = build_app();
    let local_device_id = local_device_id(&test.state);
    let repo = test._temp.path().join("delete-project");
    std::fs::create_dir_all(repo.join(".git/objects")).unwrap();
    std::fs::create_dir_all(repo.join("src")).unwrap();
    std::fs::write(repo.join("README.md"), b"keep me byte-for-byte\n").unwrap();
    std::fs::write(repo.join("src/lib.rs"), b"pub fn untouched() {}\n").unwrap();
    std::fs::write(repo.join(".git/HEAD"), b"ref: refs/heads/main\n").unwrap();
    std::fs::write(repo.join(".git/objects/fixture"), [0, 1, 2, 3, 255]).unwrap();
    let before = snapshot_directory(&repo);

    let create = request(
        &test.app,
        Method::POST,
        "/api/projects",
        Some(json!({
            "directories": [{"device_id": local_device_id, "resource_root": repo}],
            "display_name": "Delete fixture",
        })),
    )
    .await;
    assert_eq!(create.status(), StatusCode::CREATED);
    let project_id = response_json(create).await["project"]["project_id"]
        .as_str()
        .unwrap()
        .to_string();
    let project_id_typed = ProjectId::new(project_id.clone());
    let chat_id = "00000000-0000-4000-8000-000000000511";
    let submit = request(
        &test.app,
        Method::POST,
        &format!("/api/chats/{chat_id}/console/submit"),
        Some(json!({ "project_id": project_id,
            "agent_id": "default",
            "chat_id": chat_id,
            "message": "create history before delete",
        })),
    )
    .await;
    assert_eq!(submit.status(), StatusCode::OK);
    wait_for_chat_idle(&test.state, chat_id).await;
    let address = ChatAddress::new(chat_id);
    let scope_root = ChatStore::resolve(&test.state.rua_root, address.clone())
        .root()
        .to_path_buf();

    let survivor = "00000000-0000-4000-8000-000000000512";
    let store = rua_session::WorkspaceSessionStore::new(&scope_root);
    let mut unrelated = store.load_chat_header(chat_id).await.unwrap().unwrap();
    unrelated.chat_id = survivor.to_string();
    unrelated.metadata.chat_id = survivor.to_string();
    unrelated.project_id = None;
    store.save_chat_header(&unrelated).await.unwrap();

    for (call_id, decision) in [
        (
            "chat-rule",
            ApprovalDecision::AllowChat {
                decided_by: "test".to_string(),
                matcher: ApprovalReuseMatcher::ToolCall {
                    tool_name: "bash".to_string(),
                    input_params: Some("chat-rule".to_string()),
                },
            },
        ),
        (
            "agent-rule",
            ApprovalDecision::AllowAgent {
                decided_by: "test".to_string(),
                matcher: ApprovalReuseMatcher::ToolCall {
                    tool_name: "bash".to_string(),
                    input_params: Some("agent-rule".to_string()),
                },
            },
        ),
    ] {
        let approval = test
            .state
            .security
            .approval_authority()
            .create_request(ApprovalRequestInput {
                agent_id: "default".to_string(),
                owner: rua_security::ApprovalOwner::chat(address.clone()),
                turn_id: format!("turn-{call_id}"),
                tool_call_id: call_id.to_string(),
                tool_name: "bash".to_string(),
                trigger: ApprovalTrigger::approval_mode("approval required"),
                tool_call: ToolCallSnapshot {
                    display: format!("bash {call_id}"),
                    args: json!({"command": call_id}),
                },
            })
            .await;
        test.state
            .security
            .approval_authority()
            .decide(&approval.id, decision)
            .await
            .unwrap();
    }
    assert_eq!(
        test.state
            .security
            .approval_authority()
            .list_chat_rules(&address)
            .await
            .len(),
        1
    );
    assert_eq!(
        test.state
            .security
            .approval_authority()
            .list_agent_rules("default")
            .await
            .len(),
        1
    );

    let before_archive = request(
        &test.app,
        Method::POST,
        &format!("/api/projects/{project_id}/permanent-delete"),
        Some(json!({"confirmed": true})),
    )
    .await;
    assert_eq!(before_archive.status(), StatusCode::CONFLICT);
    let unconfirmed = request(
        &test.app,
        Method::POST,
        &format!("/api/projects/{project_id}/permanent-delete"),
        Some(json!({"confirmed": false})),
    )
    .await;
    assert_eq!(unconfirmed.status(), StatusCode::BAD_REQUEST);

    let archive = request(
        &test.app,
        Method::POST,
        &format!("/api/projects/{project_id}/archive"),
        Some(json!({"confirmed": true})),
    )
    .await;
    assert_eq!(archive.status(), StatusCode::OK);
    assert!(
        scope_root.is_dir(),
        "Archive must retain the local Project Workspace"
    );
    assert_eq!(snapshot_directory(&repo), before);
    let duplicate_while_archived = request(
        &test.app,
        Method::POST,
        "/api/projects",
        Some(json!({
            "directories": [{"device_id": local_device_id, "resource_root": repo}],
            "display_name": "Still owned",
        })),
    )
    .await;
    assert_eq!(duplicate_while_archived.status(), StatusCode::CREATED);

    let deleted = request(
        &test.app,
        Method::POST,
        &format!("/api/projects/{project_id}/permanent-delete"),
        Some(json!({"confirmed": true})),
    )
    .await;
    assert_eq!(deleted.status(), StatusCode::OK);
    let deleted = response_json(deleted).await;
    assert_eq!(deleted["project_id"], project_id);
    assert_eq!(deleted["deleted_chat_count"], 1);
    assert_eq!(deleted["changed"], true);
    assert!(test
        .state
        .project_catalog
        .get(&project_id_typed)
        .unwrap()
        .is_none());
    assert!(store.load_chat_header(chat_id).await.unwrap().is_none());
    assert!(store.load_chat_header(survivor).await.unwrap().is_some());
    assert!(test
        .state
        .security
        .approval_authority()
        .list_chat_rules(&address)
        .await
        .is_empty());
    assert_eq!(
        test.state
            .security
            .approval_authority()
            .list_agent_rules("default")
            .await
            .len(),
        1
    );
    assert_eq!(snapshot_directory(&repo), before);

    let replacement = request(
        &test.app,
        Method::POST,
        "/api/projects",
        Some(json!({
            "directories": [{"device_id": local_device_id, "resource_root": repo}],
            "display_name": "Replacement",
        })),
    )
    .await;
    assert_eq!(replacement.status(), StatusCode::CREATED);
    let replacement = response_json(replacement).await;
    let replacement_id = replacement["project"]["project_id"].as_str().unwrap();
    assert_ne!(replacement_id, project_id);
    let replacement_chats = request(
        &test.app,
        Method::GET,
        &format!("/api/projects/{replacement_id}/chats"),
        None,
    )
    .await;
    assert_eq!(replacement_chats.status(), StatusCode::OK);
    assert!(response_json(replacement_chats)
        .await
        .as_array()
        .unwrap()
        .is_empty());

    let repeated = request(
        &test.app,
        Method::POST,
        &format!("/api/projects/{project_id}/permanent-delete"),
        Some(json!({"confirmed": true})),
    )
    .await;
    assert_eq!(repeated.status(), StatusCode::OK);
    assert_eq!(response_json(repeated).await["changed"], false);
    assert_eq!(snapshot_directory(&repo), before);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn archived_offline_project_delete_does_not_contact_resource_device() {
    let test = build_app();
    let project = ProjectDescriptor {
        execution_default: None,
        project_id: ProjectId::new("project-offline-delete"),
        display_name: "Offline delete".to_string(),
        lifecycle: ProjectLifecycle::Archived,
        resources: rua_project::ProjectResources::create(vec![
            rua_project::ProjectResourceDirectoryInput::new("0102030405060708", "/remote/offline"),
        ])
        .unwrap(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        extra: BTreeMap::new(),
    };
    test.state
        .project_catalog
        .apply_descriptor(project.clone())
        .unwrap();
    let sync_dir = test.state.rua_root.join("sync");
    std::fs::create_dir_all(&sync_dir).unwrap();
    std::fs::write(
        sync_dir.join("device_list.json"),
        serde_json::to_vec_pretty(&rua_sync::device_list::DeviceList {
            devices: vec![rua_sync::device_list::DeviceEntry {
                device_id: hex::decode(&project.resources.directories[0].device_id)
                    .unwrap()
                    .try_into()
                    .unwrap(),
                public_key: [8; 32],
                label: "Offline".to_string(),
                friendly_name: "Offline".to_string(),
                added_at_ms: 1,
                last_seen_ms: 1,
                direct_addrs: Vec::new(),
            }],
            ..Default::default()
        })
        .unwrap(),
    )
    .unwrap();

    let deleted = request(
        &test.app,
        Method::POST,
        "/api/projects/project-offline-delete/permanent-delete",
        Some(json!({"confirmed": true})),
    )
    .await;
    assert_eq!(deleted.status(), StatusCode::OK);
    assert!(test
        .state
        .project_catalog
        .get(&project.project_id)
        .unwrap()
        .is_none());
}

fn snapshot_directory(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    fn visit(root: &Path, current: &Path, output: &mut Vec<(PathBuf, Vec<u8>)>) {
        let mut entries = std::fs::read_dir(current)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                visit(root, &path, output);
            } else {
                output.push((
                    path.strip_prefix(root).unwrap().to_path_buf(),
                    std::fs::read(path).unwrap(),
                ));
            }
        }
    }
    let mut output = Vec::new();
    visit(root, root, &mut output);
    output
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_resource_locations_are_visible_and_chat_deletion_targets_its_saved_authority() {
    let test = build_app();
    let project = ProjectDescriptor {
        execution_default: None,
        project_id: ProjectId::new("remote-project"),
        display_name: "Remote Project".to_string(),
        lifecycle: ProjectLifecycle::Active,
        resources: rua_project::ProjectResources::create(vec![
            rua_project::ProjectResourceDirectoryInput::new(
                "1111111111111111",
                "/remote/display/path",
            ),
        ])
        .unwrap(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        extra: BTreeMap::new(),
    };
    test.state
        .project_catalog
        .apply_descriptor(project)
        .unwrap();

    let list = request(&test.app, Method::GET, "/api/projects?lifecycle=all", None).await;
    assert_eq!(list.status(), StatusCode::OK);
    let list = response_json(list).await;
    assert_eq!(list["projects"][0]["project_id"], "remote-project");
    assert!(list["projects"][0].get("availability").is_none());
    assert!(list["projects"][0].get("resource_root").is_none());
    assert_eq!(
        list["projects"][0]["resources"]["directories"][0]["resource_root"],
        "/remote/display/path"
    );
    let chat_id = "00000000-0000-4000-8000-000000000411";
    let store = rua_session::WorkspaceSessionStore::new(test.state.rua_root.join("chats"));
    let now = Utc::now().to_rfc3339();
    let mut header = rua_session::ChatSessionHeader::new(rua_session::ChatMetadata {
        chat_id: chat_id.into(),
        agent_id: "default".into(),
        title: "Remote".into(),
        created_at: now.clone(),
        last_activity_at: now,
        pinned: false,
        archived: false,
        preview: None,
    });
    header.project_id = Some("remote-project".into());
    let execution_device = "2222222222222222";
    header.extra.insert("authority_placement".into(), json!({
        "device_id": execution_device,
        "authority_epoch": rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::next_for_device(execution_device, None),
        "base_manifest_hash": rua_server::services::conversation_manifest::initial_conversation_manifest_hash(&ChatAddress::new(chat_id)).unwrap(),
    }));
    store.save_chat_header(&header).await.unwrap();
    let deleted = request(
        &test.app,
        Method::DELETE,
        &format!("/api/chats/{chat_id}"),
        None,
    )
    .await;
    assert_eq!(deleted.status(), StatusCode::SERVICE_UNAVAILABLE);
    let error = response_json(deleted).await;
    assert!(!error["detail"]
        .as_str()
        .unwrap()
        .contains("Binding not found"));
    assert!(test
        .state
        .project_catalog
        .get(&ProjectId::new("remote-project"))
        .unwrap()
        .is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn project_chat_remote_delete_command_preserves_busy_force_and_not_found() {
    use rua_device_network::DeviceSessionCommandHandler;
    use rua_server::services::device_command_bus::{
        DeviceCommandHandler, DeviceCommandResponse, ProjectDeviceAction, ProjectDeviceCommand,
        ProjectDeviceResponse,
    };
    use rua_server::services::project_chat::ProjectChatDeleteError;

    let test = build_app();
    let device_id = local_device_id(&test.state);
    let repo = test._temp.path().join("remote-delete-resource");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::write(repo.join("KEEP"), "untouched").unwrap();
    let created = request(
        &test.app,
        Method::POST,
        "/api/projects",
        Some(json!({
            "directories": [{"device_id": device_id, "resource_root": repo}],
        })),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let project_id = response_json(created).await["project"]["project_id"]
        .as_str()
        .unwrap()
        .to_string();
    let chat_id = "00000000-0000-4000-8000-000000000419";
    let path = format!("/api/chats/{chat_id}");
    let submitted = request(
        &test.app,
        Method::POST,
        &format!("{path}/console/submit"),
        Some(json!({
            "project_id": project_id, "agent_id": "default", "chat_id": chat_id, "message": "Delete fixture",
        })),
    )
    .await;
    assert_eq!(submitted.status(), StatusCode::OK);
    wait_for_chat_idle(&test.state, chat_id).await;
    let scope = ChatStore::resolve(&test.state.rua_root, ChatAddress::new(chat_id));
    let store = rua_session::WorkspaceSessionStore::new(scope.root());
    store
        .update_chat_header(chat_id, |header| {
            header.extra.insert(
                "acp_binding".into(),
                json!({"kind": "acp_passthrough", "runner": "claude"}),
            );
        })
        .await
        .unwrap();
    let handler = DeviceCommandHandler::new(test._temp.path().join("commands"), device_id.clone())
        .with_conversation_input_intake(test.state.conversation_input_intake.clone());
    for (index, force) in [false, true, false].into_iter().enumerate() {
        let command = ProjectDeviceCommand {
            command_id: format!("remote-delete-{index}"),
            source_device_id: "1111111111111111".to_string(),
            target_device_id: device_id.clone(),
            action: ProjectDeviceAction::DeleteChat {
                project_id: ProjectId::new(project_id.clone()),
                chat_id: chat_id.to_string(),
                force,
            },
        };
        let response = handler
            .handle_command_from(
                &command.source_device_id,
                &serde_json::to_vec(&json!({"command": "project", "payload": command})).unwrap(),
            )
            .await
            .unwrap();
        let response: DeviceCommandResponse = serde_json::from_slice(&response).unwrap();
        let DeviceCommandResponse::Project {
            response: ProjectDeviceResponse::ChatDeleted { result },
        } = response
        else {
            panic!("unexpected remote deletion response: {response:?}");
        };
        match index {
            0 => {
                assert!(matches!(result, Err(ProjectChatDeleteError::Busy(_))));
                assert!(store.load_chat_header(chat_id).await.unwrap().is_some());
            }
            1 => {
                let result = result.unwrap();
                assert_eq!(result.chat_id, chat_id);
                assert!(result.was_busy);
                assert!(store.load_chat_header(chat_id).await.unwrap().is_none());
            }
            _ => assert!(matches!(result, Err(ProjectChatDeleteError::NotFound(_)))),
        }
    }
    assert!(rua_server::services::chat_index::get(scope.root(), chat_id)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        std::fs::read_to_string(repo.join("KEEP")).unwrap(),
        "untouched"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn child_root_warns_but_file_path_and_offline_remote_create_do_not_commit() {
    let test = build_app();
    let local_device_id = local_device_id(&test.state);
    let repo = test._temp.path().join("repo");
    let package = repo.join("package");
    std::fs::create_dir_all(&package).unwrap();
    let file = repo.join("README.md");
    std::fs::write(&file, b"repo").unwrap();

    let root = request(
        &test.app,
        Method::POST,
        "/api/projects",
        Some(json!({
            "directories": [{"device_id": local_device_id, "resource_root": repo}],
        })),
    )
    .await;
    assert_eq!(root.status(), StatusCode::CREATED);

    let child = request(
        &test.app,
        Method::POST,
        "/api/projects",
        Some(json!({
            "directories": [{"device_id": local_device_id, "resource_root": package}],
        })),
    )
    .await;
    assert_eq!(child.status(), StatusCode::CREATED);
    let child = response_json(child).await;
    assert_eq!(
        child["overlap_warnings"][0]["relation"],
        "existing_ancestor"
    );

    let file_response = request(
        &test.app,
        Method::POST,
        "/api/projects",
        Some(json!({
            "directories": [{"device_id": local_device_id, "resource_root": file}],
        })),
    )
    .await;
    assert_eq!(file_response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(test.state.project_catalog.list().unwrap().len(), 2);

    let offline = request(
        &test.app,
        Method::POST,
        "/api/projects",
        Some(json!({
            "directories": [{"device_id": "ffffffffffffffff", "resource_root": "/remote/only"}],
        })),
    )
    .await;
    assert_eq!(offline.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(test.state.project_catalog.list().unwrap().len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn project_chat_archive_filter_and_bulk_delete_archived_contract() {
    let test = build_app();
    let local_device_id = local_device_id(&test.state);
    let repo = test._temp.path().join("archive-repo");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::write(repo.join("KEEP.md"), b"archive contract\n").unwrap();

    let create = request(
        &test.app,
        Method::POST,
        "/api/projects",
        Some(json!({
            "directories": [{"device_id": local_device_id, "resource_root": repo}],
            "display_name": "Archive Contract",
        })),
    )
    .await;
    let project_id = response_json(create).await["project"]["project_id"]
        .as_str()
        .unwrap()
        .to_string();

    let chat_ids = [
        "00000000-0000-4000-8000-000000000501",
        "00000000-0000-4000-8000-000000000502",
    ];
    for (chat_id, message) in chat_ids.iter().zip(["Keep me", "Archive me"]) {
        let submit = request(
            &test.app,
            Method::POST,
            &format!("/api/chats/{chat_id}/console/submit"),
            Some(json!({ "project_id": project_id,
                "agent_id": "default",
                "chat_id": chat_id,
                "message": message,
            })),
        )
        .await;
        assert_eq!(submit.status(), StatusCode::OK);
        wait_for_chat_idle(&test.state, chat_id).await;
    }

    let archive_patch = request(
        &test.app,
        Method::PATCH,
        &format!("/api/chats/{}", chat_ids[1]),
        Some(json!({ "archived": true })),
    )
    .await;
    assert_eq!(archive_patch.status(), StatusCode::OK);
    let archived_chat = response_json(archive_patch).await;
    assert_eq!(archived_chat["id"], chat_ids[1]);
    assert_eq!(archived_chat["archived"], true);

    let all_list = request(
        &test.app,
        Method::GET,
        &format!("/api/projects/{project_id}/chats?limit=50"),
        None,
    )
    .await;
    assert_eq!(all_list.status(), StatusCode::OK);
    let all_chats = response_json(all_list).await;
    assert_eq!(all_chats.as_array().unwrap().len(), 1);

    let active_list = request(
        &test.app,
        Method::GET,
        &format!("/api/projects/{project_id}/chats?archived=false&limit=50"),
        None,
    )
    .await;
    assert_eq!(active_list.status(), StatusCode::OK);
    assert_eq!(active_list.headers()["X-Total-Count"], "1");
    let active_chats = response_json(active_list).await;
    assert_eq!(active_chats.as_array().unwrap().len(), 1);
    assert_eq!(active_chats[0]["id"], chat_ids[0]);
    assert_eq!(active_chats[0]["archived"], false);

    let archived_list = request(
        &test.app,
        Method::GET,
        &format!("/api/projects/{project_id}/chats?archived=true&limit=50"),
        None,
    )
    .await;
    assert_eq!(archived_list.status(), StatusCode::OK);
    assert_eq!(archived_list.headers()["X-Total-Count"], "1");
    let archived_chats = response_json(archived_list).await;
    assert_eq!(archived_chats.as_array().unwrap().len(), 1);
    assert_eq!(archived_chats[0]["id"], chat_ids[1]);
    assert_eq!(archived_chats[0]["archived"], true);

    let collaboration_address = rua_core::ChatAddress::new(chat_ids[0]);
    let scope = rua_core::ChatStore::resolve(&test.state.rua_root, collaboration_address.clone());
    let store = rua_session::WorkspaceSessionStore::new(scope.root());
    store.update_chat_header(chat_ids[0], |header| {
        header.metadata.archived = true;
        header.extra.insert("collaboration".into(), json!({"source":{"scope":{"kind":"project","project_id":project_id},"chat_id":chat_ids[1]},"executor":{"kind":"agent","agent_id":"default"}}));
    }).await.unwrap();
    rua_chat::ChatService::from_workspace(rua_core::WorkspaceRoot::new(scope.root()))
        .repair_projection(&collaboration_address)
        .await
        .unwrap();

    let bulk_delete = request(
        &test.app,
        Method::POST,
        &format!("/api/projects/{project_id}/chats/delete-archived"),
        None,
    )
    .await;
    assert_eq!(bulk_delete.status(), StatusCode::OK);
    let bulk_delete = response_json(bulk_delete).await;
    assert_eq!(bulk_delete["deleted"].as_array().unwrap().len(), 1);
    assert_eq!(bulk_delete["deleted"][0], chat_ids[1]);
    assert_eq!(bulk_delete["skipped"].as_array().unwrap().len(), 0);

    let post_delete_list = request(
        &test.app,
        Method::GET,
        &format!("/api/projects/{project_id}/chats?limit=50"),
        None,
    )
    .await;
    assert_eq!(post_delete_list.status(), StatusCode::OK);
    assert_eq!(post_delete_list.headers()["X-Total-Count"], "0");
    let remaining = response_json(post_delete_list).await;
    assert!(remaining.as_array().unwrap().is_empty());
    let collaboration_list = request(
        &test.app,
        Method::GET,
        &format!("/api/projects/{project_id}/chats?archived=true&view=collaboration"),
        None,
    )
    .await;
    assert_eq!(collaboration_list.headers()["X-Total-Count"], "1");
    assert_eq!(
        response_json(collaboration_list).await[0]["id"],
        chat_ids[0]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_metadata_reads_only_the_index_including_archived_project_chats() {
    let test = build_app();
    let chat_id = "00000000-0000-4000-8000-000000000914";
    for id in ["metadata-project", "metadata-other"] {
        test.state
            .project_catalog
            .apply_descriptor(ProjectDescriptor {
                execution_default: None,
                project_id: ProjectId::new(id),
                display_name: id.to_string(),
                lifecycle: ProjectLifecycle::Active,
                resources: rua_project::ProjectResources::create(vec![
                    rua_project::ProjectResourceDirectoryInput::new(
                        "1111111111111111",
                        "/remote/unavailable",
                    ),
                ])
                .unwrap(),
                created_at: Utc::now(),
                updated_at: Utc::now(),
                extra: BTreeMap::new(),
            })
            .unwrap();
    }
    let scope = ChatStore::resolve(&test.state.rua_root, ChatAddress::new(chat_id));
    let uri = format!("/api/chats/{chat_id}/metadata");
    let missing = request(&test.app, Method::GET, &uri, None).await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    assert!(response_json(missing).await["detail"].is_string());
    assert!(
        !scope.root().exists(),
        "metadata read must not materialize a remote scope"
    );

    let now = Utc::now();
    rua_session::chat_index::insert(
        scope.root(),
        rua_session::chat_index::ChatIndexRecord {
            id: chat_id.to_string(),
            name: "Archived project chat".to_string(),
            user_id: "default".to_string(),
            channel: "console".to_string(),
            created_at: now,
            updated_at: now,
            meta: json!({"sidebar_fixture": "kept"}),
            status: "idle".to_string(),
            pinned: false,
            pin_order: 0,
            tag: rua_session::ChatTag::Green,
            archived: true,
        },
    )
    .await
    .unwrap();
    let history_path = scope.root().join("sessions");
    std::fs::write(&history_path, b"history must not be read").unwrap();
    let response = request(&test.app, Method::GET, &uri, None).await;
    assert_eq!(response.status(), StatusCode::OK);
    let metadata = response_json(response).await;
    assert_eq!(metadata["id"], chat_id);
    assert_eq!(metadata["name"], "Archived project chat");
    assert_eq!(metadata["archived"], true);
    assert_eq!(metadata["tag"], "green");
    assert_eq!(metadata["meta"]["sidebar_fixture"], "kept");
    assert!(metadata.get("messages").is_none());
    assert_eq!(
        std::fs::read(&history_path).unwrap(),
        b"history must not be read"
    );

    for project in ["metadata-other", "metadata-unregistered"] {
        let response = request(
            &test.app,
            Method::GET,
            &format!("{uri}?project_id={project}"),
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response_json(response).await, metadata);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn multi_directory_project_creation_preserves_roots_and_rejects_duplicate_entries() {
    let test = build_app();
    let app_dir = test._temp.path().join("multi-app");
    let docs_dir = test._temp.path().join("multi-docs");
    std::fs::create_dir(&app_dir).unwrap();
    std::fs::create_dir(&docs_dir).unwrap();
    let local = local_device_id(&test.state);
    let entry = |root: &Path| json!({"device_id":local,"resource_root":root});
    let body =
        json!({"directories":[entry(&app_dir),entry(&docs_dir)], "display_name":"App and docs"});
    let response = request(&test.app, Method::POST, "/api/projects", Some(body.clone())).await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let created = response_json(response).await;
    let descriptor = &created["project"];
    let directories = descriptor["resources"]["directories"].as_array().unwrap();
    assert_eq!(directories.len(), 2);
    assert_eq!(directories[0]["resource_root"], json!(app_dir));
    assert_eq!(directories[1]["resource_root"], json!(docs_dir));
    assert_eq!(directories[1]["device_id"], local);
    assert_ne!(
        directories[0]["directory_id"],
        directories[1]["directory_id"]
    );
    assert!(descriptor.get("resource_device_id").is_none());
    assert!(descriptor["resources"]
        .get("primary_directory_id")
        .is_none());

    let reused = request(&test.app, Method::POST, "/api/projects", Some(body)).await;
    assert_eq!(reused.status(), StatusCode::CREATED);
    let reused = response_json(reused).await;
    assert_ne!(reused["project"]["project_id"], descriptor["project_id"]);
    assert_eq!(
        reused["overlap_warnings"][0]["relation"],
        "shared_directory"
    );

    let invalid = request(
        &test.app,
        Method::POST,
        "/api/projects",
        Some(json!({
            "directories": [entry(&app_dir),entry(&app_dir.join("."))]
        })),
    )
    .await;
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
    assert_eq!(test.state.project_catalog.list().unwrap().len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resource_edits_preserve_referencing_chat_and_allow_active_admission() {
    let test = build_app();
    let app = test._temp.path().join("edit-app");
    let docs = test._temp.path().join("edit-docs");
    let assets = test._temp.path().join("edit-assets");
    for root in [&app, &docs, &assets] {
        std::fs::create_dir(root).unwrap();
    }
    let local = local_device_id(&test.state);
    let entry = |root: &Path| json!({"device_id":local,"resource_root":root});
    let created = response_json(
        request(
            &test.app,
            Method::POST,
            "/api/projects",
            Some(json!({
                "directories":[entry(&app),entry(&docs)]
            })),
        )
        .await,
    )
    .await;
    let id = ProjectId::new(created["project"]["project_id"].as_str().unwrap());
    let uri = format!("/api/projects/{id}/resources");
    let chat_id = "00000000-0000-4000-8000-000000000703";
    let submitted = request(
        &test.app,
        Method::POST,
        &format!("/api/chats/{chat_id}/console/submit"),
        Some(json!({ "project_id": id,"agent_id":"default", "message":"Keep this folder"})),
    )
    .await;
    assert_eq!(submitted.status(), StatusCode::OK);
    wait_for_chat_idle(&test.state, chat_id).await;
    let archived = request(
        &test.app,
        Method::PATCH,
        &format!("/api/chats/{chat_id}"),
        Some(json!({"archived":true})),
    )
    .await;
    assert_eq!(archived.status(), StatusCode::OK);
    let store = rua_session::WorkspaceSessionStore::new(test.state.chat_workspace().as_path());
    let before = store.load_chat_header(chat_id).await.unwrap().unwrap();
    let history_uri = format!("/api/chats/{chat_id}");
    let history = response_json(request(&test.app, Method::GET, &history_uri, None).await).await;
    assert!(!history["messages"].as_array().unwrap().is_empty());
    let admission = test.state.project_catalog.begin_admission(&id).unwrap();
    let remove_used = request(
        &test.app,
        Method::PATCH,
        &uri,
        Some(json!({"expected_revision":1,"directories":[entry(&assets)]})),
    )
    .await;
    assert_eq!(remove_used.status(), StatusCode::OK);
    drop(admission);
    let stale = request(
        &test.app,
        Method::PATCH,
        &uri,
        Some(json!({"expected_revision":1,"directories":[]})),
    )
    .await;
    assert_eq!(stale.status(), StatusCode::CONFLICT);
    let empty = request(
        &test.app,
        Method::PATCH,
        &uri,
        Some(json!({"expected_revision":2,"directories":[]})),
    )
    .await;
    assert_eq!(empty.status(), StatusCode::OK);
    let project = &response_json(empty).await["project"];
    assert_eq!(project["project_id"], id.as_str());
    assert_eq!(project["resources"], json!({"revision":3,"directories":[]}));
    assert_eq!(
        store.load_chat_header(chat_id).await.unwrap().unwrap(),
        before
    );
    let after = response_json(request(&test.app, Method::GET, &history_uri, None).await).await;
    assert_eq!(after["messages"], history["messages"]);
    assert!(app.exists() && docs.exists() && assets.exists());
}

#[tokio::test]
async fn sidebar_preferences_are_independent_of_resource_device_availability_and_reject_stale_edits(
) {
    let test = build_app();
    let now = chrono::Utc::now();
    test.state
        .project_catalog
        .apply_descriptor(rua_project::ProjectDescriptor {
            execution_default: None,
            project_id: ProjectId::new("remote-sidebar"),
            display_name: "Remote".into(),
            resources: rua_project::ProjectResources::create(vec![
                rua_project::ProjectResourceDirectoryInput::new("offline-device", "/remote/repo"),
            ])
            .unwrap(),
            lifecycle: rua_project::ProjectLifecycle::Active,
            created_at: now,
            updated_at: now,
            extra: Default::default(),
        })
        .unwrap();
    let initial =
        response_json(request(&test.app, Method::GET, "/api/project-sidebar", None).await).await;
    let changed=request(&test.app,Method::PATCH,"/api/project-sidebar",Some(json!({
        "expected_revision":initial["revision"],"action":"set_pinned","project_id":"remote-sidebar","pinned":true,
    }))).await;
    assert_eq!(changed.status(), StatusCode::OK);
    let changed = response_json(changed).await;
    assert_eq!(changed["items"]["remote-sidebar"]["pinned"], true);
    let stale=request(&test.app,Method::PATCH,"/api/project-sidebar",Some(json!({
        "expected_revision":initial["revision"],"action":"set_pinned","project_id":"remote-sidebar","pinned":false,
    }))).await;
    assert_eq!(stale.status(), StatusCode::CONFLICT);
    let moved=request(&test.app,Method::PATCH,"/api/project-sidebar",Some(json!({
        "expected_revision":changed["revision"],"action":"reorder","project_id":"remote-sidebar","ordered_project_ids":["remote-sidebar"],
    }))).await;
    assert_eq!(moved.status(), StatusCode::OK);
    let reopened = rua_project::ProjectSidebarStore::new(&test.state.rua_root)
        .snapshot()
        .unwrap();
    assert_eq!(reopened.sort, rua_project::ProjectSidebarSort::Manual);
    assert!(reopened.items["remote-sidebar"].pinned);
}

fn resource_query(device: &str, root: &std::path::Path, path: Option<&std::path::Path>) -> String {
    let mut url = reqwest::Url::parse("http://localhost").unwrap();
    url.query_pairs_mut()
        .append_pair("device_id", device)
        .append_pair("resource_root", root.to_str().unwrap());
    if let Some(path) = path {
        url.query_pairs_mut()
            .append_pair("path", path.to_str().unwrap());
    }
    url.query().unwrap().to_string()
}

#[tokio::test]
async fn resource_views_use_device_and_absolute_path_paginate_and_reject_cross_root_paths() {
    let test = build_app();
    let first = test._temp.path().join("first-resource");
    let second = test._temp.path().join(if cfg!(unix) {
        "second & 手机 "
    } else {
        "second & 手机"
    });
    std::fs::create_dir(&first).unwrap();
    std::fs::create_dir(&second).unwrap();
    std::fs::write(first.join("only-first"), "first").unwrap();
    std::fs::write(second.join("only-second"), "second").unwrap();
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::ffi::OsStringExt;
        let name = std::ffi::OsString::from_vec(vec![0xff]);
        std::fs::write(second.join(name), "cannot address this name with JSON").unwrap();
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink("unavailable", second.join("unreadable-link")).unwrap();
    assert!(Command::new("git")
        .args(["init", "-q"])
        .arg(&second)
        .status()
        .unwrap()
        .success());
    let created=response_json(request(&test.app,Method::POST,"/api/projects",Some(json!({"directories":[{"device_id":local_device_id(&test.state),"resource_root":first},{"device_id":local_device_id(&test.state),"resource_root":second}]}))).await).await;
    let project = &created["project"];
    assert_eq!(
        project["resources"]["directories"][1]["resource_root"],
        json!(second)
    );
    let id = project["project_id"].as_str().unwrap();
    let device = local_device_id(&test.state);
    let selected = resource_query(&device, &second, Some(&second));
    let git_location = resource_query(&device, &second, None);
    let first_location = resource_query(&device, &first, None);
    let picker = request(
        &test.app,
        Method::POST,
        "/api/projects/directory",
        Some(json!({"device_id":device,"path":second})),
    )
    .await;
    assert_eq!(picker.status(), StatusCode::OK);
    let picker = response_json(picker).await;
    assert_eq!(picker["path"], json!(second));
    let files = request(
        &test.app,
        Method::GET,
        &format!("/api/projects/{id}/workspace/files?{selected}"),
        None,
    )
    .await;
    assert_eq!(files.status(), StatusCode::OK);
    let files = response_json(files).await;
    assert!(files["entries"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["name"] == "only-second"));
    assert!(files["roots"].as_array().unwrap().is_empty());
    let omissions = usize::from(cfg!(unix)) + usize::from(cfg!(target_os = "linux"));
    assert_eq!(files["omitted_entries"], omissions);
    assert_eq!(files["device_id"], device);
    let first_page = response_json(
        request(
            &test.app,
            Method::GET,
            &format!("/api/projects/{id}/workspace/files?{selected}&limit=1"),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(first_page["entries"].as_array().unwrap().len(), 1);
    assert_eq!(first_page["omitted_entries"], files["omitted_entries"]);
    let offset = first_page["next_offset"].as_u64().unwrap();
    let last_page = response_json(
        request(
            &test.app,
            Method::GET,
            &format!("/api/projects/{id}/workspace/files?{selected}&limit=1&offset={offset}"),
            None,
        )
        .await,
    )
    .await;
    assert_ne!(
        first_page["entries"][0]["path"],
        last_page["entries"][0]["path"]
    );
    assert!(last_page.get("next_offset").is_none());
    assert_eq!(last_page["omitted_entries"], files["omitted_entries"]);
    assert_eq!(picker["omitted_entries"], files["omitted_entries"]);
    let child = if cfg!(unix) { "Summer " } else { "Summer" };
    let creation = request(
        &test.app,
        Method::POST,
        "/api/projects/directory/create",
        Some(json!({"device_id":device,"parent":second,"name":child})),
    )
    .await;
    assert_eq!(creation.status(), StatusCode::OK);
    assert_eq!(
        response_json(creation).await["path"],
        json!(second.join(child))
    );
    assert!(second.join(child).is_dir());
    let git = response_json(
        request(
            &test.app,
            Method::GET,
            &format!("/api/projects/{id}/workspace/git?{git_location}"),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(git["repository"], true);
    assert_eq!(git["device_id"], device);
    assert_eq!(git["resource_root"], json!(second));
    let git = response_json(
        request(
            &test.app,
            Method::GET,
            &format!("/api/projects/{id}/workspace/git?{first_location}"),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(git["repository"], false);
    for suffix in [
        resource_query("foreign", &second, Some(&second)),
        resource_query(&device, &second, Some(&first)),
        resource_query(&device, &second, Some(std::path::Path::new("relative"))),
    ] {
        let response = request(
            &test.app,
            Method::GET,
            &format!("/api/projects/{id}/workspace/files?{suffix}"),
            None,
        )
        .await;
        assert!(!response.status().is_success());
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&first, second.join("escape")).unwrap();
        let response = request(
            &test.app,
            Method::GET,
            &format!(
                "/api/projects/{id}/workspace/files?{}",
                resource_query(&device, &second, Some(&second.join("escape")))
            ),
            None,
        )
        .await;
        assert!(!response.status().is_success());
    }
    std::fs::remove_dir_all(&second).unwrap();
    let response = request(
        &test.app,
        Method::GET,
        &format!("/api/projects/{id}/workspace/git?{git_location}"),
        None,
    )
    .await;
    assert!(!response.status().is_success());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bulk_archive_includes_collaboration_skips_busy_and_retries_only_selected_chats() {
    let test = build_app();
    let root = test._temp.path().join("bulk-resource");
    std::fs::create_dir(&root).unwrap();
    let created = response_json(
        request(
            &test.app,
            Method::POST,
            "/api/projects",
            Some(json!({"directories": [{"device_id": local_device_id(&test.state), "resource_root": root}]})),
        )
        .await,
    )
    .await;
    let project_id = ProjectId::new(created["project"]["project_id"].as_str().unwrap());
    let ids = [
        "00000000-0000-4000-8000-000000000801",
        "00000000-0000-4000-8000-000000000802",
        "00000000-0000-4000-8000-000000000803",
    ];
    for id in ids {
        let submitted = request(
            &test.app,
            Method::POST,
            &format!("/api/chats/{id}/console/submit"),
            Some(json!({ "project_id": project_id,"agent_id":"default","message":"Bulk archive fixture"})),
        )
        .await;
        assert_eq!(submitted.status(), StatusCode::OK);
        wait_for_chat_idle(&test.state, id).await;
    }
    let address = ChatAddress::new(ids[2]);
    let scope = ChatStore::resolve(&test.state.rua_root, address.clone());
    let store = rua_session::WorkspaceSessionStore::new(scope.root());
    store.update_chat_header(ids[1],|header|{header.extra.insert("collaboration".into(),json!({"source":{"scope":{"kind":"project","project_id":project_id},"chat_id":ids[0]},"executor":{"kind":"agent","agent_id":"default"}}));}).await.unwrap();
    rua_chat::ChatService::from_workspace(rua_core::WorkspaceRoot::new(scope.root()))
        .repair_projection(&ChatAddress::new(ids[1]))
        .await
        .unwrap();
    let placement =
        rua_server::services::conversation_authority_target::load_chat_authority_placement_target(
            scope.root(),
            ids[2],
        )
        .await
        .unwrap();
    test.state
        .conversation_authority
        .start_active_turn(ConversationActiveTurnStart {
            address: address.clone(),
            driver: ConversationDriverKind::Rua,
            authority_device_id: placement.device_id,
            authority_epoch: placement.authority_epoch,
            driver_detail: None,
        })
        .await;
    test.state
        .conversation_authority
        .enqueue_input(
            &address,
            ConversationInputPayload {
                message: "Keep queued".into(),
                model_override: None,
                input: None,
                messages: None,
                extra: Default::default(),
                attachments: Vec::new(),
                enqueued_at: std::time::Instant::now(),
            },
            Some("controller".into()),
        )
        .await
        .unwrap();
    let approval = test
        .state
        .security
        .approval_authority()
        .create_request(ApprovalRequestInput {
            agent_id: "default".into(),
            owner: rua_security::ApprovalOwner::chat(address.clone()),
            turn_id: "bulk-turn".into(),
            tool_call_id: "bulk-call".into(),
            tool_name: "bash".into(),
            trigger: ApprovalTrigger::approval_mode("required"),
            tool_call: ToolCallSnapshot {
                display: "bash pwd".into(),
                args: json!({"command":"pwd"}),
            },
        })
        .await;
    let uri = format!("/api/projects/{project_id}/chats/archive-idle");
    let response = request(&test.app, Method::POST, &uri, Some(json!({}))).await;
    assert_eq!(response.status(), StatusCode::OK);
    let result = response_json(response).await;
    let items = result["items"].as_array().unwrap();
    assert_eq!(items.len(), 3);
    assert_eq!(
        items
            .iter()
            .filter(|item| item["status"] == "archived")
            .count(),
        2
    );
    let busy = items.iter().find(|item| item["chat_id"] == ids[2]).unwrap();
    assert_eq!(busy["status"], "skipped");
    let reason = busy["reason"].as_str().unwrap();
    assert!(
        reason.contains("active_turn")
            && reason.contains("queued_input")
            && reason.contains("pending_approval")
    );
    assert!(test
        .state
        .conversation_authority
        .active_turn(&address)
        .await
        .is_some());
    assert_eq!(
        test.state
            .conversation_authority
            .queue_snapshot(&address)
            .await
            .len(),
        1
    );
    assert!(test
        .state
        .security
        .approval_authority()
        .list_pending_for_address(&address)
        .await
        .iter()
        .any(|item| item.id == approval.id));
    assert_eq!(
        test.state
            .project_catalog
            .get(&project_id)
            .unwrap()
            .unwrap()
            .lifecycle,
        ProjectLifecycle::Active
    );
    assert!(root.exists());
    for id in &ids[..2] {
        assert!(
            store
                .load_chat_header(id)
                .await
                .unwrap()
                .unwrap()
                .metadata
                .archived
        );
    }
    let retry = response_json(
        request(
            &test.app,
            Method::POST,
            &uri,
            Some(json!({"chat_ids":[ids[0],"missing-chat","../escape"]})),
        )
        .await,
    )
    .await;
    assert_eq!(retry["items"][0]["status"], "archived");
    assert_eq!(retry["items"][1]["status"], "failed");
    assert_eq!(retry["items"][2]["status"], "failed");
    let admission = test
        .state
        .project_catalog
        .begin_admission(&project_id)
        .unwrap();
    let blocked = response_json(
        request(
            &test.app,
            Method::POST,
            &uri,
            Some(json!({"chat_ids":[ids[2]]})),
        )
        .await,
    )
    .await;
    assert_eq!(
        blocked["items"][0]["reason"],
        "project_admission_in_progress"
    );
    drop(admission);

    store
        .update_chat_header(ids[0], |header| {
            header.metadata.archived = false;
            header.extra.get_mut("authority_placement").unwrap()["device_id"] =
                json!("offline-authority");
        })
        .await
        .unwrap();
    let offline = response_json(
        request(
            &test.app,
            Method::POST,
            &uri,
            Some(json!({"chat_ids":[ids[0]]})),
        )
        .await,
    )
    .await;
    assert_eq!(offline["items"][0]["status"], "failed");
    assert!(
        !store
            .load_chat_header(ids[0])
            .await
            .unwrap()
            .unwrap()
            .metadata
            .archived
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn legacy_project_chat_listing_updates_do_not_require_runtime_configuration() {
    let test = build_app();
    let repo = test._temp.path().join("legacy-listing");
    std::fs::create_dir_all(&repo).unwrap();
    let created = request(
        &test.app,
        Method::POST,
        "/api/projects",
        Some(json!({
            "directories": [{"device_id": local_device_id(&test.state), "resource_root": repo}],
        })),
    )
    .await;
    let id = response_json(created).await["project"]["project_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let chat_id = "00000000-0000-4000-8000-000000009930";
    let path = format!("/api/chats/{chat_id}");
    let submitted = request(
        &test.app,
        Method::POST,
        &format!("{path}/console/submit"),
        Some(json!({"project_id":id,"agent_id":"default","message":"Legacy listing"})),
    )
    .await;
    assert_eq!(submitted.status(), StatusCode::OK);
    wait_for_chat_idle(&test.state, chat_id).await;
    let address = ChatAddress::new(chat_id);
    let scope = ChatStore::resolve(&test.state.rua_root, address);
    let store = rua_session::WorkspaceSessionStore::new(scope.root());
    let mut header = store.load_chat_header(chat_id).await.unwrap().unwrap();
    header.extra.remove("hook_configuration");
    // Reproduce a pre-snapshot Chat while retaining its original history/identity.
    let head = store.configuration_head_path(chat_id);
    if head.exists() {
        std::fs::remove_file(head).unwrap();
    }
    rua_config::atomic_write(
        &store.chat_header_path(chat_id),
        &serde_json::to_vec(&header).unwrap(),
    )
    .unwrap();
    assert!(!store
        .load_chat_header(chat_id)
        .await
        .unwrap()
        .unwrap()
        .extra
        .contains_key("hook_configuration"));
    for update in [
        json!({"pinned":true}),
        json!({"pinned":false}),
        json!({"name":"Legacy renamed"}),
        json!({"tag":"green"}),
        json!({"archived":true}),
        json!({"archived":false}),
    ] {
        let response = request(&test.app, Method::PATCH, &path, Some(update.clone())).await;
        let status = response.status();
        let body = response_json(response).await;
        assert_eq!(status, StatusCode::OK, "{update}: {body}");
        for (key, value) in update.as_object().unwrap() {
            assert_eq!(&body[key], value);
        }
    }
    let after = store.load_chat_header(chat_id).await.unwrap().unwrap();
    assert_eq!(after.chat_id, header.chat_id);
    assert_eq!(after.message_log, header.message_log);
    assert!(
        !after.extra.contains_key("hook_configuration"),
        "Listing changes must not synthesize runtime configuration"
    );
    let response = request(
        &test.app,
        Method::PATCH,
        &path,
        Some(json!({"collaboration_mode":"plan"})),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert!(response_json(response).await["detail"]
        .as_str()
        .unwrap()
        .contains("configuration snapshot"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resource_edits_leave_active_acp_and_queued_work_untouched() {
    let test = build_app();
    let app = test._temp.path().join("idle-acp-app");
    let docs = test._temp.path().join("idle-acp-docs");
    std::fs::create_dir(&app).unwrap();
    std::fs::create_dir(&docs).unwrap();
    let created = response_json(
        request(
            &test.app,
            Method::POST,
            "/api/projects",
            Some(json!({
                "directories": [{"device_id": local_device_id(&test.state), "resource_root": app}]
            })),
        )
        .await,
    )
    .await;
    let id = created["project"]["project_id"].as_str().unwrap();
    let chat_id = "00000000-0000-4000-8000-000000009940";
    let submitted = request(
        &test.app,
        Method::POST,
        &format!("/api/chats/{chat_id}/console/submit"),
        Some(json!({ "project_id": id,"agent_id":"default", "message":"Idle ACP binding"})),
    )
    .await;
    assert_eq!(submitted.status(), StatusCode::OK);
    wait_for_chat_idle(&test.state, chat_id).await;
    let address = ChatAddress::new(chat_id);
    let scope = ChatStore::resolve(&test.state.rua_root, address.clone());
    let store = rua_session::WorkspaceSessionStore::new(scope.root());
    store
        .update_chat_header(chat_id, |header| {
            header.extra.insert(
                "acp_binding".into(),
                json!({"kind":"acp_passthrough", "runner":"claude"}),
            );
        })
        .await
        .unwrap();
    let preview = response_json(
        request(
            &test.app,
            Method::GET,
            &format!("/api/projects/{id}/lifecycle-preview"),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(preview["impact"]["active_turn_count"], 0);
    assert_eq!(preview["impact"]["active_acp_count"], 1);
    let resources = format!("/api/projects/{id}/resources");
    let changed = request(
        &test.app,
        Method::PATCH,
        &resources,
        Some(json!({"expected_revision":1,"directories":[{"device_id":local_device_id(&test.state),"resource_root":app},{"device_id":local_device_id(&test.state),"resource_root":docs}]})),
    )
    .await;
    let status = changed.status();
    let changed = response_json(changed).await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    assert!(store
        .load_chat_header(chat_id)
        .await
        .unwrap()
        .unwrap()
        .extra
        .contains_key("acp_binding"));
    let placement =
        rua_server::services::conversation_authority_target::load_chat_authority_placement_target(
            scope.root(),
            chat_id,
        )
        .await
        .unwrap();
    test.state
        .conversation_authority
        .start_active_turn(ConversationActiveTurnStart {
            address: address.clone(),
            driver: ConversationDriverKind::Acp,
            authority_device_id: placement.device_id,
            authority_epoch: placement.authority_epoch,
            driver_detail: None,
        })
        .await;
    let unlinked = request(
        &test.app,
        Method::PATCH,
        &resources,
        Some(json!({"expected_revision":2,"directories":[]})),
    )
    .await;
    assert_eq!(unlinked.status(), StatusCode::OK);
    assert!(test
        .state
        .conversation_authority
        .active_turn(&address)
        .await
        .is_some());
    test.state
        .conversation_authority
        .enqueue_input(
            &address,
            ConversationInputPayload {
                message: "pending".into(),
                model_override: None,
                input: None,
                messages: None,
                extra: Default::default(),
                attachments: Vec::new(),
                enqueued_at: std::time::Instant::now(),
            },
            None,
        )
        .await
        .unwrap();
    test.state
        .conversation_authority
        .finish_active_turn(&address)
        .await;
    let next = request(&test.app,Method::PATCH,&resources,Some(json!({"expected_revision":3,"directories":[{"device_id":local_device_id(&test.state),"resource_root":docs}]}))).await;
    assert_eq!(next.status(), StatusCode::OK);
    assert_eq!(
        test.state
            .conversation_authority
            .queue_snapshot(&address)
            .await
            .len(),
        1
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn archive_and_reopen_preserve_idle_acp_collaboration_executor() {
    let test = build_app();
    let repo = test._temp.path().join("archive-acp-selection");
    std::fs::create_dir(&repo).unwrap();
    let created = response_json(
        request(
            &test.app,
            Method::POST,
            "/api/projects",
            Some(
                json!({"directories": [{"device_id": local_device_id(&test.state), "resource_root": repo}]}),
            ),
        )
        .await,
    )
    .await;
    let project_id = created["project"]["project_id"].as_str().unwrap();
    let chat_id = "00000000-0000-4000-8000-000000009950";
    let submitted = request(
        &test.app,
        Method::POST,
        &format!("/api/chats/{chat_id}/console/submit"),
        Some(json!({ "project_id": project_id,"agent_id":"default", "message":"Archive preserves executor"})),
    )
    .await;
    assert_eq!(submitted.status(), StatusCode::OK);
    wait_for_chat_idle(&test.state, chat_id).await;
    let address = ChatAddress::new(chat_id);
    let scope = ChatStore::resolve(&test.state.rua_root, address.clone());
    let store = rua_session::WorkspaceSessionStore::new(scope.root());
    let binding = json!({"kind":"acp_passthrough", "runner":"claude"});
    let collaboration = json!({
        "source": ChatAddress::new("missing-source"),
        "executor": {"kind":"acp", "runner":"claude"},
        "executor_agent_id":"default"
    });
    store
        .update_chat_header(chat_id, |header| {
            header.extra.insert("acp_binding".into(), binding.clone());
            header
                .extra
                .insert("collaboration".into(), collaboration.clone());
        })
        .await
        .unwrap();
    let preview = test
        .state
        .conversation_input_intake
        .inspect_project_lifecycle(&ProjectId::new(project_id))
        .await
        .unwrap();
    assert_eq!(
        preview.impact.unwrap().affected_chat_count,
        0,
        "Idle executor selection is not work to stop"
    );
    for action in ["archive", "reopen"] {
        let response = request(
            &test.app,
            Method::POST,
            &format!("/api/projects/{project_id}/{action}"),
            Some(json!({"confirmed":true})),
        )
        .await;
        let status = response.status();
        let body = response_json(response).await;
        assert_eq!(status, StatusCode::OK, "{action}: {body}");
        let header = store.load_chat_header(chat_id).await.unwrap().unwrap();
        assert_eq!(header.extra["acp_binding"], binding);
        assert_eq!(header.extra["collaboration"], collaboration);
        assert!(test
            .state
            .conversation_authority
            .active_turn(&address)
            .await
            .is_none());
        assert!(test
            .state
            .conversation_authority
            .queue_snapshot(&address)
            .await
            .is_empty());
    }
}
