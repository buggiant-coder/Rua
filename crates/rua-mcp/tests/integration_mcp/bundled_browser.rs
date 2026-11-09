use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use mcp_manager::client::McpClient;
use mcp_manager::manager::McpManager;
use mcp_manager::types::{McpCallContext, McpConfig, McpServerId, ToolFilter};
use rua_contract::Tool;
use rua_core::runtime_authority::{RuntimeCapability, RuntimeCapabilitySource};
use rua_core::tool::ToolContext;
use rua_core::ChatAddress;
use rua_mcp::bundled_browser::{
    session_key, BundledBrowser, BundledBrowserClient, DRIVER_VERSION, SERVER_ID,
};
use rua_mcp::{McpManagerImpl, McpMobilePolicy, McpToolBridge, PlatformProfile};
use serde_json::{json, Value};

fn context(agent: &str, address: ChatAddress) -> McpCallContext {
    McpCallContext {
        agent_id: agent.into(),
        session_id: "same-short-id".into(),
        metadata: BTreeMap::from([
            ("device_id".into(), json!("execution-device-a")),
            (
                "security_chat_address".into(),
                serde_json::to_value(address).unwrap(),
            ),
        ]),
        ..Default::default()
    }
}

fn settings(temp: &tempfile::TempDir) -> BundledBrowser {
    BundledBrowser {
        driver: temp.path().join("missing-driver"),
        data_dir: temp.path().join("data"),
        preferences_path: None,
        browser: RuntimeCapability {
            name: "chromium".into(),
            available: false,
            path: None,
            version: None,
            source: RuntimeCapabilitySource::Unavailable,
            diagnostics: vec!["Install a Chromium browser".into()],
        },
    }
}

#[test]
fn complete_chat_address_and_execution_device_determine_identity() {
    let a = context("a", ChatAddress::agent("a", "same-short-id"));
    let b = context("a", ChatAddress::project("project-b", "same-short-id"));
    let other_agent = context("b", ChatAddress::agent("b", "same-short-id"));
    assert_ne!(session_key(&a).unwrap(), session_key(&b).unwrap());
    assert_ne!(session_key(&a).unwrap(), session_key(&other_agent).unwrap());
    let mut next_turn = a.clone();
    next_turn.session_id = "different-transport-session".into();
    assert_eq!(session_key(&a).unwrap(), session_key(&next_turn).unwrap());
    next_turn
        .metadata
        .insert("device_id".into(), json!("device-b"));
    assert_ne!(session_key(&a).unwrap(), session_key(&next_turn).unwrap());
    next_turn.metadata.remove("security_chat_address");
    assert!(session_key(&next_turn).is_err());
    assert!(session_key(&context("b", ChatAddress::agent("a", "same-short-id"))).is_err());
}

#[tokio::test]
async fn model_cannot_supply_driver_flags_or_session_identity() {
    let temp = tempfile::tempdir().unwrap();
    let client = BundledBrowserClient::new(settings(&temp));
    let ctx = context("a", ChatAddress::agent("a", "chat"));
    for args in [
        json!({"session":"victim"}),
        json!({"extraArgs":["--cdp","9222"]}),
        json!({"namespace":"victim"}),
        json!({"url":"--profile"}),
        json!({"url":"file:///etc/passwd"}),
        json!({"url":"javascript:alert(1)"}),
        json!({"headed":true}),
    ] {
        let error = client
            .call_tool("agent_browser_open", args, ctx.clone())
            .await
            .unwrap_err();
        assert!(
            matches!(
                error,
                mcp_manager::error::McpError::SchemaValidationFailed { .. }
            ),
            "{error}"
        );
    }
    assert!(client
        .call_tool("agent_browser_eval", json!({"script":"1"}), ctx.clone())
        .await
        .is_err());
    assert!(client
        .call_tool("agent_browser_close", json!({"all":true}), ctx)
        .await
        .is_err());
    assert!(
        !temp.path().join("data").exists(),
        "rejected calls must not start a driver"
    );
}

