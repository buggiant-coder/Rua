use std::sync::Arc;

use rua_channel::core::ChannelManager;
use rua_config::DingTalkConfig;
use rua_workspace::services::ChannelsServiceAdapter;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn config_injection_registers_with_ws_and_bot_config() {
    let manager = Arc::new(ChannelManager::new());
    let config = DingTalkConfig {
        enabled: true,
        client_id: "test-client-id".into(),
        client_secret: "test-secret".into(),
        bot_id: "bot-123".into(),
        bot_name: "rua".into(),
        enable_ws: true,
        ..Default::default()
    };

    let adapter = ChannelsServiceAdapter::new(Arc::clone(&manager)).with_dingtalk_config(config);

    adapter.register_configured_channels().await.unwrap();

    let channels = manager.channel_ids().await;
    assert!(
        channels.iter().any(|id| id == "dingtalk"),
        "DingTalk channel should be registered; got: {channels:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn empty_client_id_skips_registration() {
    let manager = Arc::new(ChannelManager::new());
    let config = DingTalkConfig {
        enabled: true,
        client_id: String::new(),
        client_secret: "some-secret".into(),
        bot_id: "bot-123".into(),
        bot_name: "rua".into(),
        enable_ws: true,
        ..Default::default()
    };

    let adapter = ChannelsServiceAdapter::new(Arc::clone(&manager)).with_dingtalk_config(config);

    assert!(adapter.register_configured_channels().await.is_err());

    let channels = manager.channel_ids().await;
    assert!(
        !channels.iter().any(|id| id == "dingtalk"),
        "DingTalk channel should NOT be registered with empty client_id"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn empty_client_secret_skips_registration() {
    let manager = Arc::new(ChannelManager::new());
    let config = DingTalkConfig {
        enabled: true,
        client_id: "some-id".into(),
        client_secret: String::new(),
        bot_id: "bot-123".into(),
        bot_name: "rua".into(),
        enable_ws: true,
        ..Default::default()
    };

    let adapter = ChannelsServiceAdapter::new(Arc::clone(&manager)).with_dingtalk_config(config);

    assert!(adapter.register_configured_channels().await.is_err());

    let channels = manager.channel_ids().await;
    assert!(
        !channels.iter().any(|id| id == "dingtalk"),
        "DingTalk channel should NOT be registered with empty client_secret"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_config_skips_registration() {
    let manager = Arc::new(ChannelManager::new());

    let adapter = ChannelsServiceAdapter::new(Arc::clone(&manager));

    adapter.register_configured_channels().await.unwrap();

    let channels = manager.channel_ids().await;
    assert!(
        !channels.iter().any(|id| id == "dingtalk"),
        "DingTalk channel should NOT be registered without config"
    );
}

#[test]
fn dingtalk_config_deserializes_from_json() {
    let json = serde_json::json!({
        "client_id": "ding-abc123",
        "client_secret": "sec-xyz",
        "bot_id": "bot-001",
        "bot_name": "TestBot",
        "enable_ws": false
    });

    let config: DingTalkConfig = serde_json::from_value(json).unwrap();
    assert_eq!(config.client_id, "ding-abc123");
    assert_eq!(config.client_secret, "sec-xyz");
    assert_eq!(config.bot_id, "bot-001");
    assert_eq!(config.bot_name, "TestBot");
    assert!(!config.enable_ws);
}

#[test]
fn dingtalk_config_defaults_enable_ws_true() {
    let json = serde_json::json!({
        "client_id": "id",
        "client_secret": "secret"
    });

    let config: DingTalkConfig = serde_json::from_value(json).unwrap();
    assert!(config.enable_ws, "enable_ws should default to true");
}

#[test]
fn workspace_config_extracts_dingtalk_from_config_json() {
    let config_json = serde_json::json!({
        "channels": {
            "dingtalk": {
                "client_id": "ding-test",
                "client_secret": "secret-test",
                "bot_id": "bot-test",
                "bot_name": "rua",
                "enable_ws": true,
                "enabled": true,
                "bot_prefix": ""
            }
        }
    });

    let dt_val = config_json
        .get("channels")
        .unwrap()
        .get("dingtalk")
        .unwrap();
    let config: DingTalkConfig = serde_json::from_value(dt_val.clone()).unwrap();
    assert_eq!(config.client_id, "ding-test");
    assert_eq!(config.client_secret, "secret-test");
    assert_eq!(config.bot_name, "rua");
}
