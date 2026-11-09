use clap::Parser;
use reqwest::StatusCode;
use rua_cli::agent::{
    agent_delete_endpoint, agent_info_endpoint, delete_confirmation_allows_proceed,
    is_workspace_dir_inside_working_dir, workspace_dir_for_local_removal,
    workspace_dir_from_agent_info,
};
use rua_cli::http_client::format_http_error;
use rua_cli::{Cli, Commands};
use serde_json::json;
use std::path::PathBuf;

#[test]
fn delete_uses_rust_agent_api_endpoint_for_info_and_delete() {
    assert_eq!(agent_info_endpoint("research"), "/api/agents/research");
    assert_eq!(agent_delete_endpoint("research"), "/api/agents/research");
}

#[test]
fn delete_confirmation_accepts_only_explicit_yes_answers() {
    for input in ["y", "Y", "yes", "YES", " yes\n"] {
        assert!(
            delete_confirmation_allows_proceed(input),
            "{input:?} should confirm delete"
        );
    }

    for input in ["", "\n", "n", "no", " yep ", "是"] {
        assert!(
            !delete_confirmation_allows_proceed(input),
            "{input:?} should not confirm delete"
        );
    }
}

#[test]
fn delete_cli_parses_yes_and_remove_workspace_flags() {
    let cli = Cli::try_parse_from([
        "rua",
        "agent",
        "delete",
        "research",
        "--yes",
        "--remove-workspace",
    ])
    .expect("delete --yes --remove-workspace should parse");

    assert!(matches!(cli.command, Commands::Agent(_)));
}

#[test]
fn delete_api_detail_errors_are_rendered_without_raw_fastapi_wrapper() {
    let error = format_http_error(
        "DELETE",
        "http://127.0.0.1:8765/api/agents/default",
        StatusCode::BAD_REQUEST,
        r#"{"detail":"Cannot delete the default agent"}"#,
    );

    assert_eq!(
        error,
        "DELETE http://127.0.0.1:8765/api/agents/default returned 400 Bad Request: Cannot delete the default agent"
    );
    assert!(!error.contains(r#""detail""#));
}

#[test]
fn missing_agent_detail_errors_keep_agent_id_readable() {
    let error = format_http_error(
        "GET",
        "http://127.0.0.1:8765/api/agents/missing",
        StatusCode::NOT_FOUND,
        r#"{"detail":"missing"}"#,
    );

    assert_eq!(
        error,
        "GET http://127.0.0.1:8765/api/agents/missing returned 404 Not Found: missing"
    );
}

#[test]
fn workspace_dir_is_loaded_from_agent_info_when_removal_is_requested() {
    let info = json!({
        "id": "research",
        "workspace_dir": "/tmp/rua/research",
    });

    assert_eq!(
        workspace_dir_from_agent_info(&info),
        Some(PathBuf::from("/tmp/rua/research"))
    );
}

#[test]
fn workspace_dir_expands_tilde_and_ignores_missing_or_non_string_values() {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));

    assert_eq!(
        workspace_dir_from_agent_info(&json!({"workspace_dir": "~/agents/research"})),
        Some(home.join("agents/research"))
    );
    assert_eq!(
        workspace_dir_from_agent_info(&json!({"workspace_dir": "~"})),
        Some(home)
    );
    assert_eq!(workspace_dir_from_agent_info(&json!({})), None);
    assert_eq!(
        workspace_dir_from_agent_info(&json!({"workspace_dir": false})),
        None
    );
}

#[test]
fn remove_workspace_accepts_agent_workspace_below_working_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let working_dir = tmp.path().join("rua");
    let workspace_dir = working_dir.join("workspaces/research");
    std::fs::create_dir_all(&workspace_dir).unwrap();

    let selected = workspace_dir_for_local_removal(
        &json!({"workspace_dir": workspace_dir.display().to_string()}),
        &working_dir,
    )
    .expect("workspace under working dir should be removable");

    assert_eq!(selected, Some(workspace_dir.canonicalize().unwrap()));
}

#[test]
fn remove_workspace_resolves_relative_paths_under_working_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let working_dir = tmp.path().join("rua");
    std::fs::create_dir_all(&working_dir).unwrap();

    let selected = workspace_dir_for_local_removal(
        &json!({"workspace_dir": "workspaces/research"}),
        &working_dir,
    )
    .expect("relative workspace path should be scoped to working dir");

    assert_eq!(
        selected,
        Some(
            working_dir
                .canonicalize()
                .unwrap()
                .join("workspaces/research")
        )
    );
}

#[test]
fn remove_workspace_rejects_paths_outside_working_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let working_dir = tmp.path().join("rua");
    let outside_dir = tmp.path().join("outside/research");
    std::fs::create_dir_all(&outside_dir).unwrap();

    let error = workspace_dir_for_local_removal(
        &json!({"workspace_dir": outside_dir.display().to_string()}),
        &working_dir,
    )
    .expect_err("outside path must not be locally deleted");

    assert!(error
        .to_string()
        .contains("Refusing to remove workspace directory outside Rua working directory"));
}

#[test]
fn remove_workspace_rejects_the_working_dir_itself() {
    let tmp = tempfile::tempdir().unwrap();
    let working_dir = tmp.path().join("rua");
    std::fs::create_dir_all(&working_dir).unwrap();

    let error = workspace_dir_for_local_removal(
        &json!({"workspace_dir": working_dir.display().to_string()}),
        &working_dir,
    )
    .expect_err("working dir itself must not be locally deleted");

    assert!(error
        .to_string()
        .contains("Refusing to remove workspace directory outside Rua working directory"));
}

#[test]
fn remove_workspace_rejects_traversal_that_escapes_working_dir_even_when_missing() {
    let tmp = tempfile::tempdir().unwrap();
    let working_dir = tmp.path().join("rua");
    std::fs::create_dir_all(&working_dir).unwrap();

    let escaped = working_dir.join("../outside/research");
    let error = workspace_dir_for_local_removal(
        &json!({"workspace_dir": escaped.display().to_string()}),
        &working_dir,
    )
    .expect_err("lexical traversal escape must be rejected before deletion");

    assert!(error
        .to_string()
        .contains("Refusing to remove workspace directory outside Rua working directory"));
}

#[test]
fn workspace_inside_check_rejects_equal_root_and_traversal_escape() {
    let tmp = tempfile::tempdir().unwrap();
    let working_dir = tmp.path().join("rua");

    assert!(is_workspace_dir_inside_working_dir(
        &working_dir.join("workspaces/research"),
        &working_dir
    ));
    assert!(!is_workspace_dir_inside_working_dir(
        &working_dir,
        &working_dir
    ));
    assert!(!is_workspace_dir_inside_working_dir(
        &working_dir.join("../outside/research"),
        &working_dir
    ));
}
