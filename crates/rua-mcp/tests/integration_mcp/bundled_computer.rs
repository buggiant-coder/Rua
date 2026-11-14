#[cfg(unix)]
use mcp_manager::types::{McpCallContext, McpConfig, McpServerId};
#[cfg(unix)]
use mcp_manager::{client::McpClient, manager::McpManager};
#[cfg(unix)]
use rua_contract::Tool;
#[cfg(unix)]
use rua_core::tool::{ToolContext, MODEL_SUPPORTS_IMAGE_METADATA_KEY};
#[cfg(unix)]
use rua_core::{ChatAddress, ContentPart};
use rua_mcp::bundled_computer::ComputerPermissions;
#[cfg(unix)]
use rua_mcp::bundled_computer::{BundledComputer, BundledComputerClient, SERVER_ID};
#[cfg(unix)]
use rua_mcp::{McpManagerImpl, McpMobilePolicy, McpToolBridge, PlatformProfile};
use serde_json::json;
#[cfg(unix)]
use serde_json::Value;
#[cfg(unix)]
use std::collections::BTreeMap;
#[cfg(unix)]
use std::sync::Arc;

#[cfg(unix)]
fn permissions() -> Value {
    json!({"source":{"attribution":"host"},"accessibility":true,"screen_recording":true})
}
#[cfg(unix)]
fn context(agent: &str) -> McpCallContext {
    McpCallContext {
        agent_id: agent.into(),
        session_id: "chat".into(),
        metadata: BTreeMap::from([
            ("turn_id".into(), json!("turn")),
            ("tool_call_id".into(), json!("input")),
            ("device_id".into(), json!("fixture-device")),
            (
                "security_chat_address".into(),
                serde_json::to_value(ChatAddress::agent(agent, "chat")).unwrap(),
            ),
            (MODEL_SUPPORTS_IMAGE_METADATA_KEY.into(), json!(true)),
            (
                "chat_capability_configuration".into(),
                json!({
                    "selection_policy":rua_config::types::tools::ToolSelectionPolicy::default_enabled()
                }),
            ),
        ]),
        ..Default::default()
    }
}
#[cfg(unix)]
fn fixture(root: &std::path::Path) -> BundledComputer {
    use std::os::unix::fs::PermissionsExt;
    let python = std::process::Command::new("python3")
        .args(["-c", "import sys; print(sys.executable)"])
        .output()
        .unwrap();
    assert!(python.status.success());
    let driver = root.join("fixture-driver");
    rua_config::atomic_write(
        &driver,
        format!(
            "#!{}\n{}",
            String::from_utf8(python.stdout).unwrap().trim(),
            include_str!("../fixtures/mock_cua.py")
        )
        .as_bytes(),
    )
    .unwrap();
    std::fs::set_permissions(&driver, std::fs::Permissions::from_mode(0o700)).unwrap();
    rua_config::atomic_write_json(&root.join("permissions.json"), &permissions()).unwrap();
    let connection_file = root.join("connection.json");
    rua_config::atomic_write_json(&connection_file,&json!({"generation":uuid::Uuid::new_v4().to_string(),"socket_path":"fixture","driver_version":rua_mcp::bundled_computer::DRIVER_VERSION,"host_bundle_id":"test.rua"})).unwrap();
    BundledComputer {
        driver,
        connection_file,
    }
}

#[test]
fn permissions_distinguish_windows_capability_from_macos_host_grants() {
    let win = ComputerPermissions::from_driver(
        &json!({"uia":true,"post_message":true,"integrity_level":"Medium"}),
    )
    .unwrap();
    assert_eq!(win.platform, "windows");
    assert_eq!(win.screen_recording, None);
    assert!(win.capture_allowed());
    let denied = ComputerPermissions::from_driver(
        &json!({"source":{"attribution":"host"},"accessibility":false,"screen_recording":false}),
    )
    .unwrap();
    assert!(!denied.input);
    assert!(!denied.capture_allowed());
    assert!(ComputerPermissions::from_driver(
        &json!({"source":{"attribution":"caller"},"accessibility":true,"screen_recording":true})
    )
    .is_err());
}

