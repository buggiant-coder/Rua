use reqwest::StatusCode;
use rua_cli::agent::{agent_info_endpoint, agent_list_endpoint, format_agent_list_human};
use rua_cli::http_client::format_http_error;
use serde_json::json;

#[test]
fn list_and_info_use_rust_agent_api_endpoints() {
    assert_eq!(agent_list_endpoint(), "/api/agents");
    assert_eq!(agent_info_endpoint("research"), "/api/agents/research");
}

#[test]
fn list_human_output_reports_empty_agent_list() {
    let output = format_agent_list_human(&json!({"agents": []}));

    assert_eq!(output, "No agents found.\n");
}

#[test]
fn list_human_output_formats_enabled_and_disabled_agents() {
    let output = format_agent_list_human(&json!({
        "agents": [
            {
                "id": "research",
                "name": "Research Bot",
                "enabled": true
            },
            {
                "id": "qa",
                "name": "QA Bot",
                "enabled": false
            }
        ]
    }));

    assert_eq!(
        output,
        "Agents:\n  research: Research Bot\n  qa: QA Bot [disabled]\n"
    );
}

#[test]
fn list_human_output_keeps_existing_fallbacks_for_partial_agent_rows() {
    let output = format_agent_list_human(&json!({
        "agents": [
            {},
            {
                "id": "nameless",
                "enabled": true
            }
        ]
    }));

    assert_eq!(
        output,
        "Agents:\n  ?: unnamed [disabled]\n  nameless: unnamed\n"
    );
}

#[test]
fn list_human_output_ignores_non_array_agents_like_existing_cli() {
    assert_eq!(format_agent_list_human(&json!({})), "No agents found.\n");
    assert_eq!(format_agent_list_human(&json!({"agents": null})), "");
    assert_eq!(format_agent_list_human(&json!({"agents": {}})), "");
}

#[test]
fn info_api_error_detail_is_formatted_for_human_cli_output() {
    let error = format_http_error(
        "GET",
        "http://127.0.0.1:8765/api/agents/research",
        StatusCode::NOT_FOUND,
        r#"{"detail":"Agent not found: research"}"#,
    );

    assert_eq!(
        error,
        "GET http://127.0.0.1:8765/api/agents/research returned 404 Not Found: Agent not found: research"
    );
}
