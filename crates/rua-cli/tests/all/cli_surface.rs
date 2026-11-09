use clap::{CommandFactory, Parser};
use reqwest::StatusCode;
use rua_cli::http_client::{
    format_http_error, require_server, resolve_base_url, resolve_base_url_from_all_sources,
    resolve_base_url_from_sources, resolve_server_port_from_sources,
    should_bypass_proxy_for_base_url, ApiClient,
};
use rua_cli::{Cli, Commands};

fn command_names() -> Vec<String> {
    Cli::command()
        .get_subcommands()
        .map(|cmd| cmd.get_name().to_string())
        .collect()
}

#[test]
fn qwenpaw_referenced_cli_surfaces_have_rust_canonical_commands() {
    let names = command_names();

    for expected in ["agent", "doctor", "shutdown", "update", "desktop", "server"] {
        assert!(
            names.iter().any(|name| name == expected),
            "CLI command {expected:?} should remain registered"
        );
    }

    let cli = Cli::try_parse_from(["rua", "app"]).expect("server alias app should parse");
    assert!(matches!(cli.command, Commands::Server(_)));
}

#[test]
fn task_is_not_a_rust_cli_surface() {
    let names = command_names();

    assert!(
        !names.iter().any(|name| name == "task"),
        "QwenPaw's headless `task` command is not currently a Rust CLI command"
    );
}

#[test]
fn mission_kernel_is_not_a_rust_cli_surface() {
    let names = command_names();

    assert!(
        !names.iter().any(|name| name == "mission"),
        "the retired Mission kernel must not remain a CLI command"
    );
}

#[test]
fn version_flag_uses_cargo_package_version() {
    let err = Cli::try_parse_from(["rua", "--version"]).unwrap_err();
    let rendered = err.to_string();

    assert!(rendered.contains("rua"));
    assert!(rendered.contains(rua_cli::VERSION));
}

#[test]
fn update_yes_flag_parses_without_prompting_contract() {
    let cli = Cli::try_parse_from(["rua", "update", "--yes"]).expect("update --yes parses");

    assert!(matches!(cli.command, Commands::Update(_)));
}

#[test]
fn server_start_owns_tracing_initialization() {
    for args in [
        vec!["rua", "server"],
        vec!["rua", "server", "start"],
        vec!["rua", "app", "start"],
    ] {
        let cli = Cli::try_parse_from(args).expect("server start command should parse");
        assert!(cli.command.starts_server());
    }

    for args in [
        vec!["rua", "server", "stop"],
        vec!["rua", "server", "status"],
        vec!["rua", "doctor"],
    ] {
        let cli = Cli::try_parse_from(args).expect("non-start command should parse");
        assert!(!cli.command.starts_server());
    }
}

#[test]
fn server_start_accepts_hidden_desktop_endpoint_contract() {
    let cli = Cli::try_parse_from([
        "rua",
        "server",
        "start",
        "--port",
        "0",
        "--desktop-endpoint-file",
        "/tmp/rua-desktop-endpoint.json",
    ])
    .expect("desktop endpoint arguments should parse");

    assert!(cli.command.starts_server());
}

#[test]
fn removed_process_root_flags_are_rejected() {
    for removed in ["--workspace", "--working-dir"] {
        let error = Cli::try_parse_from(["rua", removed, "/tmp/rua", "doctor"])
            .expect_err("removed process-root flag must not parse");
        assert!(error.to_string().contains("unexpected argument"));
    }
}

#[test]
fn local_api_clients_bypass_proxy_policy_for_loopback_targets() {
    for url in [
        "http://localhost:8765",
        "http://localhost.:8765",
        "http://127.0.0.1:8765",
        "http://127.1.2.3:8765",
        "http://[::1]:8765",
    ] {
        assert!(
            should_bypass_proxy_for_base_url(url),
            "loopback URL should bypass proxy: {url}"
        );
    }
}

