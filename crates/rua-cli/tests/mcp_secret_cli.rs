use clap::Parser;
use rua_cli::http_client::ApiClient;
use rua_cli::mcp::{
    build_secret_update_body, build_tool_call_body, mcp_config_path, parse_mcp_import,
};
use rua_cli::{Cli, Commands};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;

#[test]
fn parses_agent_and_shared_secret_commands() {
    let get_agent = Cli::try_parse_from(["rua", "mcp", "get", "demo"]).expect("agent get parses");
    assert!(matches!(get_agent.command, Commands::Mcp(_)));

    let get_shared =
        Cli::try_parse_from(["rua", "mcp", "get", "demo", "--shared"]).expect("shared get parses");
    assert!(matches!(get_shared.command, Commands::Mcp(_)));

    let set_secret = Cli::try_parse_from([
        "rua",
        "mcp",
        "set-secret",
        "demo",
        "transport.headers.Authorization",
        "--from-env",
        "MCP_TOKEN",
        "--shared",
    ])
    .expect("shared set-secret parses");
    assert!(matches!(set_secret.command, Commands::Mcp(_)));
}

#[test]
fn tool_call_body_uses_server_contract_field() {
    assert_eq!(
        build_tool_call_body("search", json!({"q": "rust"})),
        json!({
            "tool": "search",
            "arguments": {"q": "rust"}
        })
    );
}

#[test]
fn mcp_import_accepts_wrapped_and_direct_maps() {
    let wrapped = parse_mcp_import(&json!({
        "mcpServers": {
            "docs": {"command": "npx", "args": ["docs-mcp"]}
        }
    }))
    .expect("wrapped import");
    assert!(wrapped.contains_key("docs"));

    let direct = parse_mcp_import(&json!({
        "remote": {"url": "https://example.test/mcp"}
    }))
    .expect("direct import");
    assert!(direct.contains_key("remote"));
}

#[test]
fn secret_update_body_changes_one_path_and_preserves_masked_siblings() {
    let current = json!({
        "key": "demo",
        "name": "Demo",
        "transport": "streamable_http",
        "headers": {
            "Authorization": "Bear******1234",
            "x-api-key": "sk-******abcd",
            "User-Agent": "rua"
        },
        "env": {
            "API_KEY": "env-******1234"
        }
    });

    let body = build_secret_update_body(
        &current,
        "transport.headers.Authorization",
        Value::String("Bearer replacement".into()),
    )
    .expect("build update");

    assert_eq!(
        body,
        json!({
            "headers": {
                "Authorization": "Bearer replacement",
                "x-api-key": "sk-******abcd",
                "User-Agent": "rua"
            }
        })
    );
}

