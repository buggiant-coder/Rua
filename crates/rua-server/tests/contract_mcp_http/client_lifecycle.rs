use axum::http::{Method, StatusCode};
use serde_json::{json, Value};

use super::support::{build_app, read_json, read_text, request};

const AGENT_ID: &str = "mcp_contract_agent";

fn create_payload(client_key: &str, enabled: bool) -> Value {
    json!({
        "client_key": client_key,
        "client": {
            "name": "contract mcp client",
            "description": "created by contract tests",
            "enabled": enabled,
            "transport": "stdio",
            "command": "/bin/echo",
            "args": ["mcp"],
            "env": {
                "API_KEY": "sk-supersecretvalue1234"
            }
        }
    })
}

fn list_keys(payload: &Value) -> Vec<String> {
    payload
        .as_array()
        .expect("list response array")
        .iter()
        .map(|client| client["key"].as_str().expect("client key").to_string())
        .collect()
}

fn api_key(payload: &Value) -> &str {
    payload["env"]["API_KEY"].as_str().expect("API_KEY")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn root_scoped_client_crud_toggle_and_path_scoped_get_roundtrip() {
    let app = build_app();
    let client_key = "contract_mcp_client";

    let create = request(
        &app.app,
        Method::POST,
        "/api/mcp",
        Some(create_payload(client_key, true)),
        Some(AGENT_ID),
    )
    .await;
    assert_eq!(create.status(), StatusCode::CREATED);
    let created = read_json(create).await;
    assert_eq!(created["key"], client_key);
    assert_eq!(created["name"], "contract mcp client");
    assert_eq!(created["enabled"], true);
    assert_eq!(created["transport"], "stdio");
    assert_ne!(api_key(&created), "sk-supersecretvalue1234");
    assert!(
        api_key(&created).contains("****"),
        "secret should be display-masked, got {created:?}"
    );

    let list = request(&app.app, Method::GET, "/api/mcp", None, Some(AGENT_ID)).await;
    assert_eq!(list.status(), StatusCode::OK);
    assert!(list_keys(&read_json(list).await).contains(&client_key.to_string()));

    let get = request(
        &app.app,
        Method::GET,
        &format!("/api/mcp/{client_key}"),
        None,
        Some(AGENT_ID),
    )
    .await;
    assert_eq!(get.status(), StatusCode::OK);
    let got = read_json(get).await;
    assert_eq!(got["key"], client_key);
    assert_ne!(api_key(&got), "sk-supersecretvalue1234");

    let scoped_get = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{AGENT_ID}/mcp/{client_key}"),
        None,
        None,
    )
    .await;
    assert_eq!(scoped_get.status(), StatusCode::OK);
    assert_eq!(read_json(scoped_get).await["key"], client_key);

    let toggle_off = request(
        &app.app,
        Method::PATCH,
        &format!("/api/mcp/{client_key}/toggle"),
        None,
        Some(AGENT_ID),
    )
    .await;
    assert_eq!(toggle_off.status(), StatusCode::OK);
    assert_eq!(read_json(toggle_off).await["enabled"], false);

    let after_toggle = request(
        &app.app,
        Method::GET,
        &format!("/api/mcp/{client_key}"),
        None,
        Some(AGENT_ID),
    )
    .await;
    assert_eq!(read_json(after_toggle).await["enabled"], false);

    let toggle_on = request(
        &app.app,
        Method::PATCH,
        &format!("/api/mcp/{client_key}/toggle"),
        None,
        Some(AGENT_ID),
    )
    .await;
    assert_eq!(toggle_on.status(), StatusCode::OK);
    assert_eq!(read_json(toggle_on).await["enabled"], true);

    let delete = request(
        &app.app,
        Method::DELETE,
        &format!("/api/mcp/{client_key}"),
        None,
        Some(AGENT_ID),
    )
    .await;
    assert_eq!(delete.status(), StatusCode::OK);
    assert!(read_json(delete).await["message"]
        .as_str()
        .expect("delete message")
        .contains(client_key));

    let list_after = request(&app.app, Method::GET, "/api/mcp", None, Some(AGENT_ID)).await;
    assert_eq!(list_after.status(), StatusCode::OK);
    assert!(!list_keys(&read_json(list_after).await).contains(&client_key.to_string()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn duplicate_client_create_returns_conflict_detail() {
    let app = build_app();
    let client_key = "contract_mcp_duplicate";

    let first = request(
        &app.app,
        Method::POST,
        "/api/mcp",
        Some(create_payload(client_key, true)),
        Some(AGENT_ID),
    )
    .await;
    assert_eq!(first.status(), StatusCode::CREATED);

    let second = request(
        &app.app,
        Method::POST,
        "/api/mcp",
        Some(create_payload(client_key, true)),
        Some(AGENT_ID),
    )
    .await;
    assert_eq!(second.status(), StatusCode::CONFLICT);
    let body = read_json(second).await;
    assert!(
        body["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("already exists")),
        "expected conflict detail, got {body:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_client_routes_return_fastapi_compatible_detail_errors() {
    let app = build_app();
    let missing = "contract_mcp_missing";

    let get = request(
        &app.app,
        Method::GET,
        &format!("/api/mcp/{missing}"),
        None,
        Some(AGENT_ID),
    )
    .await;
    assert_eq!(get.status(), StatusCode::NOT_FOUND);
    assert!(read_json(get).await["detail"]
        .as_str()
        .is_some_and(|detail| detail.contains(missing)));

    let toggle = request(
        &app.app,
        Method::PATCH,
        &format!("/api/mcp/{missing}/toggle"),
        None,
        Some(AGENT_ID),
    )
    .await;
    assert_eq!(toggle.status(), StatusCode::NOT_FOUND);
    assert!(read_json(toggle).await["detail"]
        .as_str()
        .is_some_and(|detail| detail.contains(missing)));

    let delete = request(
        &app.app,
        Method::DELETE,
        &format!("/api/mcp/{missing}"),
        None,
        Some(AGENT_ID),
    )
    .await;
    assert_eq!(delete.status(), StatusCode::NOT_FOUND);
    assert!(read_json(delete).await["detail"]
        .as_str()
        .is_some_and(|detail| detail.contains(missing)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disabled_client_tools_short_circuit_to_empty_list_without_manager() {
    let app = build_app();
    let client_key = "contract_mcp_disabled_tools";

    let create = request(
        &app.app,
        Method::POST,
        "/api/mcp",
        Some(create_payload(client_key, false)),
        Some(AGENT_ID),
    )
    .await;
    assert_eq!(create.status(), StatusCode::CREATED);

    for path in [
        format!("/api/mcp/{client_key}/tools"),
        format!("/api/mcp/clients/{client_key}/tools"),
        format!("/api/agents/{AGENT_ID}/mcp/{client_key}/tools"),
    ] {
        let tools = request(&app.app, Method::GET, &path, None, Some(AGENT_ID)).await;
        assert_eq!(tools.status(), StatusCode::OK, "path={path}");
        assert_eq!(read_json(tools).await, json!([]), "path={path}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn qwenpaw_compat_toggle_and_tools_aliases_delegate_to_rust_canonical_routes() {
    let app = build_app();
    let client_key = "contract_mcp_qwenpaw_alias";

    let create = request(
        &app.app,
        Method::POST,
        "/api/mcp",
        Some(create_payload(client_key, false)),
        Some(AGENT_ID),
    )
    .await;
    assert_eq!(create.status(), StatusCode::CREATED);

    let tools = request(
        &app.app,
        Method::GET,
        &format!("/api/mcp/tools/{client_key}"),
        None,
        Some(AGENT_ID),
    )
    .await;
    assert_eq!(tools.status(), StatusCode::OK);
    assert_eq!(read_json(tools).await, json!([]));

    let toggle_on = request(
        &app.app,
        Method::PATCH,
        &format!("/api/mcp/toggle/{client_key}"),
        None,
        Some(AGENT_ID),
    )
    .await;
    assert_eq!(toggle_on.status(), StatusCode::OK);
    assert_eq!(read_json(toggle_on).await["enabled"], true);

    let missing_toggle = request(
        &app.app,
        Method::PATCH,
        "/api/mcp/toggle/missing-qwenpaw-alias",
        None,
        Some(AGENT_ID),
    )
    .await;
    assert_eq!(missing_toggle.status(), StatusCode::NOT_FOUND);
    assert!(read_json(missing_toggle).await["detail"]
        .as_str()
        .is_some_and(|detail| detail.contains("missing-qwenpaw-alias")));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_scoped_tool_toggle_persists_disabled_tools() {
    let app = build_app();
    let client_key = "contract_mcp_tool_toggle";

    let create = request(
        &app.app,
        Method::POST,
        "/api/mcp",
        Some(create_payload(client_key, true)),
        Some(AGENT_ID),
    )
    .await;
    assert_eq!(create.status(), StatusCode::CREATED);

    let disable = request(
        &app.app,
        Method::PATCH,
        &format!("/api/agents/{AGENT_ID}/mcp/{client_key}/tool-toggle/echo"),
        Some(json!({ "enabled": false })),
        None,
    )
    .await;
    let disable_status = disable.status();
    let disable_body = read_text(disable).await;
    assert_eq!(disable_status, StatusCode::OK, "{disable_body}");
    let disabled: Value = serde_json::from_str(&disable_body).expect("disable response json");
    assert_eq!(disabled["disabled_tools"], json!(["echo"]));

    let get_after_disable = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{AGENT_ID}/mcp/{client_key}"),
        None,
        None,
    )
    .await;
    assert_eq!(get_after_disable.status(), StatusCode::OK);
    assert_eq!(
        read_json(get_after_disable).await["disabled_tools"],
        json!(["echo"])
    );

    let enable = request(
        &app.app,
        Method::PATCH,
        &format!("/api/agents/{AGENT_ID}/mcp/{client_key}/tool-toggle/echo"),
        Some(json!({ "enabled": true })),
        None,
    )
    .await;
    assert_eq!(enable.status(), StatusCode::OK);
    assert_eq!(read_json(enable).await["disabled_tools"], json!([]));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_scoped_qwenpaw_compat_toggle_and_tools_aliases_use_path_agent() {
    let app = build_app();
    let path_agent = "mcp_contract_alias_path_agent";
    let header_agent = "mcp_contract_alias_header_agent";
    let client_key = "contract_scoped_mcp_alias_client";

    let create = request(
        &app.app,
        Method::POST,
        &format!("/api/agents/{path_agent}/mcp"),
        Some(create_payload(client_key, false)),
        Some(header_agent),
    )
    .await;
    assert_eq!(create.status(), StatusCode::CREATED);

    let tools = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{path_agent}/mcp/tools/{client_key}"),
        None,
        Some(header_agent),
    )
    .await;
    assert_eq!(tools.status(), StatusCode::OK);
    assert_eq!(read_json(tools).await, json!([]));

    let toggle_on = request(
        &app.app,
        Method::PATCH,
        &format!("/api/agents/{path_agent}/mcp/toggle/{client_key}"),
        None,
        Some(header_agent),
    )
    .await;
    assert_eq!(toggle_on.status(), StatusCode::OK);
    assert_eq!(read_json(toggle_on).await["enabled"], true);

    let header_get = request(
        &app.app,
        Method::GET,
        &format!("/api/mcp/{client_key}"),
        None,
        Some(header_agent),
    )
    .await;
    assert_eq!(
        header_get.status(),
        StatusCode::NOT_FOUND,
        "agent-scoped alias must not mutate the conflicting header agent"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn root_scoped_client_put_updates_mutable_fields_and_preserves_unset_values() {
    let app = build_app();
    let client_key = "contract_mcp_update";

    let create = request(
        &app.app,
        Method::POST,
        "/api/mcp",
        Some(create_payload(client_key, true)),
        Some(AGENT_ID),
    )
    .await;
    assert_eq!(create.status(), StatusCode::CREATED);

    let update = request(
        &app.app,
        Method::PUT,
        &format!("/api/mcp/{client_key}"),
        Some(json!({
            "name": "contract mcp updated",
            "description": "updated through PUT",
            "enabled": false,
            "args": ["after"],
            "custom_flag": "preserved-extra"
        })),
        Some(AGENT_ID),
    )
    .await;
    assert_eq!(update.status(), StatusCode::OK);
    let updated = read_json(update).await;
    assert_eq!(updated["key"], client_key);
    assert_eq!(updated["name"], "contract mcp updated");
    assert_eq!(updated["description"], "updated through PUT");
    assert_eq!(updated["enabled"], false);
    assert_eq!(updated["transport"], "stdio");
    assert_eq!(updated["command"], "/bin/echo");
    assert_eq!(updated["args"], json!(["after"]));
    assert_eq!(updated["custom_flag"], "preserved-extra");
    assert_ne!(api_key(&updated), "sk-supersecretvalue1234");

    let readback = request(
        &app.app,
        Method::GET,
        &format!("/api/mcp/{client_key}"),
        None,
        Some(AGENT_ID),
    )
    .await;
    assert_eq!(readback.status(), StatusCode::OK);
    let body = read_json(readback).await;
    assert_eq!(body["name"], "contract mcp updated");
    assert_eq!(body["description"], "updated through PUT");
    assert_eq!(body["enabled"], false);
    assert_eq!(body["transport"], "stdio");
    assert_eq!(body["command"], "/bin/echo");
    assert_eq!(body["args"], json!(["after"]));
    assert_eq!(body["custom_flag"], "preserved-extra");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_scoped_client_create_update_toggle_delete_roundtrip_uses_path_agent() {
    let app = build_app();
    let path_agent = "mcp_contract_path_agent";
    let header_agent = "mcp_contract_header_agent";
    let client_key = "contract_scoped_mcp_client";

    let create = request(
        &app.app,
        Method::POST,
        &format!("/api/agents/{path_agent}/mcp"),
        Some(create_payload(client_key, true)),
        Some(header_agent),
    )
    .await;
    assert_eq!(create.status(), StatusCode::CREATED);
    assert_eq!(read_json(create).await["key"], client_key);

    let header_list = request(&app.app, Method::GET, "/api/mcp", None, Some(header_agent)).await;
    assert_eq!(header_list.status(), StatusCode::OK);
    assert!(
        !list_keys(&read_json(header_list).await).contains(&client_key.to_string()),
        "path-scoped create must not write into the conflicting header agent"
    );

    let update = request(
        &app.app,
        Method::PUT,
        &format!("/api/agents/{path_agent}/mcp/{client_key}"),
        Some(json!({
            "name": "path scoped updated",
            "description": "path scoped description",
            "enabled": false,
            "args": ["path-scoped"]
        })),
        Some(header_agent),
    )
    .await;
    assert_eq!(update.status(), StatusCode::OK);
    let updated = read_json(update).await;
    assert_eq!(updated["name"], "path scoped updated");
    assert_eq!(updated["description"], "path scoped description");
    assert_eq!(updated["enabled"], false);
    assert_eq!(updated["args"], json!(["path-scoped"]));

    let toggle = request(
        &app.app,
        Method::PATCH,
        &format!("/api/agents/{path_agent}/mcp/{client_key}/toggle"),
        None,
        Some(header_agent),
    )
    .await;
    assert_eq!(toggle.status(), StatusCode::OK);
    assert_eq!(read_json(toggle).await["enabled"], true);

    let scoped_list = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{path_agent}/mcp"),
        None,
        Some(header_agent),
    )
    .await;
    assert_eq!(scoped_list.status(), StatusCode::OK);
    assert!(list_keys(&read_json(scoped_list).await).contains(&client_key.to_string()));

    let delete = request(
        &app.app,
        Method::DELETE,
        &format!("/api/agents/{path_agent}/mcp/{client_key}"),
        None,
        Some(header_agent),
    )
    .await;
    assert_eq!(delete.status(), StatusCode::OK);

    let list_after = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{path_agent}/mcp"),
        None,
        Some(header_agent),
    )
    .await;
    assert_eq!(list_after.status(), StatusCode::OK);
    assert!(!list_keys(&read_json(list_after).await).contains(&client_key.to_string()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tools_and_call_validate_client_config_before_manager_lookup() {
    let app = build_app();
    let missing = "contract_mcp_missing_for_tools";

    let missing_tools = request(
        &app.app,
        Method::GET,
        &format!("/api/mcp/{missing}/tools"),
        None,
        Some(AGENT_ID),
    )
    .await;
    assert_eq!(missing_tools.status(), StatusCode::NOT_FOUND);
    assert!(read_json(missing_tools).await["detail"]
        .as_str()
        .is_some_and(|detail| detail.contains(missing)));

    let missing_call = request(
        &app.app,
        Method::POST,
        &format!("/api/mcp/{missing}/call"),
        Some(json!({"tool": "echo", "arguments": {}})),
        Some(AGENT_ID),
    )
    .await;
    assert_eq!(missing_call.status(), StatusCode::NOT_FOUND);
    assert!(read_json(missing_call).await["detail"]
        .as_str()
        .is_some_and(|detail| detail.contains(missing)));

    let disabled_key = "contract_mcp_disabled_call";
    let create_disabled = request(
        &app.app,
        Method::POST,
        "/api/mcp",
        Some(create_payload(disabled_key, false)),
        Some(AGENT_ID),
    )
    .await;
    assert_eq!(create_disabled.status(), StatusCode::CREATED);

    let disabled_call = request(
        &app.app,
        Method::POST,
        &format!("/api/mcp/{disabled_key}/call"),
        Some(json!({"tool": "echo", "arguments": {}})),
        Some(AGENT_ID),
    )
    .await;
    assert_eq!(disabled_call.status(), StatusCode::BAD_REQUEST);
    assert!(read_json(disabled_call).await["detail"]
        .as_str()
        .is_some_and(|detail| detail.contains("disabled")));
}