#[tokio::test]
#[cfg(unix)]
async fn observation_preserves_structured_images_identity_and_other_proxy() {
    let temp = tempfile::tempdir().unwrap();
    let options = fixture(temp.path());
    let first = BundledComputerClient::new(options.clone());
    let second = BundledComputerClient::new(options);
    let tools = first.list_tools().await.unwrap();
    assert_eq!(tools.len(), 5);
    assert!(tools
        .iter()
        .all(|t| t.input_schema["properties"].get("session").is_none()));
    let result = first
        .call_tool(
            "get_window_state",
            json!({"pid":100,"window_id":200}),
            context("a"),
        )
        .await
        .unwrap();
    let mapped = rua_mcp::tool_adapter::McpToolAdapter::map_call_result(&result);
    assert!(mapped.content.iter().any(|part| matches!(part,ContentPart::Image {url,..} if url.starts_with("data:image/png;base64,iVBOR"))));
    assert_eq!(
        mapped.metadata["mcp_structured_content"]["snapshot_id"],
        "snapshot-731"
    );
    assert!(mapped.text_content().contains("snapshot-731"));
    second
        .call_tool(
            "get_window_state",
            json!({"pid":100,"window_id":200}),
            context("b"),
        )
        .await
        .unwrap();
    first.shutdown().await.unwrap();
    assert!(
        !second
            .call_tool("list_windows", json!({"pid":100}), context("b"))
            .await
            .unwrap()
            .is_error
    );
    second.shutdown().await.unwrap();
    let calls: Vec<Value> = std::fs::read_to_string(temp.path().join("calls.jsonl"))
        .unwrap()
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    let observations: Vec<_> = calls
        .iter()
        .filter(|c| c["name"] == "get_window_state")
        .collect();
    assert_ne!(
        observations[0]["args"]["session"],
        observations[1]["args"]["session"]
    );
    for observation in observations {
        assert!(calls.iter().any(|c| c["name"] == "end_session"
            && c["args"]["session"] == observation["args"]["session"]
            && c["proxy"] == observation["proxy"]));
    }
}