#[test]
fn remote_api_clients_keep_default_proxy_policy() {
    for url in [
        "https://rua.example.test",
        "http://192.168.1.8:8765",
        "http://10.0.0.1:8765",
        "http://0.0.0.0:8765",
        "http://[::]:8765",
        "not a url",
    ] {
        assert!(
            !should_bypass_proxy_for_base_url(url),
            "non-loopback URL should not force proxy bypass: {url}"
        );
    }
}

#[test]
fn api_client_normalizes_base_url_without_losing_path() {
    let client = ApiClient::new("http://127.0.0.1:8765/", "agent-a");

    assert_eq!(
        client.streaming_url("/api/agents/agent-a/chats/chat-a/console/chat"),
        "http://127.0.0.1:8765/api/agents/agent-a/chats/chat-a/console/chat"
    );
}

#[test]
fn default_base_url_uses_rua_host_and_port_env_contract() {
    let old_host = std::env::var_os("RUA_HOST");
    let old_port = std::env::var_os("RUA_PORT");

    unsafe {
        std::env::set_var("RUA_HOST", "0.0.0.0");
        std::env::set_var("RUA_PORT", "9876");
    }
    let resolved = resolve_base_url(None);
    unsafe {
        match old_host {
            Some(value) => std::env::set_var("RUA_HOST", value),
            None => std::env::remove_var("RUA_HOST"),
        }
        match old_port {
            Some(value) => std::env::set_var("RUA_PORT", value),
            None => std::env::remove_var("RUA_PORT"),
        }
    }

    assert_eq!(resolved, "http://0.0.0.0:9876");
}

#[test]
fn installed_endpoint_config_supplies_cli_defaults() {
    let temp = tempfile::tempdir().unwrap();
    let config = temp.path().join("cli-endpoint.env");
    std::fs::write(&config, "RUA_HOST=127.0.0.1\nRUA_PORT=2026\n").unwrap();

    assert_eq!(
        resolve_base_url_from_sources(None, None, None, Some(&config)),
        "http://127.0.0.1:2026"
    );
    assert_eq!(
        resolve_server_port_from_sources(None, None, Some(&config)),
        2026
    );
}

#[test]
fn ready_desktop_endpoint_supplies_dynamic_cli_defaults() {
    let temp = tempfile::tempdir().unwrap();
    let endpoint_path = temp.path().join("desktop-endpoint.json");
    let (port, server) = spawn_ready_endpoint_server(true);
    std::fs::write(
        &endpoint_path,
        serde_json::json!({
            "version": 1,
            "host": "127.0.0.1",
            "port": port,
            "pid": std::process::id(),
            "rua_version": "0.1.0"
        })
        .to_string(),
    )
    .unwrap();

    assert_eq!(
        resolve_base_url_from_all_sources(None, None, None, Some(&endpoint_path), None),
        format!("http://127.0.0.1:{port}")
    );
    server.join().unwrap();
}

#[test]
fn standalone_server_port_uses_only_server_owned_sources() {
    let temp = tempfile::tempdir().unwrap();
    let config = temp.path().join("cli-endpoint.env");
    std::fs::write(&config, "RUA_PORT=2026\n").unwrap();

    assert_eq!(
        resolve_server_port_from_sources(None, None, Some(&config)),
        2026
    );
    assert_eq!(
        resolve_server_port_from_sources(None, None, None),
        8765,
        "Desktop endpoint discovery belongs to API clients, not server ownership commands"
    );
}

#[test]
fn stale_or_unready_desktop_endpoint_is_ignored() {
    let temp = tempfile::tempdir().unwrap();
    let endpoint_path = temp.path().join("desktop-endpoint.json");
    let installed = temp.path().join("cli-endpoint.env");
    std::fs::write(&installed, "RUA_HOST=127.0.0.1\nRUA_PORT=2026\n").unwrap();
    std::fs::write(
        &endpoint_path,
        serde_json::json!({
            "version": 1,
            "host": "127.0.0.1",
            "port": 49999,
            "pid": u32::MAX,
            "rua_version": "0.1.0"
        })
        .to_string(),
    )
    .unwrap();

    assert_eq!(
        resolve_base_url_from_all_sources(None, None, None, Some(&endpoint_path), Some(&installed)),
        "http://127.0.0.1:2026"
    );

    let (port, server) = spawn_ready_endpoint_server(false);
    std::fs::write(
        &endpoint_path,
        serde_json::json!({
            "version": 1,
            "host": "127.0.0.1",
            "port": port,
            "pid": std::process::id(),
            "rua_version": "0.1.0"
        })
        .to_string(),
    )
    .unwrap();
    assert_eq!(
        resolve_base_url_from_all_sources(None, None, None, Some(&endpoint_path), Some(&installed)),
        "http://127.0.0.1:2026"
    );
    server.join().unwrap();
}

