use axum::http::{Method, StatusCode};
use serde_json::json;

use super::support::{build_app, create_fixture_plugin, read_json, request};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_returns_bare_array_on_fresh_workspace() {
    let test = build_app().await;

    let resp = request(&test.app, Method::GET, "/api/plugins", None).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert!(
        body.as_array().is_some(),
        "plugin list should be an array: {body}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn install_list_get_delete_lifecycle_freezes_rust_plugin_shape() {
    let test = build_app().await;
    let plugin_id = "contract-plugin";
    let source_path = create_fixture_plugin(&test.tmp, plugin_id);

    let install = request(
        &test.app,
        Method::POST,
        "/api/plugins/install",
        Some(json!({"source": source_path, "force": false})),
    )
    .await;
    assert_eq!(install.status(), StatusCode::OK);
    let install = read_json(install).await;
    assert_eq!(install["plugin_id"], plugin_id);
    assert_eq!(install["plugin_name"], "Contract Plugin");
    assert_eq!(install["status"], "installed");
    assert_eq!(install["installed"], true);
    assert!(install["source_path"]
        .as_str()
        .is_some_and(|p| p.ends_with(plugin_id)));

    let list = request(&test.app, Method::GET, "/api/plugins", None).await;
    assert_eq!(list.status(), StatusCode::OK);
    let list = read_json(list).await;
    let plugins = list.as_array().expect("plugins array");
    assert_eq!(plugins.len(), 1);
    assert_eq!(plugins[0]["id"], plugin_id);
    assert_eq!(plugins[0]["name"], plugin_id);
    assert_eq!(plugins[0]["version"], "1.0.0");
    assert_eq!(plugins[0]["enabled"], true);

    let detail = request(
        &test.app,
        Method::GET,
        &format!("/api/plugins/{plugin_id}"),
        None,
    )
    .await;
    assert_eq!(detail.status(), StatusCode::OK);
    let detail = read_json(detail).await;
    assert_eq!(detail["id"], plugin_id);
    assert_eq!(detail["entry_point"], "plugin.wasm");
    assert_eq!(detail["min_rua_version"], "0.2.0");
    assert_eq!(detail["author"], "rua-test");
    assert_eq!(detail["meta"]["key"], "value");
    assert_eq!(detail["startup_hooks"], json!([]));

    let delete = request(
        &test.app,
        Method::DELETE,
        &format!("/api/plugins/{plugin_id}"),
        None,
    )
    .await;
    assert_eq!(delete.status(), StatusCode::OK);
    let delete = read_json(delete).await;
    assert_eq!(delete["plugin_id"], plugin_id);
    assert_eq!(delete["removed"], true);
    assert_eq!(delete["requires_restart"], false);

    let final_list = request(&test.app, Method::GET, "/api/plugins", None).await;
    assert_eq!(final_list.status(), StatusCode::OK);
    assert_eq!(read_json(final_list).await, json!([]));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_and_invalid_paths_return_stable_errors() {
    let test = build_app().await;

    let install = request(
        &test.app,
        Method::POST,
        "/api/plugins/install",
        Some(json!({"source": "/tmp/rua-contract-missing-plugin-source"})),
    )
    .await;
    assert_eq!(install.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let install = read_json(install).await;
    assert_eq!(install["status"], "failed");
    assert_eq!(install["installed"], false);
    assert!(install["errors"]
        .as_array()
        .is_some_and(|errors| !errors.is_empty()));

    for (method, uri) in [
        (Method::GET, "/api/plugins/contract-missing-plugin"),
        (Method::DELETE, "/api/plugins/contract-missing-plugin"),
    ] {
        let resp = request(&test.app, method, uri, None).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let body = read_json(resp).await;
        assert!(
            body["detail"]
                .as_str()
                .is_some_and(|detail| detail.contains("contract-missing-plugin")),
            "expected missing plugin detail, got {body}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn qwenpaw_only_plugin_routes_are_not_rust_public_contract() {
    let test = build_app().await;

    for uri in [
        "/api/plugins/catalog",
        "/api/plugins/contract-missing-plugin/status",
        "/api/plugins/upload",
    ] {
        let resp = request(&test.app, Method::GET, uri, None).await;
        assert_eq!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "unexpected route: {uri}"
        );
    }
}