#[tokio::test]
#[cfg(unix)]
async fn same_chat_observation_restarts_ended_driver_session_without_exposing_lifecycle_tools() {
    let temp = tempfile::tempdir().unwrap();
    let client = BundledComputerClient::new(fixture(temp.path()));
    client.connect().await.unwrap();
    for _ in 0..2 {
        let result = client
            .call_tool(
                "get_window_state",
                json!({"pid":100,"window_id":200}),
                context("a"),
            )
            .await
            .unwrap();
        assert!(!result.is_error, "{result:?}");
        client.shutdown().await.unwrap();
    }
    let calls: Vec<Value> = std::fs::read_to_string(temp.path().join("calls.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let starts: Vec<_> = calls
        .iter()
        .filter(|call| call["name"] == "start_session")
        .collect();
    assert_eq!(starts.len(), 2);
    assert_eq!(starts[0]["args"]["session"], starts[1]["args"]["session"]);
    assert!(client
        .list_tools()
        .await
        .unwrap()
        .iter()
        .all(|tool| !matches!(tool.name.as_str(), "start_session" | "end_session")));
}

#[cfg(unix)]
#[tokio::test]
async fn markdown_snapshot_tokens_are_registered_for_scoped_input() {
    let temp = tempfile::tempdir().unwrap();
    let client = BundledComputerClient::new(fixture(temp.path()));
    rua_config::atomic_write(&temp.path().join("markdown-elements"), b"1").unwrap();
    let observation = observe(&client, "a").await;
    let mut input = sequence(&observation, "token input");
    input["actions"][0]["arguments"]["element_token"] = json!("s00000001:7");
    let result = client
        .call_tool("interact", input, context("a"))
        .await
        .unwrap();
    assert!(!result.is_error, "{result:?}");
    assert_eq!(
        result.structured_content.as_ref().unwrap()["actions"][0]["arguments"]["element_token"],
        "s00000001:7"
    );
    assert_eq!(
        result.structured_content.as_ref().unwrap()["observation"]["elements"][0]["element_token"],
        "s00000001:7"
    );
    client.shutdown().await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn permission_revocation_hidden_controls_nonvisual_and_missing_image_fail_closed() {
    let temp = tempfile::tempdir().unwrap();
    let client = BundledComputerClient::new(fixture(temp.path()));
    let ctx = context("a");
    for (name, args) in [
        ("click", json!({"pid":100,"window_id":200,"x":1,"y":2})),
        (
            "get_window_state",
            json!({"pid":100,"window_id":200,"session":"victim"}),
        ),
        (
            "get_desktop_state",
            json!({"screenshot_out_file":"/tmp/victim"}),
        ),
    ] {
        assert!(client.call_tool(name, args, ctx.clone()).await.is_err());
    }
    let mut nonvisual = ctx.clone();
    nonvisual.metadata.remove(MODEL_SUPPORTS_IMAGE_METADATA_KEY);
    assert!(client
        .call_tool(
            "get_window_state",
            json!({"pid":100,"window_id":200}),
            nonvisual.clone()
        )
        .await
        .is_err());
    assert!(!temp.path().join("calls.jsonl").exists());
    client
        .call_tool(
            "get_window_state",
            json!({"pid":100,"window_id":200,"include_screenshot":false}),
            nonvisual,
        )
        .await
        .unwrap();
    rua_config::atomic_write_json(
        &temp.path().join("permissions.json"),
        &json!({"source":{"attribution":"host"},"accessibility":false,"screen_recording":false}),
    )
    .unwrap();
    assert_eq!(client.list_tools().await.unwrap().len(), 2);
    assert!(client
        .call_tool(
            "get_window_state",
            json!({"pid":100,"window_id":200}),
            ctx.clone()
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("permission"));
    rua_config::atomic_write_json(&temp.path().join("permissions.json"), &permissions()).unwrap();
    rua_config::atomic_write(&temp.path().join("omit-image"), b"1").unwrap();
    assert!(client
        .call_tool("get_window_state", json!({"pid":100,"window_id":200}), ctx)
        .await
        .unwrap_err()
        .to_string()
        .contains("screenshot"));
    client.shutdown().await.unwrap();
}

#[tokio::test]
#[cfg(unix)]
async fn defaults_preserve_user_controls_and_tool_guard_precedes_observation() {
    let temp = tempfile::tempdir().unwrap();
    let settings = fixture(temp.path());
    let mut config = settings.defaults(McpConfig::default());
    let id = McpServerId::new(SERVER_ID);
    config.clients.get_mut(&id).unwrap().disabled_tools = vec!["get_desktop_state".into()];
    assert_eq!(
        settings.defaults(config.clone()).clients[&id].disabled_tools,
        vec!["get_desktop_state"]
    );
    let mut third_party = config.clone();
    third_party.clients.get_mut(&id).unwrap().extra.clear();
    assert_eq!(settings.defaults(third_party.clone()), third_party);
    let manager = Arc::new(
        McpManagerImpl::new(McpMobilePolicy::new(PlatformProfile::Desktop))
            .with_bundled_computer(settings),
    );
    manager.load_config(config).await.unwrap();
    let security = Arc::new(rua_security::SecurityModule::new(
        Arc::new(rua_security::SecurityConfigStore::new(
            temp.path().join("policy"),
        )),
        None,
    ));
    let tools = McpToolBridge::new(manager.clone())
        .with_security(security)
        .discover_tools()
        .await
        .unwrap();
    assert!(tools
        .iter()
        .all(|t| t.original_name() != "get_desktop_state"));
    let tool = tools
        .iter()
        .find(|t| t.original_name() == "get_window_state")
        .unwrap();
    let mut ctx = ToolContext {
        agent_id: "a".into(),
        session_id: "chat".into(),
        metadata: context("a").metadata.into_iter().collect(),
        ..Default::default()
    };
    let mut config = rua_config::types::security::SecurityConfig::default();
    config.tool_guard.approval_mode = "ask_first".into();
    ctx.metadata.insert(
        rua_security::models::EFFECTIVE_SECURITY_CONFIG_METADATA_KEY.into(),
        serde_json::to_value(config).unwrap(),
    );
    ctx.metadata.insert("turn_id".into(), json!("turn"));
    ctx.metadata.insert("tool_call_id".into(), json!("observe"));
    assert!(tool
        .call(json!({"pid":100,"window_id":200}), ctx)
        .await
        .is_err());
    let calls = std::fs::read_to_string(temp.path().join("calls.jsonl")).unwrap();
    assert!(!calls.contains("get_window_state"));
    manager.shutdown_all().await.unwrap();
}

#[cfg(unix)]
async fn observe(client: &BundledComputerClient, agent: &str) -> Value {
    client
        .call_tool(
            "get_window_state",
            json!({"pid":100,"window_id":200}),
            context(agent),
        )
        .await
        .unwrap()
        .structured_content
        .unwrap()
}
#[cfg(unix)]
fn sequence(observation: &Value, text: &str) -> Value {
    json!({"pid":100,"window_id":200,"observation_id":observation["observation_id"],"actions":[
        {"tool":"type_text","arguments":{"text":text}},
        {"tool":"press_key","arguments":{"key":"return"}}
    ]})
}
#[cfg(unix)]
fn input_calls(root: &std::path::Path) -> Vec<Value> {
    std::fs::read_to_string(root.join("calls.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|c| {
            matches!(
                c["name"].as_str(),
                Some("type_text" | "click" | "press_key" | "scroll")
            )
        })
        .collect()
}
#[cfg(unix)]
async fn wait_for(path: &std::path::Path) {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !path.exists() {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
#[cfg(unix)]
async fn sequences_hold_device_control_across_steps_and_stop_cancels_waiter() {
    let temp = tempfile::tempdir().unwrap();
    let settings = fixture(temp.path());
    let first = BundledComputerClient::new(settings.clone());
    let second = BundledComputerClient::new(settings);
    let a = observe(&first, "a").await;
    let b = observe(&second, "b").await;
    rua_config::atomic_write(&temp.path().join("pause-input"), b"1").unwrap();
    let (cancel, rx) = tokio::sync::watch::channel(false);
    let (events, mut stream) = tokio::sync::mpsc::channel(8);
    let mut second_ctx = context("b");
    second_ctx.cancel = Some(rx);
    second_ctx.event_tx = Some(events);
    let (one, two, ()) = tokio::join!(
        first.call_tool("interact", sequence(&a, "first"), context("a")),
        async {
            wait_for(&temp.path().join("input-entered")).await;
            second
                .call_tool("interact", sequence(&b, "second"), second_ctx)
                .await
        },
        async {
            let event = tokio::time::timeout(std::time::Duration::from_secs(5), stream.recv())
                .await
                .unwrap()
                .unwrap();
            assert!(
                matches!(event,agent_types::AgentEvent::ToolResultDelta{metadata,..} if metadata["state"]=="waiting_for_desktop_control")
            );
            cancel.send(true).unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            rua_config::atomic_write(&temp.path().join("resume-input"), b"1").unwrap();
        }
    );
    assert!(!one.unwrap().is_error);
    assert!(two.unwrap_err().to_string().contains("cancel"));
    let calls = input_calls(temp.path());
    assert_eq!(calls.len(), 6);
    assert_eq!(
        calls
            .iter()
            .filter_map(|call| call["args"]["text"].as_str())
            .collect::<String>(),
        "first"
    );
    let result = second
        .call_tool("interact", sequence(&b, "second"), context("b"))
        .await
        .unwrap();
    assert!(!result.is_error, "{result:?}");
    let calls = input_calls(temp.path());
    assert_eq!(calls.len(), 13);
    assert_eq!(
        calls[6..]
            .iter()
            .filter_map(|call| call["args"]["text"].as_str())
            .collect::<String>(),
        "second"
    );
    assert!(calls[..6]
        .iter()
        .all(|call| call["proxy"] == calls[0]["proxy"]));
    assert!(calls[6..]
        .iter()
        .all(|call| call["proxy"] == calls[6]["proxy"]));
    first.shutdown().await.unwrap();
    second.shutdown().await.unwrap();
}

#[tokio::test]
#[cfg(unix)]
async fn stop_after_submission_finishes_only_current_step_and_releases_control() {
    let temp = tempfile::tempdir().unwrap();
    let settings = fixture(temp.path());
    let client = BundledComputerClient::new(settings);
    let observation = observe(&client, "a").await;
    rua_config::atomic_write(&temp.path().join("pause-input"), b"1").unwrap();
    let (cancel, rx) = tokio::sync::watch::channel(false);
    let mut ctx = context("a");
    ctx.cancel = Some(rx);
    let (result, ()) = tokio::join!(
        client.call_tool("interact", sequence(&observation, "one"), ctx),
        async {
            wait_for(&temp.path().join("input-entered")).await;
            cancel.send(true).unwrap();
            rua_config::atomic_write(&temp.path().join("resume-input"), b"1").unwrap();
        }
    );
    let result = result.unwrap();
    assert!(result.is_error);
    assert_eq!(input_calls(temp.path()).len(), 1);
    assert_eq!(input_calls(temp.path())[0]["args"]["text"], "o");
    assert_eq!(result.structured_content.unwrap()["outcome_unknown"], false);
    let observation = observe(&client, "a").await;
    assert!(
        !client
            .call_tool("interact", sequence(&observation, "next"), context("a"))
            .await
            .unwrap()
            .is_error
    );
    client.shutdown().await.unwrap();
}

#[tokio::test]
#[cfg(unix)]
async fn stale_observation_and_invalid_target_prevent_input_then_recovery_is_explicit() {
    let temp = tempfile::tempdir().unwrap();
    let settings = fixture(temp.path());
    let client = BundledComputerClient::new(settings);
    let first = observe(&client, "a").await;
    let latest = observe(&client, "a").await;
    assert!(client
        .call_tool("interact", sequence(&first, "stale"), context("a"))
        .await
        .is_err());
    assert!(client
        .call_tool("interact", sequence(&latest, "foreign"), context("b"))
        .await
        .is_err());
    rua_config::atomic_write(&temp.path().join("window-gone"), b"1").unwrap();
    assert!(
        client
            .call_tool("interact", sequence(&latest, "gone"), context("a"))
            .await
            .unwrap()
            .is_error
    );
    assert!(input_calls(temp.path()).is_empty());
    std::fs::remove_file(temp.path().join("window-gone")).unwrap();
    let observed = observe(&client, "a").await;
    rua_config::atomic_write(&temp.path().join("drop-after-input"), b"1").unwrap();
    let result = client
        .call_tool("interact", sequence(&observed, "maybe"), context("a"))
        .await
        .unwrap();
    assert!(result.is_error);
    assert_eq!(result.structured_content.unwrap()["outcome_unknown"], true);
    assert_eq!(input_calls(temp.path()).len(), 1);
    assert!(client.control_requires_restart().await);
    std::fs::remove_file(temp.path().join("drop-after-input")).unwrap();
    // A new proxy is insufficient to clear an uncertain physical input.
    client.shutdown().await.unwrap();
    client.connect().await.unwrap();
    let observed = observe(&client, "a").await;
    assert!(client
        .call_tool("interact", sequence(&observed, "no replay"), context("a"))
        .await
        .unwrap_err()
        .to_string()
        .contains("unknown outcome"));
    assert_eq!(input_calls(temp.path()).len(), 1);
    client.shutdown().await.unwrap();
}

#[tokio::test]
#[cfg(unix)]
async fn dropped_input_future_notifies_owner_without_replay() {
    let temp = tempfile::tempdir().unwrap();
    let settings = fixture(temp.path());
    let generation = settings.connection_generation().unwrap();
    let client = BundledComputerClient::new(settings);
    let observed = observe(&client, "a").await;
    rua_config::atomic_write(&temp.path().join("pause-input"), b"1").unwrap();
    let mut recovery = rua_mcp::computer_control::recovery_requests();
    let (result, ()) = tokio::join!(
        tokio::time::timeout(
            std::time::Duration::from_millis(200),
            client.call_tool("interact", sequence(&observed, "once"), context("a"))
        ),
        async {
            wait_for(&temp.path().join("input-entered")).await;
        }
    );
    assert!(result.is_err());
    recovery.changed().await.unwrap();
    assert!(client.control_requires_restart().await);
    client.shutdown().await.unwrap();
    assert_eq!(input_calls(temp.path()).len(), 1);
    let fresh = observe(&client, "a").await;
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (done_tx, done_rx) = tokio::sync::oneshot::channel();
    let (recovered, ()) = tokio::join!(
        rua_mcp::computer_control::recover(&generation, async {
            started_tx.send(()).unwrap();
            done_rx.await.unwrap();
            Ok(())
        }),
        async {
            started_rx.await.unwrap();
            assert!(client.control_requires_restart().await);
            let rejected = tokio::time::timeout(
                std::time::Duration::from_millis(200),
                client.call_tool("interact", sequence(&fresh, "blocked"), context("a")),
            )
            .await
            .unwrap()
            .unwrap_err();
            assert!(rejected.to_string().contains("unknown outcome"));
            done_tx.send(()).unwrap();
        }
    );
    recovered.unwrap();
    client.shutdown().await.unwrap();
}

#[tokio::test]
#[cfg(unix)]
async fn sequence_approval_covers_visible_steps_and_leaf_deny_is_not_hidden() {
    use rua_security::{
        ApprovalDecision, ApprovalReuseMatcher, PermissionRuleEffect, PermissionRuleInput,
    };
    let temp = tempfile::tempdir().unwrap();
    let settings = fixture(temp.path());
    let manager = Arc::new(
        McpManagerImpl::new(McpMobilePolicy::new(PlatformProfile::Desktop))
            .with_bundled_computer(settings),
    );
    manager.load_config(McpConfig::default()).await.unwrap();
    let security = Arc::new(rua_security::SecurityModule::new(
        Arc::new(rua_security::SecurityConfigStore::new(
            temp.path().join("policy"),
        )),
        None,
    ));
    let bridge = McpToolBridge::new(manager.clone()).with_security(security.clone());
    let tools = bridge.discover_tools().await.unwrap();
    let read = tools
        .iter()
        .find(|t| t.original_name() == "get_window_state")
        .unwrap();
    let interact = tools
        .iter()
        .find(|t| t.original_name() == "interact")
        .unwrap();
    let mut ctx = ToolContext {
        agent_id: "a".into(),
        session_id: "chat".into(),
        metadata: context("a").metadata.into_iter().collect(),
        ..Default::default()
    };
    let observation = read
        .call(json!({"pid":100,"window_id":200}), ctx.clone())
        .await
        .unwrap()
        .metadata["mcp_structured_content"]
        .clone();
    let args = sequence(&observation, "approved once");
    let mut config = rua_config::types::security::SecurityConfig::default();
    config.tool_guard.approval_mode = "ask_first".into();
    ctx.metadata.insert(
        rua_security::models::EFFECTIVE_SECURITY_CONFIG_METADATA_KEY.into(),
        serde_json::to_value(config).unwrap(),
    );
    assert!(interact.call(args.clone(), ctx.clone()).await.is_err());
    assert!(input_calls(temp.path()).is_empty());
    let pending = security
        .approval_authority()
        .snapshot_at(&ChatAddress::agent("a", "chat"))
        .await
        .pending;
    assert_eq!(pending.len(), 1);
    security
        .approval_authority()
        .decide(
            &pending[0].id,
            ApprovalDecision::RunOnce {
                decided_by: "user".into(),
            },
        )
        .await
        .unwrap();
    let result = interact.call(args, ctx.clone()).await.unwrap();
    assert!(!result.is_error, "{result:?}");
    assert_eq!(
        input_calls(temp.path()).len(),
        "approved once".chars().count() + 1
    );
    ctx.metadata
        .remove(rua_security::models::EFFECTIVE_SECURITY_CONFIG_METADATA_KEY);
    let observation = read
        .call(json!({"pid":100,"window_id":200}), ctx.clone())
        .await
        .unwrap()
        .metadata["mcp_structured_content"]
        .clone();
    security
        .approval_authority()
        .create_agent_rule(
            "a",
            PermissionRuleInput {
                effect: PermissionRuleEffect::Deny,
                enabled: true,
                label: "No desktop typing".into(),
                matcher: ApprovalReuseMatcher::ToolCall {
                    tool_name: "mcp__rua_computer__type_text".into(),
                    input_params: None,
                },
            },
        )
        .await
        .unwrap();
    let result = interact
        .call(sequence(&observation, "must not run"), ctx)
        .await
        .unwrap_err();
    assert!(result.to_string().contains("No desktop typing"));
    assert_eq!(
        input_calls(temp.path()).len(),
        "approved once".chars().count() + 1
    );
    manager.shutdown_all().await.unwrap();
}

#[tokio::test]
#[cfg(unix)]
async fn disconnect_cancels_in_flight_input_without_replay_and_recovers_status() {
    use mcp_manager::{status::McpConnectionState, types::ToolFilter};
    let temp = tempfile::tempdir().unwrap();
    let options = fixture(temp.path());
    let generation = options.connection_generation().unwrap();
    let mut recovery = rua_mcp::computer_control::recovery_requests();
    let descriptor = options.connection_file.clone();
    let manager = McpManagerImpl::new(McpMobilePolicy::new(PlatformProfile::Desktop))
        .with_bundled_computer(options);
    manager.load_config(McpConfig::default()).await.unwrap();
    let id = McpServerId::new(SERVER_ID);
    let observation = manager
        .call_tool(
            &id,
            "get_window_state",
            json!({"pid":100,"window_id":200}),
            context("a"),
        )
        .await
        .unwrap()
        .structured_content
        .unwrap();
    rua_config::atomic_write(&temp.path().join("pause-input"), b"1").unwrap();
    let (result, disconnect) = tokio::time::timeout(std::time::Duration::from_secs(4), async {
        tokio::join!(
            manager.call_tool(
                &id,
                "interact",
                sequence(&observation, "must not complete"),
                context("a")
            ),
            async {
                wait_for(&temp.path().join("input-entered")).await;
                manager.disconnect_server(&id).await
            }
        )
    })
    .await
    .expect("disconnect must cancel the pending call before taking its lock");
    assert!(result.unwrap_err().to_string().contains("cancel"));
    disconnect.unwrap();
    recovery.changed().await.unwrap();
    assert!(rua_mcp::computer_control::recover(&generation, async {
        Err("executor still alive".into())
    })
    .await
    .is_err());
    assert!(manager
        .list_tools(ToolFilter::default())
        .await
        .unwrap()
        .iter()
        .any(|tool| tool.name == "interact"));
    assert_eq!(input_calls(temp.path()).len(), 1);
    std::fs::remove_file(temp.path().join("pause-input")).unwrap();
    let observed = manager
        .call_tool(
            &id,
            "get_window_state",
            json!({"pid":100,"window_id":200}),
            context("a"),
        )
        .await
        .unwrap()
        .structured_content
        .unwrap();
    assert!(manager
        .call_tool(
            &id,
            "interact",
            sequence(&observed, "no replay"),
            context("a")
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("unknown outcome"));
    assert_eq!(input_calls(temp.path()).len(), 1);
    rua_config::atomic_write_json(&descriptor, &json!({"error":"host stopped"})).unwrap();
    let filter = ToolFilter {
        server_id: Some(id.clone()),
        ..Default::default()
    };
    assert!(manager.list_tools(filter.clone()).await.is_err());
    assert_eq!(
        manager.list_servers().await.unwrap()[0].state,
        McpConnectionState::Failed
    );
    // Only a new native-host generation can release the unknown physical input.
    rua_mcp::computer_control::recover(&generation, async {
        rua_config::atomic_write_json(&descriptor, &json!({"generation":uuid::Uuid::new_v4().to_string(),"socket_path":"fixture","driver_version":rua_mcp::bundled_computer::DRIVER_VERSION,"host_bundle_id":"test.rua"})).unwrap();
        Ok(())
    }).await.unwrap();
    let stale = manager
        .call_tool(&id, "interact", sequence(&observed, "stale"), context("a"))
        .await
        .unwrap_err();
    assert!(stale.to_string().contains("Observe"));
    manager.list_tools(filter).await.unwrap();
    let status = manager.list_servers().await.unwrap().remove(0);
    assert_eq!(status.state, McpConnectionState::Ready);
    assert!(status.last_error.is_none());
    let observed = manager
        .call_tool(
            &id,
            "get_window_state",
            json!({"pid":100,"window_id":200}),
            context("a"),
        )
        .await
        .unwrap()
        .structured_content
        .unwrap();
    assert!(
        !manager
            .call_tool(&id, "interact", sequence(&observed, "new"), context("a"))
            .await
            .unwrap()
            .is_error
    );
    assert_eq!(input_calls(temp.path()).len(), 5);
    assert!(manager.shutdown_all().await.unwrap().errors.is_empty());
}

#[tokio::test]
#[cfg(unix)]
async fn failed_new_observation_invalidates_the_previous_window_reference() {
    let temp = tempfile::tempdir().unwrap();
    let client = BundledComputerClient::new(fixture(temp.path()));
    let observed = observe(&client, "a").await;
    rua_config::atomic_write(&temp.path().join("omit-image"), b"1").unwrap();
    assert!(client
        .call_tool(
            "get_window_state",
            json!({"pid":100,"window_id":200}),
            context("a")
        )
        .await
        .is_err());
    std::fs::remove_file(temp.path().join("omit-image")).unwrap();
    assert!(client
        .call_tool("interact", sequence(&observed, "stale"), context("a"))
        .await
        .is_err());
    assert!(input_calls(temp.path()).is_empty());
    client.shutdown().await.unwrap();
}

#[tokio::test]
#[cfg(unix)]
async fn captured_leaf_policy_survives_manager_reload() {
    use rua_config::types::tools::{ToolIdentity, ToolSelectionPolicy};
    let temp = tempfile::tempdir().unwrap();
    let options = fixture(temp.path());
    let id = McpServerId::new(SERVER_ID);
    let manager = McpManagerImpl::new(McpMobilePolicy::new(PlatformProfile::Desktop))
        .with_bundled_computer(options);
    manager.load_config(McpConfig::default()).await.unwrap();
    let mut captured = context("a");
    let observed = manager
        .call_tool(
            &id,
            "get_window_state",
            json!({"pid":100,"window_id":200}),
            captured.clone(),
        )
        .await
        .unwrap()
        .structured_content
        .unwrap();
    let result = manager
        .call_tool(
            &id,
            "interact",
            sequence(&observed, "old"),
            captured.clone(),
        )
        .await
        .unwrap();
    assert!(!result.is_error, "{result:?}");
    assert_eq!(input_calls(temp.path()).len(), 4);
    let mut policy = ToolSelectionPolicy::default_enabled();
    policy.set_tool_enabled(ToolIdentity::mcp(SERVER_ID, "type_text"), false);
    captured.metadata.insert(
        "chat_capability_configuration".into(),
        json!({"selection_policy":policy}),
    );
    manager.reload(McpConfig::default()).await.unwrap();
    let observed = manager
        .call_tool(
            &id,
            "get_window_state",
            json!({"pid":100,"window_id":200}),
            captured.clone(),
        )
        .await
        .unwrap()
        .structured_content
        .unwrap();
    let error = manager
        .call_tool(
            &id,
            "interact",
            sequence(&observed, "must not run"),
            captured.clone(),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("disabled"));
    assert_eq!(input_calls(temp.path()).len(), 4);
    captured.metadata.remove("chat_capability_configuration");
    let error = manager
        .call_tool(
            &id,
            "interact",
            sequence(&observed, "must not run"),
            captured,
        )
        .await
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("requires captured Tool selection policy"));
    assert_eq!(input_calls(temp.path()).len(), 4);
    manager.shutdown_all().await.unwrap();
}

#[tokio::test]
#[cfg(unix)]
async fn computer_sequence_enforces_captured_tool_selection_for_each_leaf() {
    use rua_config::types::tools::{ToolIdentity, ToolSelectionPolicy};
    let temp = tempfile::tempdir().unwrap();
    let manager = McpManagerImpl::new(McpMobilePolicy::new(PlatformProfile::Desktop))
        .with_bundled_computer(fixture(temp.path()));
    manager.load_config(McpConfig::default()).await.unwrap();
    let id = McpServerId::new(SERVER_ID);
    let mut captured = context("a");
    captured.metadata.insert(
        "chat_configuration_epoch".into(),
        json!("captured-restricted"),
    );
    captured.metadata.insert(
        rua_core::tool::EFFECTIVE_DISABLED_TOOLS_METADATA_KEY.into(),
        json!([]),
    );
    let restricted = ToolSelectionPolicy::restricted(vec![
        ToolIdentity::mcp(SERVER_ID, "interact"),
        ToolIdentity::mcp(SERVER_ID, "get_window_state"),
        ToolIdentity::mcp(SERVER_ID, "press_key"),
    ]);
    captured.metadata.insert(
        "chat_capability_configuration".into(),
        json!({"selection_policy":restricted}),
    );
    let observed = manager
        .call_tool(
            &id,
            "get_window_state",
            json!({"pid":100,"window_id":200}),
            captured.clone(),
        )
        .await
        .unwrap()
        .structured_content
        .unwrap();
    let error = manager
        .call_tool(
            &id,
            "interact",
            sequence(&observed, "blocked"),
            captured.clone(),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("type_text"));
    assert!(input_calls(temp.path()).is_empty());

    let policy:ToolSelectionPolicy=serde_json::from_value(json!({"mode":"default_enabled","disabled_tools":[],"disabled_capabilities":["computer_use"]})).unwrap();
    captured.metadata.insert(
        "chat_capability_configuration".into(),
        json!({"selection_policy":policy}),
    );
    assert!(manager
        .call_tool(
            &id,
            "interact",
            sequence(&observed, "blocked"),
            captured.clone()
        )
        .await
        .is_err());
    assert!(input_calls(temp.path()).is_empty());

    captured
        .metadata
        .remove(rua_core::tool::EFFECTIVE_DISABLED_TOOLS_METADATA_KEY);
    captured.metadata.insert(
        "chat_capability_configuration".into(),
        json!({"selection_policy":ToolSelectionPolicy::default_enabled()}),
    );
    let result = manager
        .call_tool(
            &id,
            "interact",
            sequence(&observed, "allowed"),
            captured.clone(),
        )
        .await
        .unwrap();
    assert!(!result.is_error, "{result:?}");
    let allowed_input_count = input_calls(temp.path()).len();
    assert!(allowed_input_count > 0);

    captured.metadata.insert(
        "chat_capability_configuration".into(),
        json!({"selection_policy":null}),
    );
    assert!(manager
        .call_tool(&id, "interact", sequence(&observed, "invalid"), captured)
        .await
        .is_err());
    assert_eq!(input_calls(temp.path()).len(), allowed_input_count);
    manager.shutdown_all().await.unwrap();
}