#[tokio::test]
async fn missing_driver_degrades_only_the_capability_and_preserves_disablements() {
    let temp = tempfile::tempdir().unwrap();
    let browser = settings(&temp);
    let manager = McpManagerImpl::new(McpMobilePolicy::new(PlatformProfile::Desktop))
        .with_bundled_browser(browser.clone());
    let (a, b) = tokio::join!(
        manager.load_config(McpConfig::default()),
        manager.load_config(McpConfig::default())
    );
    a.unwrap();
    b.unwrap();
    assert_eq!(manager.list_servers().await.unwrap().len(), 1);
    assert!(manager
        .list_tools(ToolFilter::default())
        .await
        .unwrap()
        .is_empty());
    let status = manager.list_servers().await.unwrap().remove(0);
    assert!(status.last_error.unwrap().contains("missing"));
    let mut disabled = browser.defaults(McpConfig::default());
    let entry = disabled
        .clients
        .get_mut(&McpServerId::new(SERVER_ID))
        .unwrap();
    entry.enabled = false;
    entry.disabled_tools.push("agent_browser_open".into());
    manager.reload(disabled).await.unwrap();
    assert!(!manager.list_servers().await.unwrap()[0].enabled);
    manager.shutdown_all().await.unwrap();
    assert!(manager.list_servers().await.unwrap().is_empty());
}

