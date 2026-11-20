use super::support::{build_app, json as response_json, local_device_id, request};
use axum::http::{Method, StatusCode};
use serde_json::json;

#[tokio::test]
async fn failed_resource_validation_keeps_empty_project_retryable() {
    let test = build_app();
    let root = test._temp.path().join("source");
    std::fs::create_dir(&root).unwrap();
    let local = local_device_id(&test.state);
    let valid = json!({"device_id":local,"resource_root":root});
    let missing = json!({"device_id":local,"resource_root":root.join("missing")});

    let create = request(
        &test.app,
        Method::POST,
        "/api/projects",
        Some(json!({
            "display_name":"Empty","directories":[valid, missing]
        })),
    )
    .await;
    assert_eq!(create.status(), StatusCode::BAD_REQUEST);
    assert!(test.state.project_catalog.list().unwrap().is_empty());

    let create = request(
        &test.app,
        Method::POST,
        "/api/projects",
        Some(json!({
            "display_name":"Empty","directories":[]
        })),
    )
    .await;
    assert_eq!(create.status(), StatusCode::CREATED);
    let created = response_json(create).await;
    let project = &created["project"];
    assert_eq!(project["resources"], json!({"revision":1,"directories":[]}));
    let get = format!("/api/projects/{}", project["project_id"].as_str().unwrap());
    let uri = format!("{get}/resources");
    let failed = request(
        &test.app,
        Method::PATCH,
        &uri,
        Some(json!({
            "expected_revision":1,"directories":[valid,missing]
        })),
    )
    .await;
    assert_eq!(failed.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        &response_json(request(&test.app, Method::GET, &get, None).await).await,
        project
    );

    let retry = request(
        &test.app,
        Method::PATCH,
        &uri,
        Some(json!({
            "expected_revision":1,"directories":[valid]
        })),
    )
    .await;
    assert_eq!(retry.status(), StatusCode::OK);
    let result = response_json(retry).await;
    let directories = &result["project"]["resources"]["directories"];
    assert_eq!(result["project"]["resources"]["revision"], 2);
    assert_eq!(directories[0]["device_id"], local);
    assert_eq!(directories[0]["resource_root"], json!(root));
}

#[tokio::test]
async fn competing_replacements_commit_once_and_renaming_unavailable_directory_keeps_identity() {
    let test = build_app();
    let root = test._temp.path().join("source");
    let other = test._temp.path().join("other");
    std::fs::create_dir(&root).unwrap();
    std::fs::create_dir(&other).unwrap();
    let local = local_device_id(&test.state);
    let create = request(
        &test.app,
        Method::POST,
        "/api/projects",
        Some(json!({
            "directories":[{"device_id":local,"resource_root":root,"future_metadata":"retain"}]
        })),
    )
    .await;
    let project = &response_json(create).await["project"];
    let get = format!("/api/projects/{}", project["project_id"].as_str().unwrap());
    let uri = format!("{get}/resources");
    let directory_id = &project["resources"]["directories"][0]["directory_id"];
    std::fs::remove_dir(&root).unwrap();
    let rename = |name: &str| {
        Some(json!({"expected_revision":1,"directories":[{
            "directory_id":directory_id,"device_id":local,"resource_root":root,"display_name":name
        }]}))
    };
    let (first, second) = tokio::join!(
        request(&test.app, Method::PATCH, &uri, rename("First")),
        request(&test.app, Method::PATCH, &uri, rename("Second"))
    );
    let statuses = [first.status(), second.status()];
    assert!(
        statuses.contains(&StatusCode::OK) && statuses.contains(&StatusCode::CONFLICT),
        "{statuses:?}"
    );
    let winner = if first.status().is_success() {
        first
    } else {
        second
    };
    let saved = &response_json(winner).await["project"];
    assert_eq!(saved["resources"]["revision"], 2);
    let entry = &saved["resources"]["directories"][0];
    assert_eq!(&entry["directory_id"], directory_id);
    assert_eq!(entry["future_metadata"], "retain");
    assert_eq!(
        &response_json(request(&test.app, Method::GET, &get, None).await).await,
        saved
    );

    let moved = request(
        &test.app,
        Method::PATCH,
        &uri,
        Some(json!({"expected_revision":2,"directories":[{
            "directory_id":directory_id,"device_id":local,"resource_root":other
        }]})),
    )
    .await;
    assert_eq!(moved.status(), StatusCode::OK);
    let moved = response_json(moved).await;
    let entry = &moved["project"]["resources"]["directories"][0];
    assert_ne!(&entry["directory_id"], directory_id);
    assert!(entry.get("future_metadata").is_none());
    assert_eq!(entry["resource_root"], json!(other));
    assert!(!root.exists());
}

