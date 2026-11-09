use axum::http::{Method, StatusCode};
use serde_json::{json, Value};

use super::support::{build_app, read_json, request, request_with_agent};

const AGENT_ID: &str = "tool_contract_agent";
const OTHER_AGENT_ID: &str = "tool_contract_other_agent";
const CANONICAL_TOOL_NAMES: &[&str] = &["bash", "edit", "glob", "grep", "read", "edit"];

fn agent_tools_path() -> String {
    format!("/api/agents/{AGENT_ID}/tools")
}

fn tool_path(tool_name: &str, suffix: &str) -> String {
    format!("/api/agents/{AGENT_ID}/tools/{tool_name}/{suffix}")
}

fn scoped_tool_path(agent_id: &str, tool_name: &str, suffix: &str) -> String {
    format!("/api/agents/{agent_id}/tools/{tool_name}/{suffix}")
}

fn other_agent_tools_path() -> String {
    format!("/api/agents/{OTHER_AGENT_ID}/tools")
}

fn scoped_tools_path(agent_id: &str) -> String {
    format!("/api/agents/{agent_id}/tools")
}

fn tool_by_name<'a>(tools: &'a [Value], name: &str) -> &'a Value {
    tools
        .iter()
        .find(|tool| tool["name"] == name)
        .unwrap_or_else(|| panic!("missing tool {name} in {tools:?}"))
}