/// Explicitly ignored in hermetic CI. Running this test requires the staged native
/// artifact and a real system Chromium; missing prerequisites are failures.
#[tokio::test]
#[ignore = "requires RUA_TEST_AGENT_BROWSER and RUA_TEST_CHROMIUM"]
async fn real_system_chromium_through_chat_tool_bridge() {
    let temp = tempfile::tempdir().unwrap();
    let driver = PathBuf::from(
        std::env::var_os("RUA_TEST_AGENT_BROWSER").expect("stage the pinned native driver"),
    );
    let chromium = PathBuf::from(
        std::env::var_os("RUA_TEST_CHROMIUM").expect("provide an installed Chromium executable"),
    );
    let fixture = axum::Router::new().route(
        "/",
        axum::routing::get(|| async {
            axum::response::Html(include_str!("../fixtures/browser-form.html"))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let cancel = tokio_util::sync::CancellationToken::new();
    let stop = cancel.clone();
    let server = tokio::spawn(async move {
        axum::serve(listener, fixture)
            .with_graceful_shutdown(stop.cancelled_owned())
            .await
            .unwrap();
    });
    let manager = Arc::new(
        McpManagerImpl::new(McpMobilePolicy::new(PlatformProfile::Desktop)).with_bundled_browser(
            BundledBrowser {
                driver,
                data_dir: temp.path().join("本机 profile"),
                preferences_path: None,
                browser: RuntimeCapability {
                    name: "chromium".into(),
                    available: true,
                    path: Some(chromium),
                    version: None,
                    source: RuntimeCapabilitySource::Configured,
                    diagnostics: vec![],
                },
            },
        ),
    );
    manager.load_config(McpConfig::default()).await.unwrap();
    let bridge = McpToolBridge::new(manager.clone());
    let (a, b) = tokio::join!(bridge.discover_tools(), bridge.discover_tools());
    let tools = a.unwrap();
    b.unwrap();
    assert_eq!(tools.len(), 11);
    for tool in &tools {
        assert!(tool.schema().parameters["properties"]
            .get("session")
            .is_none());
        assert!(tool.schema().parameters["properties"]
            .get("extraArgs")
            .is_none());
    }
    let mut tool_ctx = ToolContext {
        agent_id: "agent-a".into(),
        session_id: "short-chat".into(),
        ..Default::default()
    };
    tool_ctx.metadata = context("agent-a", ChatAddress::agent("agent-a", "short-chat"))
        .metadata
        .into_iter()
        .collect();
    let open = tools
        .iter()
        .find(|t| t.original_name() == "agent_browser_open")
        .unwrap();
    let result = open
        .call(json!({"url":url}), tool_ctx.clone())
        .await
        .unwrap();
    assert!(!result.is_error, "{result:?}");
    let title = tools
        .iter()
        .find(|t| t.original_name() == "agent_browser_get_title")
        .unwrap();
    let title = title.call(json!({}), tool_ctx.clone()).await.unwrap();
    assert!(format!("{title:?}").contains("Rua native browser fixture"));
    let text = tools
        .iter()
        .find(|t| t.original_name() == "agent_browser_get_text")
        .unwrap();
    let read = text
        .call(json!({"selector":"body"}), tool_ctx.clone())
        .await
        .unwrap();
    assert!(format!("{read:?}").contains("fixture-proof-731"));
    let fill = tools
        .iter()
        .find(|t| t.original_name() == "agent_browser_fill")
        .unwrap();
    assert!(
        !fill
            .call(
                json!({"selector":"#message","text":"批准一次"}),
                tool_ctx.clone()
            )
            .await
            .unwrap()
            .is_error
    );
    let security = Arc::new(rua_security::SecurityModule::new(
        Arc::new(rua_security::SecurityConfigStore::new(
            temp.path().to_path_buf(),
        )),
        None,
    ));
    let guarded = McpToolBridge::new(manager.clone()).with_security(security.clone());
    let guarded_tools = guarded.discover_tools().await.unwrap();
    let click = guarded_tools
        .iter()
        .find(|t| t.original_name() == "agent_browser_click")
        .unwrap();
    let mut config = rua_config::types::security::SecurityConfig::default();
    config.tool_guard.approval_mode = "ask_first".into();
    let mut approval_ctx = tool_ctx.clone();
    approval_ctx.metadata.insert(
        rua_security::models::EFFECTIVE_SECURITY_CONFIG_METADATA_KEY.into(),
        serde_json::to_value(config).unwrap(),
    );
    approval_ctx
        .metadata
        .insert("turn_id".into(), json!("browser-turn"));
    approval_ctx
        .metadata
        .insert("tool_call_id".into(), json!("submit"));
    let args = json!({"selector":"#submit"});
    assert!(click
        .call(args.clone(), approval_ctx.clone())
        .await
        .is_err());
    assert!(
        text.call(json!({"selector":"#result"}), tool_ctx.clone())
            .await
            .unwrap()
            .text_content()
            .contains("submissions=0"),
        "approval must precede every page side effect"
    );
    let address = ChatAddress::agent("agent-a", "short-chat");
    let pending = security
        .approval_authority()
        .snapshot_at(&address)
        .await
        .pending;
    assert_eq!(pending.len(), 1);
    security
        .approval_authority()
        .decide(
            &pending[0].id,
            rua_security::ApprovalDecision::RunOnce {
                decided_by: "user".into(),
            },
        )
        .await
        .unwrap();
    assert!(!click.call(args, approval_ctx).await.unwrap().is_error);
    assert!(text
        .call(json!({"selector":"#result"}), tool_ctx.clone())
        .await
        .unwrap()
        .text_content()
        .contains("submissions=1 value=批准一次"));
    let unchecked_click = tools
        .iter()
        .find(|t| t.original_name() == "agent_browser_click")
        .unwrap();
    let stale = unchecked_click
        .call(json!({"selector":"@e999999"}), tool_ctx.clone())
        .await
        .unwrap();
    assert!(
        stale.is_error,
        "stale reference must fail without choosing another target"
    );
    let (call_cancel, rx) = tokio::sync::watch::channel(true);
    let mut cancelled = tool_ctx.clone();
    cancelled.cancel = Some(rx);
    assert!(unchecked_click
        .call(json!({"selector":"#submit"}), cancelled)
        .await
        .is_err());
    drop(call_cancel);
    assert!(text
        .call(json!({"selector":"#result"}), tool_ctx.clone())
        .await
        .unwrap()
        .text_content()
        .contains("submissions=1"));
    let screenshot = tools
        .iter()
        .find(|t| t.original_name() == "agent_browser_screenshot")
        .unwrap();
    let no_vision = screenshot
        .call(json!({"selector":"#visual"}), tool_ctx.clone())
        .await
        .unwrap_err();
    assert!(no_vision.to_string().contains("image input"));
    tool_ctx.metadata.insert(
        rua_core::tool::MODEL_SUPPORTS_IMAGE_METADATA_KEY.into(),
        json!(true),
    );
    let result = screenshot
        .call(json!({"selector":"#visual"}), tool_ctx.clone())
        .await
        .unwrap();
    assert!(!result.is_error, "{result:?}");
    assert!(result.content.iter().any(|part| matches!(part, rua_core::ContentPart::Image { url, media_type: Some(kind), .. } if kind == "image/png" && url.starts_with("data:image/png;base64,iVBOR"))));
    let close = tools
        .iter()
        .find(|t| t.original_name() == "agent_browser_close")
        .unwrap();
    assert!(!close.call(json!({}), tool_ctx).await.unwrap().is_error);
    let report = manager.shutdown_all().await.unwrap();
    assert!(report.errors.is_empty(), "{report:?}");
    cancel.cancel();
    server.await.unwrap();
}

#[test]
fn builtin_registration_does_not_overwrite_a_third_party_name_collision() {
    let temp = tempfile::tempdir().unwrap();
    let browser = settings(&temp);
    let mut config = browser.defaults(McpConfig::default());
    let third_party = config
        .clients
        .get_mut(&McpServerId::new(SERVER_ID))
        .unwrap();
    third_party.extra.clear();
    third_party.transport = mcp_manager::types::McpTransportConfig::StreamableHttp {
        url: "https://example.org/mcp".into(),
        headers: BTreeMap::new(),
        timeout_secs: 30,
    };
    assert_eq!(browser.defaults(config.clone()), config);
}

#[tokio::test]
async fn actions_reject_hidden_controls_and_nonvisual_screenshot_before_launch() {
    let temp = tempfile::tempdir().unwrap();
    let client = BundledBrowserClient::new(settings(&temp));
    let ctx = context("a", ChatAddress::agent("a", "chat"));
    for (name, args) in [
        (
            "agent_browser_click",
            json!({"selector":"#send","newTab":true}),
        ),
        (
            "agent_browser_fill",
            json!({"selector":"#input","text":"--session=victim"}),
        ),
        ("agent_browser_scroll", json!({"direction":"invalid"})),
        ("agent_browser_scroll", json!({"amount":-1})),
        (
            "agent_browser_screenshot",
            json!({"path":"/tmp/victim.png"}),
        ),
        ("agent_browser_screenshot", json!({})),
    ] {
        assert!(client.call_tool(name, args, ctx.clone()).await.is_err());
    }
    assert!(!temp.path().join("data").exists());
}

#[tokio::test]
#[ignore = "requires staged native driver and real system Chromium"]
async fn real_chat_profiles_survive_restart_and_other_agent_shutdown() {
    let temp = tempfile::tempdir().unwrap();
    let mut options = settings(&temp);
    options.driver = std::env::var_os("RUA_TEST_AGENT_BROWSER")
        .expect("native driver")
        .into();
    options.browser = RuntimeCapability {
        name: "chromium".into(),
        available: true,
        path: Some(
            std::env::var_os("RUA_TEST_CHROMIUM")
                .expect("system Chromium")
                .into(),
        ),
        version: None,
        source: RuntimeCapabilitySource::Configured,
        diagnostics: vec![],
    };
    let fixture = axum::Router::new().route(
        "/",
        axum::routing::get(|| async {
            axum::response::Html(include_str!("../fixtures/browser-form.html"))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let cancel = tokio_util::sync::CancellationToken::new();
    let stop = cancel.clone();
    let server = tokio::spawn(async move {
        axum::serve(listener, fixture)
            .with_graceful_shutdown(stop.cancelled_owned())
            .await
            .unwrap();
    });
    let first = BundledBrowserClient::new(options.clone());
    let other = BundledBrowserClient::new(options.clone());
    let (a, b) = tokio::join!(first.connect(), other.connect());
    a.unwrap();
    b.unwrap();
    let a = context("a", ChatAddress::agent("a", "same-id"));
    let b = context("a", ChatAddress::project("project", "same-id"));
    let c = context("b", ChatAddress::agent("b", "same-id"));
    let (one, two, three) = tokio::join!(
        first.call_tool("agent_browser_open", json!({"url":url}), a.clone()),
        first.call_tool("agent_browser_open", json!({"url":url}), b.clone()),
        other.call_tool("agent_browser_open", json!({"url":url}), c.clone()),
    );
    for result in [one, two, three] {
        let result = result.unwrap();
        assert!(!result.is_error, "{result:?}");
    }
    for (client, ctx, value) in [
        (&first, &a, "first-chat"),
        (&first, &b, "project-chat"),
        (&other, &c, "other-agent"),
    ] {
        assert!(
            !client
                .call_tool(
                    "agent_browser_fill",
                    json!({"selector":"#message","text":value}),
                    ctx.clone()
                )
                .await
                .unwrap()
                .is_error
        );
        assert!(
            !client
                .call_tool(
                    "agent_browser_click",
                    json!({"selector":"#submit"}),
                    ctx.clone()
                )
                .await
                .unwrap()
                .is_error
        );
    }
    let mut next_turn = a.clone();
    next_turn.session_id = "next-turn-transport".into();
    let result = first
        .call_tool(
            "agent_browser_get_text",
            json!({"selector":"#result"}),
            next_turn,
        )
        .await
        .unwrap();
    assert!(result.text().contains("submissions=1 value=first-chat"));
    let result = first
        .call_tool(
            "agent_browser_get_text",
            json!({"selector":"#result"}),
            b.clone(),
        )
        .await
        .unwrap();
    assert!(result.text().contains("submissions=1 value=project-chat"));
    let closed = first
        .call_tool("agent_browser_close", json!({}), a.clone())
        .await
        .unwrap();
    assert!(!closed.is_error, "{closed:?}");
    let sibling = first
        .call_tool(
            "agent_browser_get_text",
            json!({"selector":"#result"}),
            b.clone(),
        )
        .await
        .unwrap();
    assert!(sibling.text().contains("submissions=1 value=project-chat"));
    let reopened = first
        .call_tool("agent_browser_open", json!({"url":url}), a.clone())
        .await
        .unwrap();
    assert!(!reopened.is_error, "{reopened:?}");
    let saved = first
        .call_tool(
            "agent_browser_get_text",
            json!({"selector":"#saved"}),
            a.clone(),
        )
        .await
        .unwrap();
    assert!(saved.text().contains("first-chat"), "{saved:?}");
    first.shutdown().await.unwrap();
    assert!(!first.socket_dir().exists());
    let result = other
        .call_tool(
            "agent_browser_get_text",
            json!({"selector":"#result"}),
            c.clone(),
        )
        .await
        .unwrap();
    assert!(result.text().contains("submissions=1 value=other-agent"));
    let restarted = BundledBrowserClient::new(options);
    restarted.connect().await.unwrap();
    for (ctx, value) in [(a, "first-chat"), (b, "project-chat")] {
        assert!(
            !restarted
                .call_tool("agent_browser_open", json!({"url":url}), ctx.clone())
                .await
                .unwrap()
                .is_error
        );
        let result = restarted
            .call_tool("agent_browser_get_text", json!({"selector":"#saved"}), ctx)
            .await
            .unwrap();
        assert!(
            result.text().contains(value),
            "cookie persistence: {result:?}"
        );
    }
    restarted.shutdown().await.unwrap();
    other.shutdown().await.unwrap();
    cancel.cancel();
    server.await.unwrap();
}

#[tokio::test]
#[ignore = "requires patched native driver and system Chromium"]
async fn real_stop_cancels_pending_click_at_execution_and_preserves_other_owner() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let temp = tempfile::tempdir().unwrap();
    let effects = Arc::new(AtomicUsize::new(0));
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let mut options = settings(&temp);
    options.driver = std::env::var_os("RUA_TEST_AGENT_BROWSER")
        .expect("patched driver")
        .into();
    options.browser = RuntimeCapability {
        name: "chromium".into(),
        available: true,
        path: Some(
            std::env::var_os("RUA_TEST_CHROMIUM")
                .expect("system Chromium")
                .into(),
        ),
        version: None,
        source: RuntimeCapabilitySource::Configured,
        diagnostics: vec![],
    };
    let app=axum::Router::new().route("/",axum::routing::get(||async{axum::response::Html(r#"<button id="later" onclick="fetch('/effect')">Submit</button><script>const rect=later.getBoundingClientRect.bind(later);later.getBoundingClientRect=()=>{let request=new XMLHttpRequest();request.open('GET','/release',false);request.send();return rect();}</script>"#)}))
        .route("/release",axum::routing::get({let entered=entered.clone();let release=release.clone();move || {let entered=entered.clone();let release=release.clone();async move {entered.notify_one();release.notified().await;"ok"}}}))
        .route("/effect",axum::routing::get({let effects=effects.clone();move||{let effects=effects.clone();async move {effects.fetch_add(1,Ordering::SeqCst);"ok"}}}));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let stop = tokio_util::sync::CancellationToken::new();
    let shutdown = stop.clone();
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(shutdown.cancelled_owned())
            .await
            .unwrap()
    });
    let first = BundledBrowserClient::new(options.clone());
    let other = BundledBrowserClient::new(options);
    let a = context("a", ChatAddress::agent("a", "stop"));
    let b = context("b", ChatAddress::agent("b", "safe"));
    let (one, two) = tokio::join!(first.connect(), other.connect());
    one.unwrap();
    two.unwrap();
    assert!(
        !first
            .call_tool("agent_browser_open", json!({"url":url}), a.clone())
            .await
            .unwrap()
            .is_error
    );
    assert!(
        !other
            .call_tool(
                "agent_browser_open",
                json!({"url":"about:blank"}),
                b.clone()
            )
            .await
            .unwrap()
            .is_error
    );
    let (cancel, rx) = tokio::sync::watch::channel(false);
    let mut input = a.clone();
    input.cancel = Some(rx);
    let started = tokio::time::Instant::now();
    let (result, ()) = tokio::join!(
        first.call_tool("agent_browser_click", json!({"selector":"#later"}), input),
        async {
            tokio::time::timeout(std::time::Duration::from_secs(5), entered.notified())
                .await
                .expect("native click reached geometry query");
            cancel.send(true).unwrap();
        }
    );
    let result = result.unwrap();
    assert!(result.is_error, "{result:?}");
    assert!(started.elapsed() < std::time::Duration::from_secs(6));
    let session = session_key(&a).unwrap();
    assert!(
        !first
            .socket_dir()
            .join(format!("{}.pid", &session[..24]))
            .exists(),
        "Stop must reach daemon and close its own browser"
    );
    release.notify_waiters();
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    assert_eq!(
        effects.load(Ordering::SeqCst),
        0,
        "late enabled button must never receive the cancelled click"
    );
    assert!(
        first
            .call_tool(
                "agent_browser_click",
                json!({"selector":"#later"}),
                a.clone()
            )
            .await
            .is_err(),
        "input requires reobservation after interruption"
    );
    let result = other
        .call_tool("agent_browser_get_url", json!({}), b)
        .await
        .unwrap();
    assert!(!result.is_error);
    assert!(result.text().contains("about:blank"));
    assert!(
        !first
            .call_tool("agent_browser_open", json!({"url":"about:blank"}), a)
            .await
            .unwrap()
            .is_error
    );
    first.shutdown().await.unwrap();
    other.shutdown().await.unwrap();
    stop.cancel();
    server.await.unwrap();
}

#[tokio::test]
#[cfg(unix)]
#[ignore = "requires patched native driver and system Chromium"]
async fn native_driver_lost_acknowledgement_does_not_resubmit_click() {
    use std::time::Duration;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let temp = tempfile::Builder::new()
        .prefix("rua-no-replay-")
        .tempdir_in("/tmp")
        .unwrap();
    let mut options = settings(&temp);
    options.driver = std::env::var_os("RUA_TEST_AGENT_BROWSER")
        .expect("patched driver")
        .into();
    options.browser = RuntimeCapability {
        name: "chromium".into(),
        available: true,
        path: Some(
            std::env::var_os("RUA_TEST_CHROMIUM")
                .expect("system Chromium")
                .into(),
        ),
        source: RuntimeCapabilitySource::Configured,
        version: None,
        diagnostics: vec![],
    };
    let client = BundledBrowserClient::new(options.clone());
    client.connect().await.unwrap();
    // Copy the actual daemon fingerprint established by the driver itself. The
    // fake endpoint simulates a click taking effect then losing its reply.
    let namespace = client.socket_dir();
    let listener = tokio::net::UnixListener::bind(namespace.join("ack.sock")).unwrap();
    for (suffix, data) in [
        ("pid", std::process::id().to_string()),
        ("version", DRIVER_VERSION.into()),
        (
            "config",
            std::fs::read_to_string(namespace.join("probe.config")).unwrap(),
        ),
    ] {
        rua_config::atomic_write(&namespace.join(format!("ack.{suffix}")), data.as_bytes())
            .unwrap();
    }
    let executable = options.browser.path.unwrap();
    let mut command = tokio::process::Command::new(options.driver);
    command
        .env("RUA_BROWSER_OWNER_FILE", namespace.join("owner"))
        .env("AGENT_BROWSER_SOCKET_DIR", namespace)
        .env("AGENT_BROWSER_CONFIG", namespace.join("driver.json"))
        .env("AGENT_BROWSER_EXECUTABLE_PATH", executable)
        .env("AGENT_BROWSER_NO_AUTO_DIALOG", "1")
        .env("AGENT_BROWSER_IDLE_TIMEOUT_MS", "900000")
        .args(["--json", "--session", "ack", "click", "#submit"])
        .kill_on_drop(true);
    let (result, count) = tokio::join!(command.output(), async {
        loop {
            let (stream, _) = tokio::time::timeout(Duration::from_secs(3), listener.accept())
                .await
                .unwrap()
                .unwrap();
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            if reader.read_line(&mut line).await.unwrap() == 0 {
                continue;
            }
            let call: Value = serde_json::from_str(&line).unwrap();
            if call["action"] == "launch" {
                let reply = json!({"id":call["id"],"success":true,"data":{}});
                reader
                    .get_mut()
                    .write_all(format!("{reply}\n").as_bytes())
                    .await
                    .unwrap();
                reader.get_mut().shutdown().await.unwrap();
                continue;
            }
            assert_eq!(call["action"], "click");
            drop(reader);
            break;
        }
        let second = tokio::time::timeout(Duration::from_millis(1200), listener.accept()).await;
        assert!(
            second.is_err(),
            "a lost acknowledgement must not cause a second native click request"
        );
        1
    });
    assert_eq!(count, 1);
    assert!(!result.unwrap().status.success());
    for suffix in ["pid", "version", "config", "sock"] {
        let _ = std::fs::remove_file(namespace.join(format!("ack.{suffix}")));
    }
    client.shutdown().await.unwrap();
}

#[tokio::test]
#[cfg(unix)]
#[ignore = "requires patched native driver and system Chromium"]
async fn native_daemon_crash_reaps_owned_browser_and_requires_new_observation() {
    use std::time::Duration;
    let temp = tempfile::tempdir().unwrap();
    let mut options = settings(&temp);
    options.driver = std::env::var_os("RUA_TEST_AGENT_BROWSER")
        .expect("patched driver")
        .into();
    options.browser = RuntimeCapability {
        name: "chromium".into(),
        available: true,
        path: Some(
            std::env::var_os("RUA_TEST_CHROMIUM")
                .expect("system Chromium")
                .into(),
        ),
        source: RuntimeCapabilitySource::Configured,
        version: None,
        diagnostics: vec![],
    };
    let client = BundledBrowserClient::new(options);
    let ctx = context("crash", ChatAddress::agent("crash", "native"));
    let opened = client
        .call_tool(
            "agent_browser_open",
            json!({"url":"about:blank"}),
            ctx.clone(),
        )
        .await
        .unwrap();
    assert!(!opened.is_error, "{opened:?}");
    let session = session_key(&ctx).unwrap();
    let pid: i32 =
        std::fs::read_to_string(client.socket_dir().join(format!("{}.pid", &session[..24])))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
    // Only the PID of the daemon this fixture launched in a private namespace.
    let processes = std::process::Command::new("ps")
        .args(["-axo", "pid=,ppid="])
        .output()
        .unwrap();
    let children: Vec<i32> = String::from_utf8(processes.stdout)
        .unwrap()
        .lines()
        .filter_map(|line| {
            let ids = line
                .split_whitespace()
                .filter_map(|v| v.parse::<i32>().ok())
                .collect::<Vec<_>>();
            (ids.len() == 2 && ids[1] == pid).then(|| ids[0])
        })
        .collect();
    assert_eq!(children.len(), 1, "one owned Chromium watchdog process");
    let watchdog = children[0];
    assert_eq!(unsafe { libc::kill(pid, libc::SIGKILL) }, 0);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while unsafe { libc::kill(-watchdog, 0) } == 0 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "owned Chrome process group survived daemon crash"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let input = client
        .call_tool(
            "agent_browser_click",
            json!({"selector":"#submit"}),
            ctx.clone(),
        )
        .await
        .unwrap();
    assert!(
        input.is_error,
        "dead session must not be recreated to execute stale input"
    );
    let recovered = client
        .call_tool("agent_browser_open", json!({"url":"about:blank"}), ctx)
        .await
        .unwrap();
    assert!(!recovered.is_error, "{recovered:?}");
    client.shutdown().await.unwrap();
}