#[test]
fn secret_update_body_supports_raw_and_nested_secret_paths() {
    let current = json!({
        "key": "demo",
        "name": "Demo",
        "transport": "streamable_http",
        "extra_token": "old******oken",
        "nested": {
            "database_password": "db-******pass",
            "display_name": "Nested"
        }
    });

    let body = build_secret_update_body(
        &current,
        "extra_token",
        Value::String("new-extra-token".into()),
    )
    .expect("build top-level update");
    assert_eq!(body, json!({"extra_token": "new-extra-token"}));

    let body = build_secret_update_body(
        &current,
        "nested.database_password",
        Value::String("new-db-password".into()),
    )
    .expect("build nested update");
    assert_eq!(
        body,
        json!({
            "nested": {
                "database_password": "new-db-password",
                "display_name": "Nested"
            }
        })
    );

    assert!(build_secret_update_body(&current, "name", Value::String("Nope".into())).is_err());
    assert!(
        build_secret_update_body(&current, "headers.User-Agent", Value::String("Nope".into()))
            .is_err()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn api_backed_secret_update_round_trip_preserves_unrelated_fields() {
    let _tmp = tempfile::tempdir().expect("tempdir");
    let current = json!({
        "key": "demo",
        "name": "Demo",
        "enabled": true,
        "transport": "streamable_http",
        "url": "https://mcp.example.test",
        "headers": {
            "Authorization": "Bear******1234",
            "x-api-key": "sk-******abcd"
        },
        "env": {
            "API_KEY": "env-******1234"
        }
    });
    let observed_put = Arc::new(Mutex::new(None));
    let (base_url, handle) = spawn_two_request_mcp_server(current.clone(), observed_put.clone());

    let client = ApiClient::new(&base_url, "agent-a");
    let endpoint = mcp_config_path(true, "demo");
    let fetched = client.get(&endpoint).await.expect("GET shared MCP config");
    let update = build_secret_update_body(
        &fetched,
        "headers.Authorization",
        Value::String("Bearer replacement".into()),
    )
    .expect("build update");
    let updated = client
        .put(&endpoint, &update)
        .await
        .expect("PUT secret update");

    handle.join().expect("mock server thread");
    assert_eq!(
        observed_put.lock().expect("lock").as_ref(),
        Some(&json!({
            "headers": {
                "Authorization": "Bearer replacement",
                "x-api-key": "sk-******abcd"
            }
        }))
    );
    assert_eq!(updated["name"], current["name"]);
    assert_eq!(updated["env"], current["env"]);
    assert_eq!(updated["headers"]["Authorization"], "Bearer replacement");
    assert_eq!(
        updated["headers"]["x-api-key"],
        current["headers"]["x-api-key"]
    );
}

fn spawn_two_request_mcp_server(
    current: Value,
    observed_put: Arc<Mutex<Option<Value>>>,
) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock server");
    let addr = listener.local_addr().expect("local addr");
    let handle = thread::spawn(move || {
        let (first, _) = listener.accept().expect("accept GET");
        let get = read_http_request(first);
        assert!(get.starts_with("GET /api/shared-mcp/demo "));
        write_json_response(get.stream, &current);

        let (second, _) = listener.accept().expect("accept PUT");
        let put = read_http_request(second);
        assert!(put.starts_with("PUT /api/shared-mcp/demo "));
        let body: Value = serde_json::from_slice(&put.body).expect("PUT json");
        *observed_put.lock().expect("lock") = Some(body.clone());

        let mut merged = current.clone();
        if let (Some(target), Some(update)) = (merged.as_object_mut(), body.as_object()) {
            for (key, value) in update {
                target.insert(key.clone(), value.clone());
            }
        }
        write_json_response(put.stream, &merged);
    });
    (format!("http://{addr}"), handle)
}

struct HttpRequest {
    stream: TcpStream,
    head: String,
    body: Vec<u8>,
}

impl std::ops::Deref for HttpRequest {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.head
    }
}

fn read_http_request(mut stream: TcpStream) -> HttpRequest {
    let mut buf = Vec::new();
    let mut tmp = [0; 1024];
    loop {
        let n = stream.read(&mut tmp).expect("read request");
        assert_ne!(n, 0, "connection closed before headers");
        buf.extend_from_slice(&tmp[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
    }

    let header_end = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("headers end")
        + 4;
    let head = String::from_utf8(buf[..header_end].to_vec()).expect("utf8 headers");
    let content_length = head
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().expect("content length"))
        })
        .unwrap_or(0);

    let mut body = buf[header_end..].to_vec();
    while body.len() < content_length {
        let n = stream.read(&mut tmp).expect("read request body");
        assert_ne!(n, 0, "connection closed before body");
        body.extend_from_slice(&tmp[..n]);
    }
    body.truncate(content_length);

    HttpRequest { stream, head, body }
}

fn write_json_response(mut stream: TcpStream, value: &Value) {
    let body = serde_json::to_vec(value).expect("response json");
    write!(
        stream,
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        body.len()
    )
    .expect("write response head");
    stream.write_all(&body).expect("write response body");
}
