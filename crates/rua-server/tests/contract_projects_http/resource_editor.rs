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
