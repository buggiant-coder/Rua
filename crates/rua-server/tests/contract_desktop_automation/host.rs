#![cfg(any(target_os = "macos", target_os = "windows"))]
use rua_server::desktop_automation::{DesktopAutomationRuntime, DesktopAutomationState};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

struct Proxy {
    child: tokio::process::Child,
    input: tokio::process::ChildStdin,
    output: BufReader<tokio::process::ChildStdout>,
    id: u64,
}
impl Proxy {
    async fn connect(binary: &std::path::Path, record: &Value) -> Self {
        let mut child = tokio::process::Command::new(binary)
            .args([
                "mcp",
                "--embedded",
                "--socket",
                record["socket_path"].as_str().unwrap(),
                "--host-bundle-id",
                record["host_bundle_id"].as_str().unwrap(),
            ])
            .env("CUA_DRIVER_RS_TELEMETRY_ENABLED", "0")
            .env("CUA_DRIVER_RS_UPDATE_CHECK", "false")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let output = BufReader::new(child.stdout.take().unwrap());
        let mut proxy = Self {
            child,
            input,
            output,
            id: 0,
        };
        let result=proxy.request("initialize",json!({"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"rua-host-test","version":"1"}})).await;
        assert_eq!(
            result["serverInfo"]["version"],
            rua_server::desktop_automation::DRIVER_VERSION
        );
        proxy
            .input
            .write_all(
                b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\",\"params\":{}}\n",
            )
            .await
            .unwrap();
        proxy
    }
    async fn request(&mut self, method: &str, params: Value) -> Value {
        self.id += 1;
        let message = json!({"jsonrpc":"2.0","id":self.id,"method":method,"params":params});
        self.input
            .write_all(format!("{message}\n").as_bytes())
            .await
            .unwrap();
        self.input.flush().await.unwrap();
        loop {
            let mut line = String::new();
            let n = tokio::time::timeout(
                std::time::Duration::from_secs(15),
                self.output.read_line(&mut line),
            )
            .await
            .unwrap()
            .unwrap();
            assert_ne!(n, 0, "MCP proxy closed");
            let reply: Value = serde_json::from_str(&line).unwrap();
            if reply["id"] == self.id {
                assert!(reply.get("error").is_none(), "{reply}");
                return reply["result"].clone();
            }
        }
    }
    async fn close(mut self) {
        drop(self.input);
        tokio::time::timeout(std::time::Duration::from_secs(5), self.child.wait())
            .await
            .unwrap()
            .unwrap();
    }
}

#[tokio::test]
#[ignore = "requires RUA_TEST_CUA_DRIVER; tests process ownership without granting OS permissions"]
async fn native_host_coalesces_start_and_survives_independent_mcp_shutdown() {
    let binary =
        PathBuf::from(std::env::var_os("RUA_TEST_CUA_DRIVER").expect("staged native driver"));
    let temp = tempfile::tempdir().unwrap();
    let owner = DesktopAutomationState::default();
    let directory = temp.path().join("host");
    let bundle = "test.rua.automation".to_string();
    let (one, two) = tokio::join!(
        owner.start(directory.clone(), binary.clone(), bundle.clone()),
        owner.start(directory.clone(), binary.clone(), bundle.clone())
    );
    let path = one.unwrap();
    assert_eq!(path, two.unwrap());
    let record: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert!(record["error"].is_null(), "{record}");
    let mut first = Proxy::connect(&binary, &record).await;
    let mut second = Proxy::connect(&binary, &record).await;
    let tools = first.request("tools/list", json!({})).await;
    for name in [
        "check_permissions",
        "get_window_state",
        "get_desktop_state",
        "click",
        "type_text",
        "press_key",
        "scroll",
    ] {
        assert!(
            tools["tools"]
                .as_array()
                .unwrap()
                .iter()
                .any(|tool| tool["name"] == name),
            "missing {name}"
        );
    }
    for (name, field) in [
        ("click", "x"),
        ("type_text", "text"),
        ("press_key", "key"),
        ("press_key", "modifiers"),
        ("press_key", "delivery_mode"),
        ("type_text", "delivery_mode"),
        ("click", "delivery_mode"),
        ("scroll", "direction"),
    ] {
        let schema = &tools["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == name)
            .unwrap()["inputSchema"];
        for property in ["pid", "window_id", "session", field] {
            assert!(
                schema["properties"][property].is_object(),
                "{name} must support Rua's exact window/session boundary: {schema}"
            );
        }
    }
    let permissions = first
        .request(
            "tools/call",
            json!({"name":"check_permissions","arguments":{}}),
        )
        .await;
    assert_ne!(permissions["isError"], true, "{permissions}");
    assert!(permissions["structuredContent"].is_object());
    #[cfg(target_os = "macos")]
    assert_eq!(
        permissions["structuredContent"]["source"]["attribution"],
        "host"
    );
    for (proxy, label) in [
        (&mut first, "rua-test-first"),
        (&mut second, "rua-test-second"),
    ] {
        let started = proxy
            .request(
                "tools/call",
                json!({"name":"start_session","arguments":{"session":label}}),
            )
            .await;
        assert_ne!(started["isError"], true, "{started}");
    }
    first.close().await;
    let result = second
        .request(
            "tools/call",
            json!({"name":"get_session_state","arguments":{"session":"rua-test-second"}}),
        )
        .await;
    assert_ne!(result["isError"], true, "{result}");
    second.close().await;
    let socket = record["socket_path"].as_str().unwrap().to_string();
    let metadata = tokio::task::spawn_blocking({
        let socket = socket.clone();
        move || cua_driver_core::daemon::request_daemon_metadata(&socket)
    })
    .await
    .unwrap()
    .unwrap();
    assert_eq!(u64::from(metadata.pid), record["pid"]);
    owner.stop().await.unwrap();
    owner.stop().await.unwrap();
    let stopped: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert!(stopped["error"].is_string());
    assert!(
        tokio::task::spawn_blocking(move || cua_driver_core::daemon::request_daemon_metadata(
            &socket
        ))
        .await
        .unwrap()
        .is_err(),
        "native host must be stopped by its owner"
    );
    let path = owner
        .start(directory, binary.clone(), bundle)
        .await
        .unwrap();
    let replacement: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_ne!(replacement["generation"], record["generation"]);
    let mut proxy = Proxy::connect(&binary, &replacement).await;
    assert!(proxy.request("tools/list", json!({})).await["tools"].is_array());
    proxy.close().await;
    owner.stop().await.unwrap();
}

#[test]
fn permission_settings_accept_only_two_fixed_system_destinations() {
    use rua_core::desktop_automation::PermissionSettings;
    for (value, suffix) in [
        ("accessibility", "Privacy_Accessibility"),
        ("screen_recording", "Privacy_ScreenCapture"),
    ] {
        let setting: PermissionSettings = serde_json::from_value(json!(value)).unwrap();
        assert!(rua_server::desktop_automation::permission_settings_url(setting).ends_with(suffix));
    }
    assert!(serde_json::from_value::<PermissionSettings>(json!("file:///tmp/anything")).is_err());
}

#[tokio::test]
async fn failed_native_start_publishes_diagnostic_without_stale_owner() {
    let temp = tempfile::tempdir().unwrap();
    let owner = DesktopAutomationState::default();
    let path = owner
        .start(
            temp.path().join("host"),
            temp.path().join("missing-driver"),
            "test.rua.automation".into(),
        )
        .await
        .unwrap();
    let record: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert!(record["error"].is_string());
    assert!(record["pid"].is_null());
    owner.stop().await.unwrap();
}

// Computer Use may be unavailable on macOS 11/12, but the Runtime must still load.
#[cfg(target_os = "macos")]
#[test]
fn runtime_host_does_not_link_capture_or_swift_runtime() {
    let output = std::process::Command::new("otool")
        .arg("-L")
        .arg(std::env::current_exe().unwrap())
        .output()
        .unwrap();
    assert!(output.status.success());
    let dependencies = String::from_utf8(output.stdout).unwrap();
    for framework in ["ScreenCaptureKit.framework", "libswift_Concurrency.dylib"] {
        assert!(
            !dependencies.contains(framework),
            "Runtime loads {framework} before the Computer Use OS check"
        );
    }
}

#[tokio::test]
async fn runtime_shutdown_prevents_restarting_its_driver() {
    let temp = tempfile::tempdir().unwrap();
    let settings = rua_mcp::bundled_computer::BundledComputer {
        driver: temp.path().join("missing-driver"),
        connection_file: temp.path().join("host/cua-connection.json"),
    };
    let runtime = std::sync::Arc::new(DesktopAutomationRuntime::new(Some(settings)));
    let cancel = tokio_util::sync::CancellationToken::new();
    let task = runtime.start(cancel.clone());
    cancel.cancel();
    tokio::time::timeout(std::time::Duration::from_secs(3), task)
        .await
        .unwrap()
        .unwrap();
    for restart in [false, true] {
        assert!(runtime
            .recheck(restart)
            .await
            .unwrap_err()
            .contains("shutting down"));
    }
    assert!(runtime
        .request_permissions()
        .await
        .unwrap_err()
        .contains("shutting down"));
    runtime.stop().await;
}

#[tokio::test]
#[cfg(unix)]
#[ignore = "requires staged native driver"]
async fn runtime_automatically_reaps_unknown_input_executor_before_new_observation() {
    use mcp_manager::{client::McpClient, types::McpCallContext};
    let binary =
        PathBuf::from(std::env::var_os("RUA_TEST_CUA_DRIVER").expect("staged native driver"));
    let temp = tempfile::tempdir().unwrap();
    let descriptor = temp.path().join("host/cua-connection.json");
    let runtime = std::sync::Arc::new(DesktopAutomationRuntime::new(Some(
        rua_mcp::bundled_computer::BundledComputer {
            driver: binary,
            connection_file: descriptor.clone(),
        },
    )));
    let cancel = tokio_util::sync::CancellationToken::new();
    let task = runtime.start(cancel.clone());
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        while !descriptor.exists() {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let old: Value = serde_json::from_slice(&std::fs::read(&descriptor).unwrap()).unwrap();
    assert!(old["pid"].is_u64(), "{old}");
    let script = temp.path().join("mock-driver");
    let python = std::process::Command::new("python3")
        .args(["-c", "import sys; print(sys.executable)"])
        .output()
        .unwrap();
    assert!(python.status.success());
    rua_config::atomic_write(
        &script,
        format!(
            "#!{}\n{}",
            String::from_utf8(python.stdout).unwrap().trim(),
            include_str!("../../../rua-mcp/tests/fixtures/mock_cua.py")
        )
        .as_bytes(),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    rua_config::atomic_write_json(
        &temp.path().join("permissions.json"),
        &json!({"source":{"attribution":"host"},"accessibility":true,"screen_recording":true}),
    )
    .unwrap();
    let client = rua_mcp::bundled_computer::BundledComputerClient::new(
        rua_mcp::bundled_computer::BundledComputer {
            driver: script,
            connection_file: descriptor.clone(),
        },
    );
    let ctx = McpCallContext {
        agent_id: "default".into(),
        session_id: "recovery-test".into(),
        metadata: [
            ("device_id".into(), json!("fixture-device")),
            (
                "security_chat_address".into(),
                serde_json::to_value(rua_core::ChatAddress::agent("default", "recovery-test"))
                    .unwrap(),
            ),
            ("scope".into(), json!("agent")),
            ("agent_id".into(), json!("default")),
            ("turn_id".into(), json!("turn")),
            ("tool_call_id".into(), json!("input")),
        ]
        .into(),
        ..Default::default()
    };
    let observation = client
        .call_tool(
            "get_window_state",
            json!({"pid":100,"window_id":200,"include_screenshot":false}),
            ctx.clone(),
        )
        .await
        .unwrap()
        .structured_content
        .unwrap();
    rua_config::atomic_write(&temp.path().join("drop-after-input"), b"1").unwrap();
    let input = json!({"pid":100,"window_id":200,"observation_id":observation["observation_id"],"include_screenshot":false,"actions":[{"tool":"click","arguments":{"x":1,"y":1}}]});
    let result = client
        .call_tool("interact", input.clone(), ctx.clone())
        .await
        .unwrap();
    assert_eq!(result.structured_content.unwrap()["outcome_unknown"], true);
    let new = tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            let current: Value =
                serde_json::from_slice(&std::fs::read(&descriptor).unwrap()).unwrap();
            if current["generation"].is_string() && current["generation"] != old["generation"] {
                break current;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_ne!(old["pid"], new["pid"]);
    assert_eq!(
        unsafe { libc::kill(old["pid"].as_u64().unwrap() as i32, 0) },
        -1
    );
    std::fs::remove_file(temp.path().join("drop-after-input")).unwrap();
    assert!(client
        .call_tool("interact", input.clone(), ctx.clone())
        .await
        .unwrap_err()
        .to_string()
        .contains("Observe"));
    let fresh = client
        .call_tool(
            "get_window_state",
            json!({"pid":100,"window_id":200,"include_screenshot":false}),
            ctx.clone(),
        )
        .await
        .unwrap();
    assert!(!fresh.is_error);
    let mut new_input = input;
    new_input["observation_id"] = fresh.structured_content.unwrap()["observation_id"].clone();
    assert!(
        !client
            .call_tool("interact", new_input, ctx)
            .await
            .unwrap()
            .is_error
    );
    let calls = std::fs::read_to_string(temp.path().join("calls.jsonl")).unwrap();
    assert_eq!(
        calls
            .lines()
            .filter(|line| serde_json::from_str::<Value>(line).unwrap()["name"] == "click")
            .count(),
        2
    );
    client.shutdown().await.unwrap();
    cancel.cancel();
    tokio::time::timeout(std::time::Duration::from_secs(10), task)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
#[cfg(unix)]
async fn runtime_startup_failures_back_off_and_shutdown_stops_retrying() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let driver = temp.path().join("failing-driver");
    let attempts = temp.path().join("attempts");
    rua_config::atomic_write(
        &driver,
        format!(
            "#!/bin/sh\necho attempt >> '{}'\necho 'invalid driver version'\nexit 1\n",
            attempts.display()
        )
        .as_bytes(),
    )
    .unwrap();
    std::fs::set_permissions(&driver, std::fs::Permissions::from_mode(0o700)).unwrap();
    let runtime = std::sync::Arc::new(DesktopAutomationRuntime::new(Some(
        rua_mcp::bundled_computer::BundledComputer {
            driver,
            connection_file: temp.path().join("host/cua-connection.json"),
        },
    )));
    let cancel = tokio_util::sync::CancellationToken::new();
    let task = runtime.start(cancel.clone());
    let started = tokio::time::Instant::now();
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let count = std::fs::read_to_string(&attempts)
                .unwrap_or_default()
                .lines()
                .count();
            if count >= 4 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert!(started.elapsed() >= std::time::Duration::from_secs(3));
    cancel.cancel();
    tokio::time::timeout(std::time::Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap();
    let final_attempts = std::fs::read_to_string(&attempts).unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(std::fs::read_to_string(&attempts).unwrap(), final_attempts);
    assert!(runtime
        .recheck(false)
        .await
        .unwrap_err()
        .contains("shutting down"));
}
