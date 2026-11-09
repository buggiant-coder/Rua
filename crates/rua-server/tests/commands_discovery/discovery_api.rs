use axum::body::Body;
use axum::extract::Request;
use axum::http::{Method, StatusCode};
use http_body_util::BodyExt;
use rua_server::endpoints::commands_discovery::{build_commands_metadata, CommandEntry};
use rua_server::{router, AppState, ServerProfile};
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;

fn metadata() -> Vec<CommandEntry> {
    build_commands_metadata()
}

fn build_app() -> (axum::Router, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    let state = AppState::from_tempdir(tmp.path().to_path_buf(), ServerProfile::Test);
    let app = router::build_app(state, None);
    (app, tmp)
}

async fn request(app: &axum::Router, method: Method, uri: &str) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("oneshot")
}

async fn read_json(resp: axum::response::Response) -> Value {
    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    serde_json::from_slice(&bytes).expect("json body")
}

fn find_command<'a>(commands: &'a [Value], name: &str, scope: &str) -> &'a Value {
    commands
        .iter()
        .find(|command| command["name"] == name && command["scope"] == scope)
        .unwrap_or_else(|| panic!("/{name} in scope {scope} missing from {commands:?}"))
}

#[test]
fn returns_non_empty_list() {
    let commands = metadata();
    assert!(
        !commands.is_empty(),
        "discovery must return at least one command"
    );
}

#[test]
fn scope_distribution_correct() {
    let commands = metadata();

    let daemon = commands.iter().filter(|c| c.scope == "daemon").count();
    let control = commands.iter().filter(|c| c.scope == "control").count();
    let conversation = commands
        .iter()
        .filter(|c| c.scope == "conversation")
        .count();

    assert_eq!(control, 2, "expected 2 control commands (stop, skills)");
    assert_eq!(conversation, 3, "expected 3 conversation commands");
    assert!(
        daemon >= 6,
        "expected at least 6 daemon commands, got {daemon}"
    );
}

#[test]
fn agent_required_correct() {
    let commands = metadata();

    let agent_required_names: Vec<&str> = commands
        .iter()
        .filter(|c| c.agent_required)
        .map(|c| c.name.as_str())
        .collect();

    assert!(
        agent_required_names.is_empty(),
        "no public slash commands should currently require the agent path: {agent_required_names:?}"
    );

    for name in &["clear", "compact", "new"] {
        let cmd = commands.iter().find(|c| c.name == *name);
        if let Some(c) = cmd {
            assert!(
                !c.agent_required,
                "/{name} should NOT have agent_required=true"
            );
        }
    }
}

#[test]
fn ui_replaced_conversation_commands_hidden_from_discovery() {
    let commands = metadata();

    for removed in [
        "compact_str",
        "dump_history",
        "history",
        "message",
        "no",
        "prompt",
        "prompts",
        "yes",
    ] {
        assert!(
            commands.iter().all(|command| command.name != removed),
            "/{removed} should be hidden from command discovery"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn http_commands_route_returns_stable_public_command_descriptors() {
    let (app, _tmp) = build_app();

    let resp = request(&app, Method::GET, "/api/commands").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    let commands = body.as_array().expect("commands array");
    assert!(
        !commands.is_empty(),
        "commands route must return at least one command"
    );

    for command in commands {
        assert!(
            command["name"]
                .as_str()
                .is_some_and(|value| !value.is_empty()),
            "command name must be non-empty: {command:?}"
        );
        assert!(
            command["scope"]
                .as_str()
                .is_some_and(|value| !value.is_empty()),
            "command scope must be non-empty: {command:?}"
        );
        assert!(
            command["help"]
                .as_str()
                .is_some_and(|value| !value.is_empty()),
            "command help must be non-empty: {command:?}"
        );
        assert!(
            command["agent_required"].as_bool().is_some(),
            "command agent_required must be boolean: {command:?}"
        );
    }

    let scopes: std::collections::BTreeSet<&str> = commands
        .iter()
        .map(|command| command["scope"].as_str().expect("scope"))
        .collect();
    assert!(scopes.contains("control"));
    assert!(scopes.contains("conversation"));
    assert!(scopes.contains("daemon"));
    assert!(
        !scopes.contains("channel-mode"),
        "ACP chat-mode slash commands are hidden from web autocomplete"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn http_commands_route_preserves_daemon_alias_and_agent_required_contracts() {
    let (app, _tmp) = build_app();

    let resp = request(&app, Method::GET, "/api/commands").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    let commands = body.as_array().expect("commands array");

    for (name, alias_of) in [
        ("status", "/daemon status"),
        ("approve", "/daemon approve"),
        ("deny", "/daemon deny"),
    ] {
        let command = find_command(commands, name, "daemon");
        assert_eq!(command["alias_of"], alias_of);
        assert_eq!(command["agent_required"], false);
    }

    for name in ["stop", "skills"] {
        assert!(
            commands.iter().any(|command| command["name"] == name),
            "priority registry command /{name} missing from route response"
        );
    }
    assert!(
        commands.iter().all(|command| command["name"] != "mission"),
        "the retired Mission kernel must not appear in command discovery"
    );

    for name in ["acp", "noacp", "acph", "acpr"] {
        assert!(
            commands.iter().all(|command| command["name"] != name),
            "hidden ACP chat-mode command /{name} leaked into route response"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retired_mission_http_surface_is_absent() {
    let (app, _tmp) = build_app();

    for uri in ["/api/missions", "/api/missions/legacy/state"] {
        let response = request(&app, Method::GET, uri).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{uri}");
    }
}

#[test]
fn daemon_aliases_have_alias_of() {
    let commands = metadata();

    let status_cmd = commands
        .iter()
        .find(|c| c.name == "status" && c.scope == "daemon");
    assert!(status_cmd.is_some(), "/status should exist in daemon scope");
    let status_cmd = status_cmd.unwrap();
    assert_eq!(
        status_cmd.alias_of.as_deref(),
        Some("/daemon status"),
        "/status alias_of should point to /daemon status"
    );
}

#[test]
fn acp_chat_mode_commands_hidden_from_discovery() {
    let commands = metadata();

    for expected in &["acp", "noacp", "acph", "acpr"] {
        assert!(
            commands.iter().all(|c| c.name != *expected),
            "/{expected} should be hidden from discovery"
        );
    }
}

#[test]
fn approval_commands_present() {
    let commands = metadata();

    for (name, alias_of) in [("approve", "/daemon approve"), ("deny", "/daemon deny")] {
        let cmd = commands
            .iter()
            .find(|c| c.name == name && c.scope == "daemon")
            .unwrap_or_else(|| panic!("/{name} should exist in daemon scope"));
        assert_eq!(
            cmd.alias_of.as_deref(),
            Some(alias_of),
            "/{name} alias_of should point to {alias_of}"
        );
    }
}

#[test]
fn discovery_superset_of_priority_registry() {
    let commands = metadata();
    let names: Vec<&str> = commands.iter().map(|c| c.name.as_str()).collect();

    for expected in &["stop", "skills"] {
        assert!(
            names.contains(expected),
            "priority registry command /{expected} missing from discovery"
        );
    }

    for expected in &[
        "status",
        "restart",
        "reload-config",
        "version",
        "logs",
        "approve",
        "daemon",
    ] {
        assert!(
            names.contains(expected),
            "daemon command /{expected} missing from discovery"
        );
    }
}
