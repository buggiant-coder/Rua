use rua_config::channel_catalog::*;
use rua_config::{
    IMessageConfig, MatrixConfig, MqttConfig, OneBotConfig, TelegramConfig, WorkspaceChannelsConfig,
};
use serde_json::{json, Value};

#[test]
fn boolean_schema_fields_reject_invalid_values_without_touching_extensions() {
    for channel in channel_types() {
        for (name, value) in channel_defaults(&channel).as_object().unwrap() {
            if !value.is_boolean() {
                continue;
            }
            for invalid in [json!(null), json!("false"), json!(0), json!({}), json!([])] {
                assert!(
                    resolve_channel_config(&channel, &json!({name:invalid})).is_err(),
                    "{channel}.{name}"
                );
            }
            for valid in [false, true] {
                validate_channel_fields(&channel, &json!({name:valid})).unwrap();
            }
        }
    }
    let saved = json!({"show_typing":null,"vendor_option":null,"password":"***"});
    let resolved = resolve_channel_config("telegram", &saved).unwrap();
    assert_eq!(resolved["show_typing"], Value::Null);
    assert_eq!(resolved["vendor_option"], Value::Null);
    assert_eq!(resolved["password"], "***");
    assert_eq!(
        resolve_channel_config("custom", &json!({"custom_flag":null})).unwrap()["custom_flag"],
        Value::Null
    );
}

#[test]
fn catalog_tracks_every_upstream_channel_and_preserves_supported_defaults() {
    let upstream: Value = serde_json::from_str(QWENPAW_CHANNELS_JSON).unwrap();
    let channels = upstream["channels"].as_object().unwrap();
    let mut types = channel_types();
    types.sort();
    assert_eq!(types, channels.keys().cloned().collect::<Vec<_>>());
    assert_eq!(channels.len(), 18);
    assert_eq!(channel_defaults("onebot")["ws_port"], 6199);
    assert_eq!(channel_defaults("telegram")["show_tool_calls"], true);
    assert_eq!(channel_defaults("telegram")["tool_result_max_length"], 500);
    assert_eq!(channel_defaults("slack")["streaming_enabled"], false);
    assert_eq!(channel_defaults("imessage")["poll_sec"], 1.0);
    assert_eq!(channel_defaults("matrix")["device_name"], "rua-bot");
    assert!(channel_fields("sip")
        .iter()
        .any(|field| field["name"] == "media_dir"));
    assert_eq!(channel_defaults("weixin"), channel_defaults("wechat"));
    assert!(
        upstream["upstream"]["source_sha256"]
            .as_object()
            .unwrap()
            .len()
            > 18
    );
}