fn assert_tool_info_response(tool: &Value, expected_name: &str, expected_async: bool) {
    assert_eq!(tool["name"], expected_name);
    assert!(tool["description"].is_string());
    assert!(tool["enabled"].is_boolean());
    assert_eq!(tool["async_execution"], expected_async);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_scoped_list_returns_tool_info_dto() {
    let test = build_app();

    let resp = request(&test.app, Method::GET, &agent_tools_path(), None).await;

    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    let tools = body.as_array().expect("tools response must be an array");
    assert!(!tools.is_empty(), "tools list must not be empty");

    let bash = tool_by_name(tools, "bash");
    assert!(bash["description"].is_string());
    assert!(bash["enabled"].is_boolean());
    assert!(bash["async_execution"].is_boolean());
    assert!(
        !tools.iter().any(|tool| tool["name"] == "Obsolete_Tool"),
        "public obsolete names must not leak into HTTP list responses"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_scoped_list_and_toggle_include_collaboration_tools() {
    let test = build_app();
    let list = request(&test.app, Method::GET, &agent_tools_path(), None).await;
    assert_eq!(list.status(), StatusCode::OK);
    let tools = read_json(list).await;
    let tools = tools.as_array().expect("tools array");
    for name in [
        "chat_send",
        "chat_read",
        "chat_stop",
        "sync_ls",
        "sync_send",
    ] {
        assert_eq!(tool_by_name(tools, name)["enabled"], true);
    }

    let disable = request(
        &test.app,
        Method::PATCH,
        &tool_path("chat_send", "toggle"),
        Some(json!({"enabled": false})),
    )
    .await;
    assert_eq!(disable.status(), StatusCode::OK);
    assert_eq!(read_json(disable).await["enabled"], false);

    let list = request(&test.app, Method::GET, &agent_tools_path(), None).await;
    let tools = read_json(list).await;
    assert_eq!(
        tool_by_name(tools.as_array().expect("tools array"), "chat_send")["enabled"],
        false
    );

    let restore = request(
        &test.app,
        Method::PATCH,
        &tool_path("chat_send", "toggle"),
        Some(json!({"enabled": true})),
    )
    .await;
    assert_eq!(restore.status(), StatusCode::OK);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_scoped_toggle_without_body_flips_and_can_restore() {
    let test = build_app();
    let list = request(&test.app, Method::GET, &agent_tools_path(), None).await;
    let tools = read_json(list).await;
    let tools = tools.as_array().expect("tools array");
    let baseline = tool_by_name(tools, "bash")["enabled"]
        .as_bool()
        .expect("enabled bool");

    let toggle = request(&test.app, Method::PATCH, &tool_path("bash", "toggle"), None).await;
    assert_eq!(toggle.status(), StatusCode::OK);
    let toggled = read_json(toggle).await;
    assert_eq!(toggled["name"], "bash");
    assert_eq!(toggled["enabled"], !baseline);

    let restore = request(&test.app, Method::PATCH, &tool_path("bash", "toggle"), None).await;
    assert_eq!(restore.status(), StatusCode::OK);
    let restored = read_json(restore).await;
    assert_eq!(restored["enabled"], baseline);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_scoped_toggle_with_explicit_enabled_body_is_idempotent() {
    let test = build_app();

    let disable = request(
        &test.app,
        Method::PATCH,
        &tool_path("bash", "toggle"),
        Some(json!({"enabled": false})),
    )
    .await;
    assert_eq!(disable.status(), StatusCode::OK);
    assert_eq!(read_json(disable).await["enabled"], false);

    let disable_again = request(
        &test.app,
        Method::PATCH,
        &tool_path("bash", "toggle"),
        Some(json!({"enabled": false})),
    )
    .await;
    assert_eq!(disable_again.status(), StatusCode::OK);
    assert_eq!(
        read_json(disable_again).await["enabled"],
        false,
        "explicit enabled=false must set the target state, not blindly flip"
    );

    let list = request(&test.app, Method::GET, &agent_tools_path(), None).await;
    assert_eq!(list.status(), StatusCode::OK);
    let tools = read_json(list).await;
    let tools = tools.as_array().expect("tools array");
    assert_eq!(tool_by_name(tools, "bash")["enabled"], false);

    let enable = request(
        &test.app,
        Method::PATCH,
        &tool_path("bash", "toggle"),
        Some(json!({"enabled": true})),
    )
    .await;
    assert_eq!(enable.status(), StatusCode::OK);
    assert_eq!(read_json(enable).await["enabled"], true);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_scoped_toggle_is_agent_isolated() {
    let test = build_app();

    let agent_list = request(&test.app, Method::GET, &agent_tools_path(), None).await;
    assert_eq!(agent_list.status(), StatusCode::OK);
    let agent_tools = read_json(agent_list).await;
    let agent_tools = agent_tools.as_array().expect("agent tools array");
    let bash_before = tool_by_name(agent_tools, "bash")["enabled"]
        .as_bool()
        .expect("enabled bool");

    let other_list = request(&test.app, Method::GET, &other_agent_tools_path(), None).await;
    assert_eq!(other_list.status(), StatusCode::OK);
    let other_tools = read_json(other_list).await;
    let other_tools = other_tools.as_array().expect("other tools array");
    let other_bash_before = tool_by_name(other_tools, "bash")["enabled"]
        .as_bool()
        .expect("other enabled bool");

    let toggle = request(
        &test.app,
        Method::PATCH,
        &scoped_tool_path(AGENT_ID, "bash", "toggle"),
        None,
    )
    .await;
    assert_eq!(toggle.status(), StatusCode::OK);
    assert_eq!(read_json(toggle).await["enabled"], !bash_before);

    let other_after = request(
        &test.app,
        Method::GET,
        &scoped_tools_path(OTHER_AGENT_ID),
        None,
    )
    .await;
    assert_eq!(other_after.status(), StatusCode::OK);
    let other_after_body = read_json(other_after).await;
    let other_after_tools = other_after_body.as_array().expect("other tools array");
    assert_eq!(
        tool_by_name(other_after_tools, "bash")["enabled"],
        other_bash_before,
        "tool toggles must stay scoped to the selected agent"
    );

    let restore = request(
        &test.app,
        Method::PATCH,
        &scoped_tool_path(AGENT_ID, "bash", "toggle"),
        None,
    )
    .await;
    assert_eq!(restore.status(), StatusCode::OK);
    assert_eq!(read_json(restore).await["enabled"], bash_before);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_scoped_tool_routes_prefer_path_agent_over_conflicting_header() {
    let test = build_app();

    let path_agent_before = request(&test.app, Method::GET, &agent_tools_path(), None).await;
    assert_eq!(path_agent_before.status(), StatusCode::OK);
    let path_agent_tools = read_json(path_agent_before).await;
    let path_agent_tools = path_agent_tools.as_array().expect("path agent tools");
    let path_bash_before = tool_by_name(path_agent_tools, "bash")["enabled"]
        .as_bool()
        .expect("path enabled bool");

    let header_agent_before = request(
        &test.app,
        Method::GET,
        &scoped_tools_path(OTHER_AGENT_ID),
        None,
    )
    .await;
    assert_eq!(header_agent_before.status(), StatusCode::OK);
    let header_agent_tools = read_json(header_agent_before).await;
    let header_agent_tools = header_agent_tools.as_array().expect("header agent tools");
    let header_bash_before = tool_by_name(header_agent_tools, "bash")["enabled"]
        .as_bool()
        .expect("header enabled bool");

    let toggle = request_with_agent(
        &test.app,
        Method::PATCH,
        &scoped_tool_path(AGENT_ID, "bash", "toggle"),
        None,
        Some(OTHER_AGENT_ID),
    )
    .await;
    assert_eq!(toggle.status(), StatusCode::OK);
    assert_eq!(read_json(toggle).await["enabled"], !path_bash_before);

    let path_agent_after = request(&test.app, Method::GET, &agent_tools_path(), None).await;
    assert_eq!(path_agent_after.status(), StatusCode::OK);
    let path_agent_tools = read_json(path_agent_after).await;
    let path_agent_tools = path_agent_tools.as_array().expect("path agent tools");
    assert_eq!(
        tool_by_name(path_agent_tools, "bash")["enabled"],
        !path_bash_before,
        "path-scoped tools route must mutate the path agent"
    );

    let header_agent_after = request(
        &test.app,
        Method::GET,
        &scoped_tools_path(OTHER_AGENT_ID),
        None,
    )
    .await;
    assert_eq!(header_agent_after.status(), StatusCode::OK);
    let header_agent_tools = read_json(header_agent_after).await;
    let header_agent_tools = header_agent_tools.as_array().expect("header agent tools");
    assert_eq!(
        tool_by_name(header_agent_tools, "bash")["enabled"],
        header_bash_before,
        "conflicting X-Agent-Id must not redirect a path-scoped tools route"
    );

    let restore = request(
        &test.app,
        Method::PATCH,
        &scoped_tool_path(AGENT_ID, "bash", "toggle"),
        None,
    )
    .await;
    assert_eq!(restore.status(), StatusCode::OK);
    assert_eq!(read_json(restore).await["enabled"], path_bash_before);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_scoped_async_execution_patch_roundtrips_through_list() {
    let test = build_app();

    let update = request(
        &test.app,
        Method::PATCH,
        &tool_path("bash", "async-execution"),
        Some(json!({"async_execution": true})),
    )
    .await;
    assert_eq!(update.status(), StatusCode::OK);
    let updated = read_json(update).await;
    assert_tool_info_response(&updated, "bash", true);

    let list = request(&test.app, Method::GET, &agent_tools_path(), None).await;
    let tools = read_json(list).await;
    let tools = tools.as_array().expect("tools array");
    assert_eq!(tool_by_name(tools, "bash")["async_execution"], true);

    let restore = request(
        &test.app,
        Method::PATCH,
        &tool_path("bash", "async-execution"),
        Some(json!({"async_execution": false})),
    )
    .await;
    assert_eq!(restore.status(), StatusCode::OK);
    let restored = read_json(restore).await;
    assert_eq!(restored["async_execution"], false);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_scoped_async_execution_accepts_only_canonical_tool_names() {
    let test = build_app();

    for tool_name in CANONICAL_TOOL_NAMES {
        let update = request(
            &test.app,
            Method::PATCH,
            &tool_path(tool_name, "async-execution"),
            Some(json!({"async_execution": true})),
        )
        .await;
        assert_eq!(update.status(), StatusCode::OK, "{tool_name}");
        let updated = read_json(update).await;
        assert_tool_info_response(&updated, tool_name, true);
    }

    let list = request(&test.app, Method::GET, &agent_tools_path(), None).await;
    let tools = read_json(list).await;
    let tools = tools.as_array().expect("tools array");
    for tool_name in CANONICAL_TOOL_NAMES {
        assert_eq!(
            tool_by_name(tools, tool_name)["async_execution"],
            true,
            "{tool_name} should roundtrip through list"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_scoped_async_execution_is_agent_isolated() {
    let test = build_app();

    let update = request(
        &test.app,
        Method::PATCH,
        &tool_path("bash", "async-execution"),
        Some(json!({"async_execution": true})),
    )
    .await;
    assert_eq!(update.status(), StatusCode::OK);

    let other_list = request(&test.app, Method::GET, &other_agent_tools_path(), None).await;
    assert_eq!(other_list.status(), StatusCode::OK);
    let other_tools = read_json(other_list).await;
    let other_tools = other_tools.as_array().expect("other tools array");
    assert_eq!(
        tool_by_name(other_tools, "bash")["async_execution"],
        false,
        "async_execution updates must not leak between agents"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_scoped_tool_config_returns_config_object() {
    let test = build_app();

    let configured = request(&test.app, Method::GET, &tool_path("bash", "config"), None).await;
    assert_eq!(configured.status(), StatusCode::OK);
    let body = read_json(configured).await;
    assert_eq!(body["cwd_policy"], "workspace");
    assert_eq!(body["max_output_chars"], 12000);

    let defaulted = request(
        &test.app,
        Method::GET,
        &tool_path("time_now", "config"),
        None,
    )
    .await;
    assert_eq!(defaulted.status(), StatusCode::OK);
    assert_eq!(
        read_json(defaulted).await,
        json!({}),
        "tools without persisted config return a stable empty object"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_scoped_tool_config_prefers_path_agent_over_conflicting_header() {
    let test = build_app();

    let path_config = request_with_agent(
        &test.app,
        Method::GET,
        &tool_path("bash", "config"),
        None,
        Some(OTHER_AGENT_ID),
    )
    .await;
    assert_eq!(path_config.status(), StatusCode::OK);
    assert_eq!(
        read_json(path_config).await,
        json!({
            "cwd_policy": "workspace",
            "max_output_chars": 12000
        }),
        "path-scoped config reads must ignore conflicting X-Agent-Id"
    );

    let header_agent_config = request(
        &test.app,
        Method::GET,
        &scoped_tool_path(OTHER_AGENT_ID, "bash", "config"),
        None,
    )
    .await;
    assert_eq!(header_agent_config.status(), StatusCode::OK);
    assert_eq!(
        read_json(header_agent_config).await,
        json!({}),
        "the conflicting header agent has no Bash config fixture"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_scoped_tool_config_put_masks_encrypts_and_preserves_secret() {
    let test = build_app();

    let save = request(
        &test.app,
        Method::PUT,
        &tool_path("web_search", "config"),
        Some(json!({
            "config": {
                "mode": "ordered_fallback",
                "providers": [
                    {"id": "tavily", "enabled": true},
                    {"id": "brave", "enabled": true, "api_key": "brave-secret"}
                ]
            }
        })),
    )
    .await;
    assert_eq!(save.status(), StatusCode::OK);
    let saved = read_json(save).await;
    let masked_key = saved["providers"][1]["api_key"]
        .as_str()
        .expect("api key should be returned masked")
        .to_string();
    assert_ne!(masked_key, "brave-secret");
    assert!(masked_key.contains('*'));

    let raw = std::fs::read_to_string(test.agent_json_path(AGENT_ID)).expect("agent json");
    assert!(
        !raw.contains("brave-secret"),
        "tool config secrets must not be stored as plaintext"
    );
    assert!(raw.contains("rua_encrypted"));

    let get = request(
        &test.app,
        Method::GET,
        &tool_path("web_search", "config"),
        None,
    )
    .await;
    assert_eq!(get.status(), StatusCode::OK);
    assert_eq!(read_json(get).await["providers"][1]["api_key"], masked_key);

    let preserve = request(
        &test.app,
        Method::PUT,
        &tool_path("web_search", "config"),
        Some(json!({
            "config": {
                "mode": "ordered_fallback",
                "providers": [
                    {"id": "tavily", "enabled": false},
                    {"id": "brave", "enabled": true, "api_key": masked_key}
                ]
            }
        })),
    )
    .await;
    assert_eq!(preserve.status(), StatusCode::OK);

    let store = rua_tool::ToolConfigStore::new(test.state.agent_tools_cells.clone())
        .with_device_key(*test.state.device_key.as_bytes());
    let unmasked = store
        .get_config_unmasked(Some(AGENT_ID), "web_search")
        .await
        .expect("unmasked config");
    assert_eq!(unmasked["providers"][1]["api_key"], "brave-secret");
    assert_eq!(unmasked["providers"][0]["enabled"], false);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_scoped_tool_config_rejects_unknown_tool() {
    let test = build_app();

    let resp = request(
        &test.app,
        Method::GET,
        &tool_path("definitely_not_a_tool", "config"),
        None,
    )
    .await;

    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let body = read_json(resp).await;
    assert!(
        body["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("definitely_not_a_tool")),
        "expected FastAPI-compatible detail body, got {body:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_scoped_async_execution_rejects_missing_required_field() {
    let test = build_app();

    let resp = request(
        &test.app,
        Method::PATCH,
        &tool_path("bash", "async-execution"),
        Some(json!({})),
    )
    .await;

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = read_json(resp).await;
    assert!(
        body["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("async_execution")),
        "expected missing async_execution detail body, got {body:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_tool_async_execution_returns_fastapi_compatible_detail_error() {
    let test = build_app();

    let resp = request(
        &test.app,
        Method::PATCH,
        &tool_path("definitely_not_a_tool", "async-execution"),
        Some(json!({"async_execution": true})),
    )
    .await;

    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let body = read_json(resp).await;
    assert!(
        body["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("definitely_not_a_tool")),
        "expected FastAPI-compatible detail body, got {body:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_tool_returns_fastapi_compatible_detail_error() {
    let test = build_app();

    let resp = request(
        &test.app,
        Method::PATCH,
        &tool_path("definitely_not_a_tool", "toggle"),
        None,
    )
    .await;

    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let body = read_json(resp).await;
    assert!(
        body["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("definitely_not_a_tool")),
        "expected FastAPI-compatible detail body, got {body:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_tool_name_is_not_a_public_http_tool_name() {
    let test = build_app();

    let baseline = request(&test.app, Method::GET, &agent_tools_path(), None).await;
    let tools = read_json(baseline).await;
    let tools = tools.as_array().expect("tools array");
    let bash_before = tool_by_name(tools, "bash")["enabled"]
        .as_bool()
        .expect("enabled bool");

    let unknown_resp = request(
        &test.app,
        Method::PATCH,
        &tool_path("Obsolete_Tool", "toggle"),
        None,
    )
    .await;
    assert_eq!(unknown_resp.status(), StatusCode::NOT_FOUND);
    let unknown_body = read_json(unknown_resp).await;
    assert!(
        unknown_body["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("Obsolete_Tool")),
        "expected unknown tool to be rejected with detail body, got {unknown_body:?}"
    );

    let after = request(&test.app, Method::GET, &agent_tools_path(), None).await;
    let tools = read_json(after).await;
    let tools = tools.as_array().expect("tools array");
    assert_eq!(tool_by_name(tools, "bash")["enabled"], bash_before);

    let unknown_async_resp = request(
        &test.app,
        Method::PATCH,
        &tool_path("Obsolete_Tool", "async-execution"),
        Some(json!({"async_execution": true})),
    )
    .await;
    assert_eq!(unknown_async_resp.status(), StatusCode::NOT_FOUND);
    let unknown_async_body = read_json(unknown_async_resp).await;
    assert!(
        unknown_async_body["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("Obsolete_Tool")),
        "expected unknown tool async update to be rejected with detail body, got {unknown_async_body:?}"
    );

    let after_async = request(&test.app, Method::GET, &agent_tools_path(), None).await;
    let tools = read_json(after_async).await;
    let tools = tools.as_array().expect("tools array");
    assert_eq!(tool_by_name(tools, "bash")["async_execution"], false);
}