#[test]
fn explicit_endpoint_sources_override_ready_desktop_state() {
    let temp = tempfile::tempdir().unwrap();
    let endpoint_path = temp.path().join("desktop-endpoint.json");
    std::fs::write(
        &endpoint_path,
        serde_json::json!({
            "version": 1,
            "host": "127.0.0.1",
            "port": 49999,
            "pid": std::process::id(),
            "rua_version": "0.1.0"
        })
        .to_string(),
    )
    .unwrap();

    assert_eq!(
        resolve_base_url_from_all_sources(
            Some("https://explicit.example"),
            None,
            None,
            Some(&endpoint_path),
            None
        ),
        "https://explicit.example"
    );
    assert_eq!(
        resolve_base_url_from_all_sources(
            None,
            Some("localhost"),
            Some("3030"),
            Some(&endpoint_path),
            None
        ),
        "http://localhost:3030"
    );
}

fn spawn_ready_endpoint_server(ready: bool) -> (u16, std::thread::JoinHandle<()>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        use std::io::{BufRead, BufReader, Write};
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                break;
            }
        }
        let status = if ready {
            "200 OK"
        } else {
            "503 Service Unavailable"
        };
        let body = serde_json::json!({ "ready": ready }).to_string();
        write!(
            stream,
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
    });
    (port, handle)
}

#[tokio::test]
async fn server_preflight_returns_actionable_start_command() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let error = require_server(&format!("http://127.0.0.1:{port}"))
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("Rua server is not running"));
    assert!(error.contains("rua daemon start"));
}

#[test]
fn explicit_cli_endpoint_sources_override_installed_config() {
    let temp = tempfile::tempdir().unwrap();
    let config = temp.path().join("cli-endpoint.env");
    std::fs::write(&config, "RUA_HOST=127.0.0.1\nRUA_PORT=2026\n").unwrap();

    assert_eq!(
        resolve_base_url_from_sources(None, Some("localhost"), Some("3030"), Some(&config)),
        "http://localhost:3030"
    );
    assert_eq!(
        resolve_base_url_from_sources(Some("https://rua.example.test"), None, None, Some(&config)),
        "https://rua.example.test"
    );
    assert_eq!(
        resolve_server_port_from_sources(Some(4040), Some("3030"), Some(&config)),
        4040
    );
}

#[test]
fn http_error_formatting_preserves_fastapi_validation_detail_arrays() {
    let error = format_http_error(
        "PUT",
        "http://127.0.0.1:8765/api/settings/language",
        StatusCode::UNPROCESSABLE_ENTITY,
        r#"{"detail":[{"loc":["body","language"],"msg":"Field required","type":"missing"}]}"#,
    );

    assert_eq!(
        error,
        r#"PUT http://127.0.0.1:8765/api/settings/language returned 422 Unprocessable Entity: [{"loc":["body","language"],"msg":"Field required","type":"missing"}]"#
    );
}

#[test]
fn http_error_formatting_uses_error_field_when_detail_is_absent() {
    let error = format_http_error(
        "POST",
        "http://127.0.0.1:8765/api/agents",
        StatusCode::BAD_REQUEST,
        r#"{"error":{"code":"invalid_agent","message":"bad id"}}"#,
    );

    assert_eq!(
        error,
        r#"POST http://127.0.0.1:8765/api/agents returned 400 Bad Request: {"code":"invalid_agent","message":"bad id"}"#
    );
}