#[test]
fn canonical_fields_roundtrip_without_losing_credentials_or_extensions() {
    let saved = json!({"host": "private.mqtt", "password": "secret", "qos": 1, "future": {"x": 3}});
    let mqtt: MqttConfig =
        serde_json::from_value(resolve_channel_config("mqtt", &saved).unwrap()).unwrap();
    assert_eq!(mqtt.broker_url, "private.mqtt");
    assert_eq!(mqtt.common.extra["future"], json!({"x": 3}));
    let serialized = serde_json::to_value(mqtt).unwrap();
    assert_eq!(serialized["host"], "private.mqtt");
    assert_eq!(serialized["password"], "secret");
    assert_eq!(serialized["qos"], 1);
    let matrix: MatrixConfig = serde_json::from_value(
        resolve_channel_config(
            "matrix",
            &json!({
                "homeserver": "https://matrix.local/", "encryption": true
            }),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(matrix.homeserver_url, "https://matrix.local");
    assert!(matrix.encryption_enabled);
    let serialized = serde_json::to_value(matrix).unwrap();
    assert_eq!(serialized["encryption"], true);
}

#[test]
fn independent_display_controls_are_typed() {
    let resolved = resolve_channel_config(
        "telegram",
        &json!({
            "show_thinking": false, "show_tool_results": false, "show_tool_calls": true
        }),
    )
    .unwrap();
    assert_eq!(resolved["show_thinking"], false);
    assert_eq!(resolved["show_tool_calls"], true);
    assert_eq!(resolved["show_tool_results"], false);
    let config: TelegramConfig = serde_json::from_value(resolved).unwrap();
    assert!(config.common.show_tool_calls);
    assert!(!config.common.show_tool_results);
    assert!(!config.common.show_thinking);
    assert!(!config.common.extra.contains_key("show_tool_calls"));
}

#[test]
fn upstream_defaults_deserialize_and_workspace_keeps_extension_channels() {
    let _: OneBotConfig = serde_json::from_value(channel_defaults("onebot")).unwrap();
    let _: IMessageConfig = serde_json::from_value(channel_defaults("imessage")).unwrap();
    let _: MqttConfig = serde_json::from_value(channel_defaults("mqtt")).unwrap();
    let saved = json!({"custom_sms": {"enabled": true, "credential": "retained"}, "voice": {"language": "zh-CN"}});
    let workspace: WorkspaceChannelsConfig = serde_json::from_value(saved.clone()).unwrap();
    let serialized = serde_json::to_value(workspace).unwrap();
    assert_eq!(serialized["custom_sms"], saved["custom_sms"]);
    assert_eq!(serialized["voice"], saved["voice"]);
}

#[test]
fn upstream_unused_or_webhook_only_options_are_preserved_without_ui_controls() {
    for (channel, absent) in [
        ("slack", "signing_secret"),
        ("slack", "thread_follow_without_mention"),
        ("feishu", "encrypt_key"),
        ("xiaoyi", "task_timeout_ms"),
        ("sip", "livekit_sip_trunk_id"),
    ] {
        assert!(!channel_fields(channel)
            .iter()
            .any(|field| field["name"] == absent));
        // Preserve the original value for future support / export.
        assert_eq!(
            resolve_channel_config(channel, &json!({absent: "saved"})).unwrap()[absent],
            "saved"
        );
    }
}

#[test]
fn synchronized_channel_credentials_are_redacted_and_masked_updates_preserve_them() {
    let secrets = json!({"app_token":"app-secret","secret_key":"xiaoyi-secret","signing_secret":"slack-secret","nested":{"twilio_auth_token":"voice-secret"}});
    let masked = rua_config::redact_secrets(&secrets);
    assert_eq!(masked["app_token"], "***");
    assert_eq!(masked["secret_key"], "***");
    assert_eq!(masked["signing_secret"], "***");
    assert_eq!(masked["nested"]["twilio_auth_token"], "***");
    assert_eq!(
        rua_config::merge_with_redaction_safety(
            secrets.as_object().unwrap(),
            masked.as_object().unwrap()
        ),
        *secrets.as_object().unwrap()
    );
    assert!(!channel_fields("dingtalk")
        .iter()
        .find(|field| field["name"] == "card_template_key")
        .unwrap()["secret"]
        .as_bool()
        .unwrap());
}

#[test]
fn realtime_sip_mode_is_explicit_and_matrix_options_remain_available() {
    assert_eq!(channel_defaults("sip")["sip_mode"], "dev");
    assert_eq!(channel_defaults("sip")["livekit_output_sample_rate"], 24000);
    assert_eq!(channel_defaults("matrix")["history_limit"], 50);
    assert_eq!(channel_defaults("matrix")["vision_enabled"], true);
    assert!(resolve_channel_config("sip", &json!({"bind_addr":"127.0.0.1:5060"})).is_err());
    let native = resolve_channel_config(
        "sip",
        &json!({"sip_mode":"native", "bind_addr":"127.0.0.1:5060"}),
    )
    .unwrap();
    assert_eq!(native["sip_mode"], "native");
    let livekit = resolve_channel_config(
        "sip",
        &json!({"sip_mode":"livekit","livekit_api_secret":"key"}),
    )
    .unwrap();
    assert_eq!(
        rua_config::redact_secrets(&livekit)["livekit_api_secret"],
        "***"
    );
    assert!(channel_fields("matrix")
        .iter()
        .any(|field| field["name"] == "encryption"));
}

#[test]
fn ported_protocol_options_are_advertised_in_channel_forms() {
    for (channel, field) in [
        ("slack", "proxy"),
        ("mattermost", "thread_follow_without_mention"),
        ("qq", "ack_message"),
        ("qq", "markdown_enabled"),
        ("qq", "max_reconnect_attempts"),
        ("wecom", "welcome_text"),
        ("wecom", "max_reconnect_attempts"),
        ("wechat", "message_merge_enabled"),
        ("wechat", "message_merge_delay_ms"),
        ("onebot", "media_base64"),
        ("onebot", "media_download_max_mb"),
        ("dingtalk", "card_auto_layout"),
        ("dingtalk", "at_sender_on_reply"),
        ("dingtalk", "endpoint"),
        ("yuanbao", "accept_bot_messages"),
        ("voice", "phone_number_sid"),
        ("sip", "sip_mode"),
        ("matrix", "history_limit"),
    ] {
        assert!(
            channel_fields(channel).iter().any(|f| f["name"] == field),
            "{channel}.{field}"
        );
    }
}
