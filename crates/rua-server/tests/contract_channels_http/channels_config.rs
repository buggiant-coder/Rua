use axum::http::{Method, StatusCode};
use serde_json::json;

use super::support::{build_app, channels_payload, create_agent, delete_agent, read_json, request};

fn assert_builtin_channel_types(types: &serde_json::Value) {
    let types = types.as_array().expect("channel types array");
    let actual: Vec<&str> = types
        .iter()
        .map(|value| value.as_str().expect("channel type string"))
        .collect();
    let expected = vec![
        "console",
        "dingtalk",
        "discord",
        "feishu",
        "imessage",
        "matrix",
        "mattermost",
        "mqtt",
        "onebot",
        "qq",
        "sip",
        "slack",
        "telegram",
        "voice",
        "wecom",
        "wechat",
        "xiaoyi",
        "yuanbao",
    ];
    assert_eq!(actual, expected);

    for expected in expected {
        assert!(
            actual.iter().any(|value| value == &expected),
            "missing builtin channel type {expected}: {types:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn channel_health_and_restart_match_qwenpaw_contract_shape() {
    let app = build_app();
    let agent_id = "integ_channels_health_restart";
    create_agent(&app.app, agent_id, "Channels health restart").await;

    let health = request(
        &app.app,
        Method::GET,
        "/api/config/channels/console/health",
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(health.status(), StatusCode::OK);
    let health_body = read_json(health).await;
    assert_eq!(health_body["channel"], "console");
    assert_eq!(health_body["status"], "healthy");
    assert!(health_body["detail"].is_string());
    assert_eq!(health_body["components"]["config"], "enabled");
    assert_eq!(health_body["components"]["runtime"], "running");
    assert_eq!(health_body["components"]["transport"], "connected");
    assert_eq!(health_body["components"]["queue"], "ready");

    let restart = request(
        &app.app,
        Method::POST,
        &format!("/api/agents/{agent_id}/config/channels/console/restart"),
        None,
        None,
    )
    .await;
    assert_eq!(restart.status(), StatusCode::OK);
    assert_eq!(
        read_json(restart).await,
        json!({
            "channel": "console",
            "status": "restarted",
            "detail": "Channel 'console' has been restarted."
        })
    );

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sip_channel_health_and_restart_use_real_runtime() {
    let app = build_app();
    let agent_id = "integ_channels_sip_runtime";
    create_agent(&app.app, agent_id, "SIP channel runtime").await;
    let original = app.state.registry.get_or_create(agent_id).await.unwrap();
    let memory = original.memory_manager().await.unwrap();
    let react = original.react_runtime().await.unwrap();

    let put = request(
        &app.app,
        Method::PUT,
        "/api/config/channels/sip",
        Some(json!({
            "enabled": true,
            "bind_addr": "127.0.0.1:0",
            "sip_mode": "native",
            "rtp_bind_addr": "127.0.0.1:0",
            "username": "rua",
            "domain": "127.0.0.1",
            "max_call_secs": 1
        })),
        Some(agent_id),
    )
    .await;
    assert_eq!(put.status(), StatusCode::OK);

    let restart = request(
        &app.app,
        Method::POST,
        &format!("/api/agents/{agent_id}/config/channels/sip/restart"),
        None,
        None,
    )
    .await;
    assert_eq!(restart.status(), StatusCode::OK);
    assert_eq!(
        read_json(restart).await,
        json!({
            "channel": "sip",
            "status": "restarted",
            "detail": "Channel 'sip' has been restarted."
        })
    );

    let health = request(
        &app.app,
        Method::GET,
        "/api/config/channels/sip/health",
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(health.status(), StatusCode::OK);
    let health_body = read_json(health).await;
    assert_eq!(health_body["channel"], "sip");
    assert_eq!(health_body["status"], "healthy");
    assert_eq!(health_body["components"]["runtime"], "running");
    assert_eq!(health_body["components"]["transport"], "unknown");
    assert!(std::sync::Arc::ptr_eq(
        &original,
        &app.state.registry.get(agent_id).await.unwrap()
    ));
    assert!(std::sync::Arc::ptr_eq(
        &memory,
        &original.memory_manager().await.unwrap()
    ));
    assert!(std::sync::Arc::ptr_eq(
        &react,
        &original.react_runtime().await.unwrap()
    ));

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sip_requires_explicit_mode_without_overwriting_saved_config() {
    let app = build_app();
    let agent_id = "canonical_channel_fields";
    create_agent(&app.app, agent_id, "Canonical fields").await;
    let path = format!("/api/agents/{agent_id}/config/channels/sip");
    let before = read_json(request(&app.app, Method::GET, &path, None, None).await).await;
    let invalid = request(
        &app.app,
        Method::PUT,
        &format!("/api/agents/{agent_id}/config/channels/sip"),
        Some(json!({"enabled":false,"bind_addr":"127.0.0.1:0"})),
        None,
    )
    .await;
    assert_eq!(invalid.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let after = read_json(request(&app.app, Method::GET, &path, None, None).await).await;
    assert_eq!(after, before);
    delete_agent(&app.app, agent_id).await;
}

#[test]
fn conflict_identifiers_use_canonical_channel_fields() {
    use rua_workspace::channel_conflict::extract_identifier;
    let matrix = json!({"homeserver":"https://matrix.test","user_id":"bot"});
    assert_eq!(
        extract_identifier("matrix", matrix.as_object().unwrap()).as_deref(),
        Some("bot@https://matrix.test")
    );
    let mqtt = json!({"host":"mqtt.test","port":1883});
    assert_eq!(
        extract_identifier("mqtt", mqtt.as_object().unwrap()).as_deref(),
        Some("mqtt.test:1883")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn boolean_config_errors_do_not_change_saved_channel_settings() {
    let app = build_app();
    let agent = "channel-bool-validation";
    create_agent(&app.app, agent, "Boolean validation").await;
    let path = format!("/api/agents/{agent}/config/channels/dingtalk");
    let saved = json!({"enabled":false,"enable_ws":true,"show_tool_details":true,"require_mention":true,"access_control_dm":true,"access_control_group":true,"app_secret":"private-secret","vendor":{"x":1}});
    assert_eq!(
        request(&app.app, Method::PUT, &path, Some(saved), None)
            .await
            .status(),
        StatusCode::OK
    );
    let before = read_json(request(&app.app, Method::GET, &path, None, None).await).await;
    for field in [
        "enabled",
        "enable_ws",
        "show_tool_details",
        "require_mention",
        "access_control_dm",
        "access_control_group",
    ] {
        for invalid in [json!(null), json!("false"), json!(0)] {
            let response = request(
                &app.app,
                Method::PUT,
                &path,
                Some(json!({field:invalid})),
                None,
            )
            .await;
            assert_eq!(
                response.status(),
                StatusCode::UNPROCESSABLE_ENTITY,
                "{field}"
            );
            let detail = read_json(response).await.to_string();
            assert!(detail.contains(field));
            assert!(!detail.contains("private-secret"));
        }
    }
    assert_eq!(
        read_json(request(&app.app, Method::GET, &path, None, None).await).await,
        before
    );
    let response = request(
        &app.app,
        Method::PUT,
        &format!("/api/agents/{agent}/config/channels"),
        Some(json!({"dingtalk":{"enabled":false,"enable_ws":null}})),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        read_json(request(&app.app, Method::GET, &path, None, None).await).await,
        before
    );
    delete_agent(&app.app, agent).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mqtt_invalid_port_is_reported_before_configuration_is_written() {
    let app = build_app();
    let agent_id = "mqtt-port-validation";
    create_agent(&app.app, agent_id, "MQTT port validation").await;
    let path = format!("/api/agents/{agent_id}/config/channels/mqtt");
    let configured = json!({"enabled":false,"host":"mqtt.example.test","port":8883,"password":"private-password","vendor":{"x":1}});
    let response = request(&app.app, Method::PUT, &path, Some(configured), None).await;
    assert_eq!(response.status(), StatusCode::OK);
    let before = read_json(request(&app.app, Method::GET, &path, None, None).await).await;
    for port in [
        json!(null),
        json!("invalid"),
        json!(-1),
        json!(65536),
        json!(1.5),
        json!(true),
    ] {
        let response = request(
            &app.app,
            Method::PUT,
            &path,
            Some(json!({"enabled":false,"port":port})),
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let error = read_json(response).await.to_string();
        assert!(error.contains("mqtt.port"));
        assert!(!error.contains("private-password"));
        let after = read_json(request(&app.app, Method::GET, &path, None, None).await).await;
        assert_eq!(after, before);
    }
    let bulk = request(
        &app.app,
        Method::PUT,
        &format!("/api/agents/{agent_id}/config/channels"),
        Some(json!({"mqtt":{"enabled":false,"port":"invalid"}})),
        None,
    )
    .await;
    assert_eq!(bulk.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        read_json(request(&app.app, Method::GET, &path, None, None).await).await,
        before
    );
    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn channel_schema_endpoint_describes_builtin_fields_and_scope() {
    let app = build_app();

    let schemas = request(
        &app.app,
        Method::GET,
        "/api/config/channels/schemas",
        None,
        Some("default"),
    )
    .await;
    assert_eq!(schemas.status(), StatusCode::OK);
    let body = read_json(schemas).await;
    assert_eq!(body["scope"], "builtin");
    assert_eq!(body["plugin_channels"], "out_of_scope");
    assert_builtin_channel_types(&body["order"]);
    let channels = body["channels"].as_object().expect("channels object");
    for channel in body["order"].as_array().expect("order array") {
        let channel = channel.as_str().expect("channel id");
        assert!(
            channels.contains_key(channel),
            "schema missing builtin channel {channel}: {body:?}"
        );
        assert_eq!(channels[channel]["id"], channel);
        assert!(channels[channel]["label"]["en"].as_str().is_some());
        assert!(channels[channel]["label"]["zh"].as_str().is_some());
        assert!(channels[channel]["icon"].as_str().is_some());
        assert!(channels[channel]["docs"]["en"].as_str().is_some());
        assert!(channels[channel]["fields"].as_array().is_some());
    }

    let telegram = &channels["telegram"];
    assert_eq!(telegram["required_fields"], json!(["bot_token"]));
    let bot_token = telegram["fields"]
        .as_array()
        .expect("telegram fields")
        .iter()
        .find(|field| field["name"] == "bot_token")
        .expect("bot_token field");
    assert_eq!(bot_token["secret"], true);
    assert_eq!(bot_token["required"], true);
    assert_eq!(bot_token["kind"], "password");

    let dingtalk_message_type = channels["dingtalk"]["fields"]
        .as_array()
        .expect("dingtalk fields")
        .iter()
        .find(|field| field["name"] == "message_type")
        .expect("message_type field");
    assert_eq!(dingtalk_message_type["options"][0]["value"], "markdown");
    assert_eq!(dingtalk_message_type["options"][1]["value"], "card");

    for name in [
        "show_tool_calls",
        "show_tool_results",
        "tool_call_max_length",
        "tool_result_max_length",
        "show_thinking",
        "no_text_debounce",
    ] {
        assert!(
            telegram["fields"]
                .as_array()
                .unwrap()
                .iter()
                .any(|field| field["name"] == name),
            "{name}"
        );
    }
    assert!(!telegram["fields"]
        .as_array()
        .unwrap()
        .iter()
        .any(|field| field["name"] == "filter_tool_messages"));
    assert_eq!(channels["onebot"]["defaults"]["ws_port"], 6199);
    assert_eq!(channels["matrix"]["required_fields"], json!(["homeserver"]));
    for (channel, name) in [
        ("slack", "app_token"),
        ("xiaoyi", "secret_key"),
        ("voice", "twilio_auth_token"),
    ] {
        let field = channels[channel]["fields"]
            .as_array()
            .unwrap()
            .iter()
            .find(|field| field["name"] == name)
            .unwrap();
        assert_eq!(field["secret"], true);
    }

    let sip_fields = channels["sip"]["fields"].as_array().expect("sip fields");
    assert_eq!(channels["sip"]["required_fields"], json!([]));
    assert!(
        sip_fields.iter().any(|field| field["name"] == "bind_addr"),
        "SIP schema must expose bind_addr"
    );
    assert!(
        sip_fields
            .iter()
            .any(|field| field["name"] == "rtp_bind_addr"),
        "SIP schema must expose RTP bind address"
    );
    assert!(
        !sip_fields
            .iter()
            .any(|field| field["name"] == "playback_url"),
        "SIP schema must not expose fake backend playback_url"
    );

    assert!(
        !channels.contains_key("plugin"),
        "plugin channel schemas are out of scope"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn channel_secret_fields_are_masked_and_masked_updates_preserve_existing_values() {
    let app = build_app();
    let agent_id = "integ_channels_secret_masking";
    create_agent(&app.app, agent_id, "Channels secret masking").await;

    let put = request(
        &app.app,
        Method::PUT,
        "/api/config/channels/telegram",
        Some(json!({
            "enabled": true,
            "bot_token": "real-token-123",
            "bot_username": "rua_bot",
            "custom_unknown": "kept"
        })),
        Some(agent_id),
    )
    .await;
    assert_eq!(put.status(), StatusCode::OK);

    let get = request(
        &app.app,
        Method::GET,
        "/api/config/channels/telegram",
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(get.status(), StatusCode::OK);
    let body = read_json(get).await;
    assert_eq!(body["bot_token"], "***");
    assert_eq!(body["bot_username"], "rua_bot");
    assert_eq!(body["custom_unknown"], "kept");

    let masked_update = request(
        &app.app,
        Method::PUT,
        "/api/config/channels/telegram",
        Some(json!({
            "enabled": true,
            "bot_token": "***",
            "bot_username": "renamed_bot"
        })),
        Some(agent_id),
    )
    .await;
    assert_eq!(masked_update.status(), StatusCode::OK);

    let cell = app
        .state
        .agent_cells
        .section::<serde_json::Value>(agent_id, "channels")
        .read()
        .await
        .expect("channels read")
        .expect("channels value");
    assert_eq!(cell["telegram"]["bot_token"], "real-token-123");
    assert_eq!(cell["telegram"]["bot_username"], "renamed_bot");
    assert_eq!(cell["telegram"]["custom_unknown"], "kept");

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enabled_misconfigured_channel_health_reports_degraded_details() {
    let app = build_app();
    let agent_id = "integ_channels_degraded_health";
    create_agent(&app.app, agent_id, "Channels degraded health").await;

    let put = request(
        &app.app,
        Method::PUT,
        "/api/config/channels/telegram",
        Some(json!({
            "enabled": true,
            "bot_token": ""
        })),
        Some(agent_id),
    )
    .await;
    assert_eq!(put.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert!(read_json(put).await["detail"]
        .as_str()
        .unwrap()
        .contains("saved"));

    let health = request(
        &app.app,
        Method::GET,
        "/api/config/channels/telegram/health",
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(health.status(), StatusCode::OK);
    let body = read_json(health).await;
    assert_eq!(body["channel"], "telegram");
    assert_eq!(body["status"], "degraded");
    assert_eq!(body["components"]["config"], "misconfigured");
    assert_eq!(body["components"]["runtime"], "not_running");
    assert_eq!(body["components"]["transport"], "disconnected");
    assert_eq!(body["components"]["token"], "missing_credentials");
    assert!(
        body["detail"]
            .as_str()
            .expect("detail")
            .contains("missing required fields: bot_token"),
        "body was {body:?}"
    );
    assert!(
        !body["detail"]
            .as_str()
            .expect("detail")
            .contains("test-token"),
        "health detail must not leak token-like values: {body:?}"
    );

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn channel_health_and_restart_unknown_channels_return_404_detail() {
    let app = build_app();
    let agent_id = "integ_channels_health_restart_unknown";
    create_agent(&app.app, agent_id, "Channels health restart unknown").await;

    let health = request(
        &app.app,
        Method::GET,
        "/api/config/channels/nonexistent_xyz/health",
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(health.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        read_json(health).await,
        json!({"detail": "Channel 'nonexistent_xyz' not available"})
    );

    let restart = request(
        &app.app,
        Method::POST,
        "/api/config/channels/nonexistent_xyz/restart",
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(restart.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        read_json(restart).await,
        json!({"detail": "Channel 'nonexistent_xyz' not available"})
    );

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disabled_channel_health_is_reported_without_restarting_runtime() {
    let app = build_app();
    let agent_id = "integ_channels_disabled_health";
    create_agent(&app.app, agent_id, "Channels disabled health").await;

    let put = request(
        &app.app,
        Method::PUT,
        "/api/config/channels/telegram",
        Some(json!({
            "enabled": false,
            "bot_token": "test-token"
        })),
        Some(agent_id),
    )
    .await;
    assert_eq!(put.status(), StatusCode::OK);

    let health = request(
        &app.app,
        Method::GET,
        "/api/config/channels/telegram/health",
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(health.status(), StatusCode::OK);
    let health_body = read_json(health).await;
    assert_eq!(health_body["channel"], "telegram");
    assert_eq!(health_body["status"], "disabled");
    assert_eq!(health_body["detail"], "Channel is disabled.");
    assert_eq!(health_body["components"]["config"], "disabled");
    assert_eq!(health_body["components"]["runtime"], "stopped");

    let restart = request(
        &app.app,
        Method::POST,
        "/api/config/channels/telegram/restart",
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(restart.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        read_json(restart).await,
        json!({
            "detail": "Channel 'telegram' is not running. It may be disabled or not configured."
        })
    );

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_scoped_channel_health_and_restart_use_path_agent_over_conflicting_header() {
    let app = build_app();
    let header_agent = "integ_channels_health_header";
    let path_agent = "integ_channels_health_path";
    create_agent(&app.app, header_agent, "Channels health header").await;
    create_agent(&app.app, path_agent, "Channels health path").await;

    let header_put = request(
        &app.app,
        Method::PUT,
        "/api/config/channels/console",
        Some(json!({
            "enabled": false,
            "bot_prefix": "/header-disabled"
        })),
        Some(header_agent),
    )
    .await;
    assert_eq!(header_put.status(), StatusCode::OK);

    let path_console = format!("/api/agents/{path_agent}/config/channels/console");
    let path_put = request(
        &app.app,
        Method::PUT,
        &path_console,
        Some(json!({
            "enabled": true,
            "bot_prefix": "/path-enabled"
        })),
        Some(header_agent),
    )
    .await;
    assert_eq!(path_put.status(), StatusCode::OK);

    let scoped_health = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{path_agent}/config/channels/console/health"),
        None,
        Some(header_agent),
    )
    .await;
    assert_eq!(scoped_health.status(), StatusCode::OK);
    let scoped_health_body = read_json(scoped_health).await;
    assert_eq!(scoped_health_body["channel"], "console");
    assert_eq!(scoped_health_body["status"], "healthy");
    assert!(scoped_health_body["detail"].is_string());

    let scoped_restart = request(
        &app.app,
        Method::POST,
        &format!("/api/agents/{path_agent}/config/channels/console/restart"),
        None,
        Some(header_agent),
    )
    .await;
    assert_eq!(scoped_restart.status(), StatusCode::OK);
    assert_eq!(read_json(scoped_restart).await["status"], "restarted");

    let header_after = request(
        &app.app,
        Method::GET,
        "/api/config/channels/console",
        None,
        Some(header_agent),
    )
    .await;
    assert_eq!(
        read_json(header_after).await,
        json!({
            "enabled": false,
            "bot_prefix": "/header-disabled"
        }),
        "agent-scoped health/restart must not mutate the conflicting header agent"
    );

    let path_after = request(&app.app, Method::GET, &path_console, None, None).await;
    assert_eq!(
        read_json(path_after).await,
        json!({
            "enabled": true,
            "bot_prefix": "/path-enabled"
        })
    );

    delete_agent(&app.app, header_agent).await;
    delete_agent(&app.app, path_agent).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn root_channels_types_and_full_payload_roundtrip() {
    let app = build_app();
    let agent_id = "integ_channels_root_contract";
    create_agent(&app.app, agent_id, "Channels root contract").await;

    let types = request(
        &app.app,
        Method::GET,
        "/api/config/channels/types",
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(types.status(), StatusCode::OK);
    assert_builtin_channel_types(&read_json(types).await);

    let before = request(
        &app.app,
        Method::GET,
        "/api/config/channels",
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(before.status(), StatusCode::OK);
    let before_body = read_json(before).await;
    assert_eq!(before_body["console"]["enabled"], true);
    assert_eq!(before_body["console"]["isBuiltin"], true);

    let payload = channels_payload(false);
    let put = request(
        &app.app,
        Method::PUT,
        "/api/config/channels",
        Some(payload),
        Some(agent_id),
    )
    .await;
    assert_eq!(put.status(), StatusCode::OK);
    let put_body = read_json(put).await;
    assert_eq!(put_body["console"]["enabled"], false);
    assert_eq!(put_body["console"]["isBuiltin"], true);
    assert_eq!(put_body["dingtalk"]["isBuiltin"], true);
    assert_eq!(put_body["telegram"]["enabled"], false);
    assert_eq!(put_body["telegram"]["isBuiltin"], true);

    let console = request(
        &app.app,
        Method::GET,
        "/api/config/channels/console",
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(console.status(), StatusCode::OK);
    let console_body = read_json(console).await;
    assert_eq!(
        console_body,
        json!({"enabled": false, "bot_prefix": "@rua"})
    );

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn malformed_channel_payloads_are_rejected_without_mutating_config() {
    let app = build_app();
    let agent_id = "integ_channels_invalid_payload";
    create_agent(&app.app, agent_id, "Channels invalid payload").await;

    let seed = request(
        &app.app,
        Method::PUT,
        "/api/config/channels",
        Some(channels_payload(true)),
        Some(agent_id),
    )
    .await;
    assert_eq!(seed.status(), StatusCode::OK);

    let invalid_full = request(
        &app.app,
        Method::PUT,
        "/api/config/channels",
        Some(json!({
            "console": {
                "enabled": "not-a-bool-and-not-coercible"
            }
        })),
        Some(agent_id),
    )
    .await;
    assert_eq!(invalid_full.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let invalid_full_body = read_json(invalid_full).await;
    assert_eq!(
        invalid_full_body["detail"][0]["loc"],
        json!(["body", "console", "enabled"])
    );

    let after_full = request(
        &app.app,
        Method::GET,
        "/api/config/channels/console",
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(after_full.status(), StatusCode::OK);
    assert_eq!(
        read_json(after_full).await,
        json!({"enabled": true, "bot_prefix": "@rua"})
    );

    let invalid_single = request(
        &app.app,
        Method::PUT,
        "/api/config/channels/console",
        Some(json!("not-an-object")),
        Some(agent_id),
    )
    .await;
    assert_eq!(invalid_single.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let invalid_single_body = read_json(invalid_single).await;
    assert_eq!(invalid_single_body["detail"][0]["loc"], json!(["body"]));

    let after_single = request(
        &app.app,
        Method::GET,
        "/api/config/channels/console",
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(after_single.status(), StatusCode::OK);
    assert_eq!(
        read_json(after_single).await,
        json!({"enabled": true, "bot_prefix": "@rua"})
    );

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn builtin_single_channel_get_backfills_default_config_for_fresh_agents() {
    let app = build_app();
    let agent_id = "integ_channels_builtin_single_default";
    create_agent(&app.app, agent_id, "Channels builtin single default").await;

    let console = request(
        &app.app,
        Method::GET,
        "/api/config/channels/console",
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(console.status(), StatusCode::OK);
    assert_eq!(read_json(console).await, json!({"enabled": true}));

    let telegram = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{agent_id}/config/channels/telegram"),
        None,
        None,
    )
    .await;
    assert_eq!(telegram.status(), StatusCode::OK);
    assert_eq!(read_json(telegram).await, json!({"enabled": false}));

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn single_channel_config_persists_across_restart_reload() {
    let app = build_app();
    let agent_id = "integ_channels_restart_persist";
    create_agent(&app.app, agent_id, "Channels restart persist").await;

    let put = request(
        &app.app,
        Method::PUT,
        "/api/config/channels/console",
        Some(json!({
            "enabled": true,
            "bot_prefix": "/restart-persist"
        })),
        Some(agent_id),
    )
    .await;
    assert_eq!(put.status(), StatusCode::OK);

    let restart = request(
        &app.app,
        Method::POST,
        "/api/config/channels/console/restart",
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(restart.status(), StatusCode::OK);
    assert_eq!(read_json(restart).await["status"], "restarted");

    let after = request(
        &app.app,
        Method::GET,
        "/api/config/channels/console",
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(after.status(), StatusCode::OK);
    assert_eq!(
        read_json(after).await,
        json!({
            "enabled": true,
            "bot_prefix": "/restart-persist"
        })
    );

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn root_and_agent_scoped_channel_types_share_the_same_canonical_list() {
    let app = build_app();
    let agent_id = "integ_channels_types_scoped";
    create_agent(&app.app, agent_id, "Channels types scoped").await;

    let root = request(
        &app.app,
        Method::GET,
        "/api/config/channels/types",
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(root.status(), StatusCode::OK);
    let root_body = read_json(root).await;
    assert_builtin_channel_types(&root_body);

    let scoped = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{agent_id}/config/channels/types"),
        None,
        None,
    )
    .await;
    assert_eq!(scoped.status(), StatusCode::OK);
    let scoped_body = read_json(scoped).await;

    assert_eq!(scoped_body, root_body);

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn legacy_weixin_channel_alias_roundtrips_without_public_type_advertising() {
    let app = build_app();
    let agent_id = "integ_channels_weixin_alias";
    create_agent(&app.app, agent_id, "Channels weixin alias").await;

    let types = request(
        &app.app,
        Method::GET,
        "/api/config/channels/types",
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(types.status(), StatusCode::OK);
    let types_body = read_json(types).await;
    assert_builtin_channel_types(&types_body);
    assert!(
        !types_body
            .as_array()
            .expect("channel types array")
            .iter()
            .any(|value| value == "weixin"),
        "legacy alias must not be advertised as a canonical public channel type"
    );

    let payload = json!({
        "enabled": false,
        "baseurl": "https://legacy.weixin.example"
    });
    let put = request(
        &app.app,
        Method::PUT,
        "/api/config/channels/weixin",
        Some(payload.clone()),
        Some(agent_id),
    )
    .await;
    assert_eq!(put.status(), StatusCode::OK);
    assert_eq!(read_json(put).await, payload);

    let list = request(
        &app.app,
        Method::GET,
        "/api/config/channels",
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(list.status(), StatusCode::OK);
    let list_body = read_json(list).await;
    assert_eq!(list_body["wechat"]["isBuiltin"], true);
    assert_eq!(list_body["weixin"]["isBuiltin"], true);
    assert_eq!(
        list_body["weixin"]["baseurl"],
        "https://legacy.weixin.example"
    );

    let get = request(
        &app.app,
        Method::GET,
        "/api/config/channels/weixin",
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(get.status(), StatusCode::OK);
    assert_eq!(read_json(get).await, payload);

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn custom_channel_config_roundtrips_without_advertising_or_fake_runtime() {
    let app = build_app();
    let agent_id = "integ_channels_custom_config";
    let channel = "test_echo";
    create_agent(&app.app, agent_id, "Channels custom config").await;

    let types = request(
        &app.app,
        Method::GET,
        "/api/config/channels/types",
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(types.status(), StatusCode::OK);
    let types_body = read_json(types).await;
    assert!(
        !types_body
            .as_array()
            .expect("channel types")
            .iter()
            .any(|value| value == channel),
        "Rust should not advertise QwenPaw custom-channel ids without a runtime registration surface"
    );

    let disabled_payload = json!({
        "enabled": false,
        "callback_url": "http://127.0.0.1:9/callback",
        "custom_field": "roundtrip"
    });
    let put_disabled = request(
        &app.app,
        Method::PUT,
        &format!("/api/config/channels/{channel}"),
        Some(disabled_payload.clone()),
        Some(agent_id),
    )
    .await;
    assert_eq!(put_disabled.status(), StatusCode::OK);
    assert_eq!(read_json(put_disabled).await, disabled_payload);

    let list = request(
        &app.app,
        Method::GET,
        "/api/config/channels",
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(list.status(), StatusCode::OK);
    let list_body = read_json(list).await;
    assert_eq!(list_body[channel]["enabled"], false);
    assert_eq!(
        list_body[channel]["callback_url"],
        "http://127.0.0.1:9/callback"
    );
    assert_eq!(list_body[channel]["custom_field"], "roundtrip");
    assert_eq!(
        list_body[channel]["isBuiltin"], false,
        "custom channel configs may round-trip, but are not builtin channel types"
    );

    let disabled_health = request(
        &app.app,
        Method::GET,
        &format!("/api/config/channels/{channel}/health"),
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(disabled_health.status(), StatusCode::OK);
    let disabled_body = read_json(disabled_health).await;
    assert_eq!(disabled_body["channel"], channel);
    assert_eq!(disabled_body["status"], "disabled");
    assert_eq!(disabled_body["detail"], "Channel is disabled.");
    assert_eq!(disabled_body["components"]["config"], "disabled");

    let enabled_payload = json!({
        "enabled": true,
        "callback_url": "http://127.0.0.1:9/callback"
    });
    let put_enabled = request(
        &app.app,
        Method::PUT,
        &format!("/api/config/channels/{channel}"),
        Some(enabled_payload),
        Some(agent_id),
    )
    .await;
    assert_eq!(put_enabled.status(), StatusCode::OK);

    let enabled_health = request(
        &app.app,
        Method::GET,
        &format!("/api/config/channels/{channel}/health"),
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(enabled_health.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        read_json(enabled_health).await,
        json!({
            "detail": "Channel 'test_echo' is not running. It may be disabled or not configured."
        })
    );

    let restart = request(
        &app.app,
        Method::POST,
        &format!("/api/config/channels/{channel}/restart"),
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(restart.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        read_json(restart).await,
        json!({
            "detail": "Channel 'test_echo' is not running. It may be disabled or not configured."
        })
    );

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn qrcode_registry_matches_rust_supported_login_channels() {
    let app = build_app();

    for channel in ["dingtalk", "feishu", "qq", "wecom", "wechat", "weixin"] {
        assert!(
            app.state.qrcode_registry.get(channel).await.is_some(),
            "{channel} should have a QR-code login handler"
        );
    }

    for channel in ["console", "telegram", "yuanbao"] {
        assert!(
            app.state.qrcode_registry.get(channel).await.is_none(),
            "{channel} should not be advertised as QR-code login capable"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_scoped_channels_full_and_single_channel_roundtrip_are_isolated() {
    let app = build_app();
    let root_agent = "integ_channels_header_isolated";
    let scoped_agent = "integ_channels_scoped_contract";
    create_agent(&app.app, root_agent, "Channels header isolated").await;
    create_agent(&app.app, scoped_agent, "Channels scoped contract").await;

    let root_put = request(
        &app.app,
        Method::PUT,
        "/api/config/channels",
        Some(channels_payload(true)),
        Some(root_agent),
    )
    .await;
    assert_eq!(root_put.status(), StatusCode::OK);

    let scoped_base = format!("/api/agents/{scoped_agent}/config/channels");
    let scoped_put = request(
        &app.app,
        Method::PUT,
        &scoped_base,
        Some(channels_payload(false)),
        None,
    )
    .await;
    assert_eq!(scoped_put.status(), StatusCode::OK);
    assert_eq!(read_json(scoped_put).await["console"]["enabled"], false);

    let scoped_console_path = format!("{scoped_base}/console");
    let single_put = request(
        &app.app,
        Method::PUT,
        &scoped_console_path,
        Some(json!({
            "enabled": true,
            "bot_prefix": "/scoped"
        })),
        None,
    )
    .await;
    assert_eq!(single_put.status(), StatusCode::OK);
    assert_eq!(
        read_json(single_put).await,
        json!({"enabled": true, "bot_prefix": "/scoped"})
    );

    let scoped_console = request(&app.app, Method::GET, &scoped_console_path, None, None).await;
    assert_eq!(scoped_console.status(), StatusCode::OK);
    assert_eq!(
        read_json(scoped_console).await,
        json!({"enabled": true, "bot_prefix": "/scoped"})
    );

    let root_console = request(
        &app.app,
        Method::GET,
        "/api/config/channels/console",
        None,
        Some(root_agent),
    )
    .await;
    assert_eq!(root_console.status(), StatusCode::OK);
    assert_eq!(
        read_json(root_console).await,
        json!({"enabled": true, "bot_prefix": "@rua"})
    );

    delete_agent(&app.app, root_agent).await;
    delete_agent(&app.app, scoped_agent).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_channel_and_unsupported_qrcode_return_fastapi_detail_errors() {
    let app = build_app();
    let agent_id = "integ_channels_error_contract";
    create_agent(&app.app, agent_id, "Channels error contract").await;

    let missing = request(
        &app.app,
        Method::GET,
        "/api/config/channels/not-a-channel",
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        read_json(missing).await,
        json!({"detail": "Channel 'not-a-channel' not found"})
    );

    let qrcode = request(
        &app.app,
        Method::GET,
        "/api/config/channels/console/qrcode",
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(qrcode.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        read_json(qrcode).await,
        json!({"detail": "QR code not supported for channel: console"})
    );

    let scoped_qrcode = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{agent_id}/config/channels/console/qrcode/status?token=x"),
        None,
        None,
    )
    .await;
    assert_eq!(scoped_qrcode.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        read_json(scoped_qrcode).await,
        json!({"detail": "QR code not supported for channel: console"})
    );

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn synchronized_credentials_are_masked_and_preserved_by_schema_form_save() {
    let app = build_app();
    let agent_id = "integ_channels_schema_credentials";
    create_agent(&app.app, agent_id, "Schema credentials").await;
    for (channel, key) in [("slack", "app_token"), ("xiaoyi", "secret_key")] {
        let url = format!("/api/agents/{agent_id}/config/channels/{channel}");
        let response = request(
            &app.app,
            Method::PUT,
            &url,
            Some(
                json!({"enabled": false, key: "private-token", "vendor_extension": {"kept": true}}),
            ),
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(read_json(response).await[key], "***");
        let response = request(&app.app, Method::PUT, &url, Some(json!({key:"***"})), None).await;
        assert_eq!(response.status(), StatusCode::OK);
        let response = request(&app.app, Method::GET, &url, None, None).await;
        let body = read_json(response).await;
        assert_eq!(body[key], "***");
        assert_eq!(body["vendor_extension"]["kept"], true);
    }
    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn optional_tokens_and_matrix_login_alternatives_do_not_mark_config_misconfigured() {
    let app = build_app();
    let agent_id = "integ_channels_optional_tokens";
    create_agent(&app.app, agent_id, "Channel optional tokens").await;

    for (channel, config) in [
        (
            "matrix",
            json!({
                "enabled": true, "homeserver": "http://127.0.0.1:1",
                "user_id": "@bot:example.test", "password": "saved-password", "access_token": ""
            }),
        ),
        (
            "matrix",
            json!({
                "enabled": true, "homeserver": "http://127.0.0.1:1",
                "user_id": "", "password": "", "access_token": "saved-token"
            }),
        ),
        (
            "onebot",
            json!({
                "enabled": true, "ws_host": "127.0.0.1", "access_token": ""
            }),
        ),
    ] {
        let put = request(
            &app.app,
            Method::PUT,
            &format!("/api/agents/{agent_id}/config/channels/{channel}"),
            Some(config),
            None,
        )
        .await;
        assert_eq!(put.status(), StatusCode::OK);
        let health = request(
            &app.app,
            Method::GET,
            &format!("/api/agents/{agent_id}/config/channels/{channel}/health"),
            None,
            None,
        )
        .await;
        assert_eq!(health.status(), StatusCode::OK);
        let body = read_json(health).await;
        assert_eq!(body["components"]["config"], "enabled", "{body:?}");
        assert_ne!(
            body["components"]["token"], "missing_credentials",
            "{body:?}"
        );
        assert!(
            !body["detail"]
                .as_str()
                .unwrap()
                .contains("missing required fields"),
            "{body:?}"
        );
    }

    delete_agent(&app.app, agent_id).await;
}
