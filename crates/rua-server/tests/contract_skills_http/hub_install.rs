use axum::http::{Method, StatusCode};
use rua_server::endpoints::skills_hub::hub_error_to_response;
use rua_skills_hub::types::HubError;
use serde_json::json;

use super::support::{build_app, read_json, request_json};

#[tokio::test]
async fn hub_error_mapping_returns_fastapi_detail_bodies() {
    for (error, expected_status, expected_detail) in [
        (
            HubError::RateLimit,
            StatusCode::TOO_MANY_REQUESTS.as_u16(),
            "hub rate limit exceeded",
        ),
        (
            HubError::ZipLimit,
            StatusCode::PAYLOAD_TOO_LARGE.as_u16(),
            "bundle exceeded",
        ),
        (
            HubError::InvalidBundle("oops".into()),
            StatusCode::UNPROCESSABLE_ENTITY.as_u16(),
            "invalid bundle: oops",
        ),
        (
            HubError::UnsupportedSource("file://x".into()),
            StatusCode::BAD_REQUEST.as_u16(),
            "source not supported: file://x",
        ),
        (
            HubError::Http {
                status: 503,
                body: "down".into(),
            },
            503,
            "down",
        ),
        (HubError::Cancelled, 499, "cancelled"),
    ] {
        let response = hub_error_to_response(error);
        assert_eq!(response.status().as_u16(), expected_status);
        let body = read_json(response).await;
        assert!(
            body["detail"]
                .as_str()
                .is_some_and(|detail| detail.contains(expected_detail)),
            "expected detail containing {expected_detail:?}, got {body}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hub_install_start_rejects_empty_bundle_url_before_remote_work() {
    let test = build_app();

    let body = request_json(
        &test.app,
        Method::POST,
        "/api/agents/skills_alpha/skills/hub/install/start",
        Some("skills_alpha"),
        Some(json!({
            "bundle_url": "   ",
            "version": "1.0.0",
            "enable": true,
            "target_name": "ignored-empty-url",
            "installed_from": "contract",
            "market_meta": {"provider": "contract"}
        })),
        StatusCode::UNPROCESSABLE_ENTITY,
    )
    .await;

    assert!(body["detail"]
        .as_str()
        .is_some_and(|detail| detail.contains("bundle_url")));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hub_install_unknown_task_errors_are_fastapi_shaped() {
    let test = build_app();
    let task_id = "missing-contract-hub-install-task";

    let scoped_status = request_json(
        &test.app,
        Method::GET,
        &format!("/api/agents/skills_alpha/skills/hub/install/status/{task_id}"),
        None,
        None,
        StatusCode::NOT_FOUND,
    )
    .await;
    assert!(scoped_status["detail"]
        .as_str()
        .is_some_and(|detail| detail.contains(task_id)));

    let scoped_cancel = request_json(
        &test.app,
        Method::POST,
        &format!("/api/agents/skills_alpha/skills/hub/install/cancel/{task_id}"),
        None,
        None,
        StatusCode::NOT_FOUND,
    )
    .await;
    assert!(scoped_cancel["detail"]
        .as_str()
        .is_some_and(|detail| detail.contains(task_id)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn installation_status_and_cancel_are_scoped_to_the_creator_agent() {
    let test = build_app();
    let task = request_json(
        &test.app,
        Method::POST,
        "/api/agents/skills_alpha/skills/hub/install/start",
        Some("skills_beta"),
        Some(json!({"bundle_url":"file:///not-a-supported-hub-source", "enable":false})),
        StatusCode::OK,
    )
    .await;
    let id = task["task_id"].as_str().unwrap();
    for (method, action) in [(Method::GET, "status"), (Method::POST, "cancel")] {
        request_json(
            &test.app,
            method,
            &format!("/api/agents/skills_beta/skills/hub/install/{action}/{id}"),
            Some("skills_alpha"),
            None,
            StatusCode::NOT_FOUND,
        )
        .await;
    }
    let status = request_json(
        &test.app,
        Method::GET,
        &format!("/api/agents/skills_alpha/skills/hub/install/status/{id}"),
        Some("skills_beta"),
        None,
        StatusCode::OK,
    )
    .await;
    assert_eq!(status["task_id"], id);
    request_json(
        &test.app,
        Method::POST,
        &format!("/api/agents/skills_alpha/skills/hub/install/cancel/{id}"),
        Some("skills_beta"),
        None,
        StatusCode::OK,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn version_update_uses_path_agent_instead_of_header() {
    let test = build_app();
    request_json(&test.app, Method::POST, "/api/agents/skills_beta/skills", None,
        Some(json!({"name":"local-version", "content":super::support::skill_md("local-version", "local")})), StatusCode::CREATED).await;
    let root = test
        .tmp
        .path()
        .join("workspaces/skills_beta/skills/local-version");
    rua_skills::write_install_meta(
        &root,
        &rua_skills::InstallMeta {
            installed_from: "test".into(),
            source_url: "file:///unsupported".into(),
            installed_at: "2026-01-01T00:00:00Z".into(),
            updated_at: None,
            installed_version: "1".into(),
            content_hash: "different-content".into(),
            market_meta: None,
        },
    )
    .unwrap();
    for action in ["check-update", "update"] {
        let body = if action == "update" {
            Some(json!({"force":false}))
        } else {
            None
        };
        let response = request_json(
            &test.app,
            Method::POST,
            &format!("/api/agents/skills_beta/skills/local-version/{action}"),
            Some("skills_alpha"),
            body,
            if action == "update" {
                StatusCode::CONFLICT
            } else {
                StatusCode::BAD_GATEWAY
            },
        )
        .await;
        if action == "update" {
            assert_eq!(response["error"], "local_modified");
        } else {
            assert!(response["detail"]
                .as_str()
                .unwrap()
                .contains("source unreachable"));
        }
    }
}
