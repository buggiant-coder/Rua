use std::sync::Arc;

use rua_channel::core::ChannelManager;
use rua_config::{ChannelCommonConfig, XiaoyiConfig};
use rua_workspace::services::ChannelsServiceAdapter;

fn xiaoyi_config(access_key: &str, secret_key: &str, agent_id: &str) -> XiaoyiConfig {
    XiaoyiConfig {
        enabled: true,
        access_key: access_key.into(),
        secret_key: secret_key.into(),
        agent_id: agent_id.into(),
        ws_url: String::new(),
        backup_ws_url: String::new(),
        media_dir: String::new(),
        common: ChannelCommonConfig::default(),
    }
}

async fn registered_channels(config: XiaoyiConfig) -> Vec<String> {
    let invalid = config.enabled
        && (config.access_key.is_empty()
            || config.secret_key.is_empty()
            || config.agent_id.is_empty());
    let manager = Arc::new(ChannelManager::new());
    let adapter = ChannelsServiceAdapter::new(Arc::clone(&manager)).with_xiaoyi_config(config);

    let result = adapter.register_configured_channels().await;
    if invalid {
        assert!(result.is_err());
    } else {
        result.unwrap();
    }
    manager.channel_ids().await
}

#[tokio::test]
async fn complete_xiaoyi_config_registers_channel() {
    let channels = registered_channels(xiaoyi_config("ak-1", "sk-1", "agent-1")).await;

    assert!(
        channels.iter().any(|id| id == "xiaoyi"),
        "Xiaoyi should be registered; got {channels:?}"
    );
}

#[tokio::test]
async fn disabled_xiaoyi_config_skips_registration() {
    let mut config = xiaoyi_config("ak-1", "sk-1", "agent-1");
    config.enabled = false;
    let channels = registered_channels(config).await;

    assert!(
        !channels.iter().any(|id| id == "xiaoyi"),
        "disabled Xiaoyi config should not register; got {channels:?}"
    );
}

#[tokio::test]
async fn missing_xiaoyi_access_key_skips_registration() {
    let channels = registered_channels(xiaoyi_config("", "sk-1", "agent-1")).await;

    assert!(
        !channels.iter().any(|id| id == "xiaoyi"),
        "Xiaoyi should not register without access_key; got {channels:?}"
    );
}

#[tokio::test]
async fn missing_xiaoyi_secret_key_skips_registration() {
    let channels = registered_channels(xiaoyi_config("ak-1", "", "agent-1")).await;

    assert!(
        !channels.iter().any(|id| id == "xiaoyi"),
        "Xiaoyi should not register without secret_key; got {channels:?}"
    );
}

#[tokio::test]
async fn missing_xiaoyi_agent_id_skips_registration() {
    let channels = registered_channels(xiaoyi_config("ak-1", "sk-1", "")).await;

    assert!(
        !channels.iter().any(|id| id == "xiaoyi"),
        "Xiaoyi should not register without agent_id; got {channels:?}"
    );
}