#[tokio::test]
async fn execution_defaults_apply_only_to_new_chats_and_directory_patch_preserves_authority() {
    let test = build_app();
    let local = local_device_id(&test.state);
    let first = test._temp.path().join("first execution");
    let second = test._temp.path().join("second execution");
    for path in [&first, &second] {
        std::fs::create_dir(path).unwrap();
    }
    let create=request(&test.app,Method::POST,"/api/projects",Some(json!({"display_name":"Locations","directories":[],"execution_default":{"device_id":local,"path":first}}))).await;
    assert_eq!(create.status(), StatusCode::CREATED);
    let project = response_json(create).await["project"].clone();
    let id = project["project_id"].as_str().unwrap();
    let uri = format!("/api/projects/{id}/execution-default");
    let chat = uuid::Uuid::new_v4().to_string();
    let second_chat = uuid::Uuid::new_v4().to_string();
    let managed_chat = uuid::Uuid::new_v4().to_string();
    for (index, chat_id) in [&chat, &second_chat, &managed_chat].into_iter().enumerate() {
        let mut submit =
            json!({"project_id":id,"agent_id":"default","message":"Check execution directory"});
        if index == 2 {
            submit["execution_directory"] = json!({"device_id":local});
        }
        let route = format!("/api/chats/{chat_id}");
        let submit = response_json(
            request(
                &test.app,
                Method::POST,
                &format!("{route}/console/submit"),
                Some(submit),
            )
            .await,
        )
        .await;
        assert_eq!(submit["accepted"], true, "{submit}");
        super::support::wait_for_chat_idle(&test.state, chat_id).await;
        let history = response_json(request(&test.app, Method::GET, &route, None).await).await;
        let cwd = &history["meta"]["execution_directory"];
        assert_eq!(cwd["device_id"], local);
        assert_eq!(cwd["managed"], index == 2);
        match index {
            0 => {
                assert_eq!(cwd["path"], json!(first));
                let result = request(
                    &test.app,
                    Method::PUT,
                    &uri,
                    Some(json!({"execution_default":{"device_id":local,"path":second}})),
                )
                .await;
                assert_eq!(result.status(), StatusCode::OK);
                assert_eq!(
                    response_json(result).await["execution_default"]["path"],
                    json!(second)
                );
            }
            1 => assert_eq!(cwd["path"], json!(second)),
            _ => assert_ne!(cwd["path"], json!(second)),
        }
    }
    let chat_uri = format!("/api/chats/{chat}");
    let history = response_json(request(&test.app, Method::GET, &chat_uri, None).await).await;
    assert_eq!(history["meta"]["execution_directory"]["path"], json!(first));
    let update = format!("{chat_uri}/execution-directory");
    std::fs::remove_dir(&first).unwrap();
    let invalid = request(
        &test.app,
        Method::PATCH,
        &update,
        Some(json!({"device_id":local,"path":first})),
    )
    .await;
    assert!(!invalid.status().is_success());
    let other = request(
        &test.app,
        Method::PATCH,
        &update,
        Some(json!({"device_id":"another-device","path":second})),
    )
    .await;
    assert_eq!(other.status(), StatusCode::CONFLICT);
    let retained = response_json(request(&test.app, Method::GET, &chat_uri, None).await).await;
    assert_eq!(
        retained["meta"]["execution_directory"],
        history["meta"]["execution_directory"]
    );
    let repaired = request(
        &test.app,
        Method::PATCH,
        &update,
        Some(json!({"device_id":local,"path":second})),
    )
    .await;
    let status = repaired.status();
    let result = response_json(repaired).await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(
        result,
        json!({"device_id":local,"path":second,"managed":false})
    );
    let after = response_json(request(&test.app, Method::GET, &chat_uri, None).await).await;
    assert_eq!(after["meta"]["execution_directory"], result);
    assert_eq!(
        after["meta"]["authority_placement"],
        history["meta"]["authority_placement"]
    );
    let error = request(
        &test.app,
        Method::PUT,
        &uri,
        Some(json!({"execution_default":{"device_id":local,"path":first}})),
    )
    .await;
    assert_eq!(error.status(), StatusCode::BAD_REQUEST);
    let unchanged =
        response_json(request(&test.app, Method::GET, &format!("/api/projects/{id}"), None).await)
            .await;
    assert_eq!(unchanged["execution_default"]["path"], json!(second));
    assert_eq!(unchanged["resources"], project["resources"]);
    let empty = request(
        &test.app,
        Method::PUT,
        &uri,
        Some(json!({"execution_default":null})),
    )
    .await;
    assert_eq!(empty.status(), StatusCode::OK);
    assert!(response_json(empty).await["execution_default"].is_null());
}
