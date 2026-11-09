use rua_cli::process_utils::is_rua_process;
use rua_cli::shutdown::{
    choose_shutdown_action, is_no_running_service_error, shutdown_endpoint, shutdown_request_body,
    ShutdownAction,
};
use serde_json::json;

#[test]
fn shutdown_prefers_local_workspace_process_over_http_fallback() {
    assert_eq!(
        choose_shutdown_action(Some(42)),
        ShutdownAction::KillProcess(42)
    );
    assert_eq!(choose_shutdown_action(None), ShutdownAction::PostShutdown);
}

#[test]
fn shutdown_http_fallback_uses_rust_shutdown_endpoint_and_empty_body() {
    assert_eq!(shutdown_endpoint(), "/api/shutdown");
    assert_eq!(shutdown_request_body(), json!({}));
}

#[test]
fn shutdown_treats_connection_failures_as_no_running_service() {
    for message in [
        "POST http://127.0.0.1:8765/api/shutdown returned error: Connection refused",
        "error sending request for url: connect error",
        "client error: tcp connect error: Connection refused",
    ] {
        assert!(
            is_no_running_service_error(message),
            "{message:?} should be treated as no local service"
        );
    }
}

#[test]
fn shutdown_propagates_non_connection_errors() {
    for message in [
        "POST http://127.0.0.1:8765/api/shutdown returned 500: boom",
        "parse POST http://127.0.0.1:8765/api/shutdown",
        "permission denied",
    ] {
        assert!(
            !is_no_running_service_error(message),
            "{message:?} should remain a real shutdown error"
        );
    }
}

#[test]
fn shutdown_process_match_is_workspace_scoped() {
    let matching_cmd: Vec<String> = vec!["rua", "server", "--rua-root", "/tmp/rua-a"]
        .into_iter()
        .map(String::from)
        .collect();
    let other_workspace_cmd: Vec<String> = vec!["rua", "server", "--rua-root", "/tmp/rua-b"]
        .into_iter()
        .map(String::from)
        .collect();
    let no_workspace_cmd: Vec<String> = vec!["rua", "server"]
        .into_iter()
        .map(String::from)
        .collect();

    assert!(is_rua_process("rua", &matching_cmd, "/tmp/rua-a"));
    assert!(!is_rua_process("rua", &other_workspace_cmd, "/tmp/rua-a"));
    assert!(!is_rua_process("rua", &no_workspace_cmd, "/tmp/rua-a"));
}
