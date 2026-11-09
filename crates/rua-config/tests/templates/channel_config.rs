use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use rua_config::{ChannelConfigStore, MatrixConfig, MqttConfig, XiaoyiConfig};

#[test]
fn channel_booleans_default_only_when_absent() {
    use rua_config::{BaseChannelConfig, ChannelCommonConfig, DingTalkConfig};
    let empty = json!({});
    let common: ChannelCommonConfig = serde_json::from_value(empty.clone()).unwrap();
    assert!(!common.require_mention);
    assert!(!common.access_control_dm);
    assert!(!common.access_control_group);
    assert!(common.show_tool_details);
    let dingtalk: DingTalkConfig = serde_json::from_value(empty.clone()).unwrap();
    assert!(dingtalk.enable_ws);
    assert!(!dingtalk.enabled);
    assert!(!dingtalk.streaming_enabled);
    assert!(!dingtalk.ai_card_enabled);
    let base: BaseChannelConfig = serde_json::from_value(empty).unwrap();
    assert!(!base.enabled);
    for field in [
        "enabled",
        "enable_ws",
        "streaming_enabled",
        "ai_card_enabled",
        "require_mention",
        "access_control_dm",
        "access_control_group",
        "show_tool_details",
        "show_thinking",
        "show_tool_calls",
        "show_tool_results",
    ] {
        for invalid in [json!(null), json!("false"), json!(0), json!({}), json!([])] {
            assert!(
                serde_json::from_value::<DingTalkConfig>(json!({field:invalid})).is_err(),
                "{field}"
            );
        }
        for value in [false, true] {
            let parsed: DingTalkConfig = serde_json::from_value(json!({field:value})).unwrap();
            assert_eq!(serde_json::to_value(parsed).unwrap()[field], value);
        }
    }
    for field in ["enabled", "require_mention"] {
        assert!(serde_json::from_value::<BaseChannelConfig>(json!({field:null})).is_err());
    }
}

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(label: &str) -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("rua-{label}-{}-{nanos}-{n}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn fresh_store() -> (TempDir, ChannelConfigStore) {
    let dir = TempDir::new("channel-config");
    let store = ChannelConfigStore::new(dir.path().to_path_buf());
    (dir, store)
}

fn read_config(dir: &Path) -> Value {
    let bytes = std::fs::read(dir.join("config.json")).expect("read config.json");
    serde_json::from_slice(&bytes).expect("parse config.json")
}

#[test]
fn empty_state_returns_empty_map() {
    let (_dir, store) = fresh_store();
    let channels = store.list_channels().expect("list");
    assert!(channels.is_empty());
}

#[test]
fn set_then_get_round_trips() {
    let (dir, store) = fresh_store();
    let cfg = json!({"enabled": true, "bot_token": "tok-abcdefghijkl"});
    store.set_channel("telegram", cfg.clone()).expect("set");

    let fetched = store.get_channel("telegram").expect("get");
    assert_eq!(fetched, Some(cfg.clone()));

    let listed = store.list_channels().expect("list");
    assert_eq!(listed.get("telegram"), Some(&cfg));

    let raw = read_config(dir.path());
    assert_eq!(raw["channels"]["telegram"], cfg);
}

#[test]
fn delete_removes_only_the_named_channel() {
    let (_dir, store) = fresh_store();
    store
        .set_channel("discord", json!({"enabled": true}))
        .unwrap();
    store
        .set_channel("telegram", json!({"enabled": false}))
        .unwrap();

    assert!(store.delete_channel("discord").unwrap());
    assert_eq!(store.get_channel("discord").unwrap(), None);
    assert_eq!(
        store.get_channel("telegram").unwrap(),
        Some(json!({"enabled": false}))
    );

    assert!(!store.delete_channel("missing").unwrap());
}

#[test]
fn replace_all_overwrites_channels_only() {
    let (dir, store) = fresh_store();

    let path = dir.path().join("config.json");
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&json!({
            "channels": {"discord": {"enabled": true}},
            "user_timezone": "Asia/Shanghai"
        }))
        .unwrap(),
    )
    .unwrap();

    let mut new_channels = serde_json::Map::new();
    new_channels.insert("dingtalk".into(), json!({"enabled": true}));
    store.replace_all(new_channels).unwrap();

    let raw = read_config(dir.path());
    assert!(raw["channels"].get("discord").is_none());
    assert_eq!(raw["channels"]["dingtalk"], json!({"enabled": true}));

    assert_eq!(raw["user_timezone"], json!("Asia/Shanghai"));
}

#[test]
fn masked_accessors_redact_secret_fields() {
    let (_dir, store) = fresh_store();
    store
        .set_channel(
            "feishu",
            json!({
                "enabled": true,
                "app_secret": "fs-supersecretvalue-1234",
                "webhook_url": "https://open.feishu.cn/abc/xyz"
            }),
        )
        .unwrap();

    let masked = store
        .get_channel_masked("feishu")
        .unwrap()
        .expect("present");
    let secret = masked["app_secret"].as_str().unwrap();
    assert_ne!(secret, "fs-supersecretvalue-1234");
    assert!(secret.contains("****"), "expected mask, got {secret}");

    assert_eq!(
        masked["webhook_url"],
        json!("https://open.feishu.cn/abc/xyz")
    );

    let raw = store.get_channel("feishu").unwrap().unwrap();
    assert_eq!(raw["app_secret"], json!("fs-supersecretvalue-1234"));
}

#[test]
fn atomic_write_leaves_no_temp_sibling() {
    let (dir, store) = fresh_store();
    store
        .set_channel("console", json!({"enabled": true}))
        .unwrap();

    let entries: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert!(
        entries.iter().any(|n| n == "config.json"),
        "config.json missing"
    );
    for name in &entries {
        assert!(
            !name.starts_with(".config.json.tmp"),
            "leftover temp file: {name}"
        );
    }
}

#[test]
fn xiaoyi_config_uses_canonical_credential_fields() {
    let config: XiaoyiConfig = serde_json::from_value(json!({
        "enabled": true,
        "access_key": "frontend-ak",
        "secret_key": "frontend-sk",
        "agent_id": "agent-1",
        "ws_url": "wss://example.test/ws"
    }))
    .expect("xiaoyi config aliases");

    assert!(config.enabled);
    assert_eq!(config.access_key, "frontend-ak");
    assert_eq!(config.secret_key, "frontend-sk");
    assert_eq!(config.agent_id, "agent-1");
    assert_eq!(config.ws_url, "wss://example.test/ws");
}

#[test]
fn matrix_config_normalizes_canonical_homeserver() {
    let config: MatrixConfig = serde_json::from_value(json!({
        "enabled": true,
        "homeserver": "  https://matrix.example.com/  ",
        "user_id": "@bot:example.com",
        "access_token": "token",
        "default_room_id": "!room:example.com"
    }))
    .expect("matrix config alias");

    assert!(config.enabled);
    assert_eq!(config.homeserver_url, "https://matrix.example.com");
    assert_eq!(config.user_id, "@bot:example.com");
    assert_eq!(config.access_token, "token");
    assert_eq!(config.default_room_id, "!room:example.com");
}

#[test]
fn matrix_config_preserves_homeserver_path() {
    let config: MatrixConfig = serde_json::from_value(json!({
        "homeserver": "https://matrix.example.com/_matrix",
        "password": "secret"
    }))
    .expect("matrix canonical config");

    assert_eq!(config.homeserver_url, "https://matrix.example.com/_matrix");
    assert_eq!(config.password, "secret");
}

#[test]
fn mqtt_config_preserves_numeric_port_and_normalizes_text_fields() {
    let config: MqttConfig = serde_json::from_value(json!({
        "enabled": true,
        "host": "  mqtt.example.test  ",
        "port": 8883,
        "subscribe_topic": "  rua/in/{client_id}  ",
        "publish_topic": "  rua/out/{client_id}  "
    }))
    .expect("mqtt config");

    assert!(config.enabled);
    assert_eq!(config.broker_url, "mqtt.example.test");
    assert_eq!(config.port, 8883);
    assert_eq!(config.subscribe_topic, "rua/in/{client_id}");
    assert_eq!(config.publish_topic, "rua/out/{client_id}");
}

#[test]
fn mqtt_port_defaults_only_when_omitted_and_rejects_invalid_explicit_values() {
    let missing: MqttConfig = serde_json::from_value(json!({
        "host": "mqtt.example.test"
    }))
    .expect("missing port uses default");
    assert_eq!(missing.port, 1883);

    for port in [
        json!(null),
        json!(""),
        json!("8883"),
        json!("not-a-port"),
        json!(-1),
        json!(65536),
        json!(1.5),
        json!(true),
        json!({}),
        json!([]),
    ] {
        let value = json!({"host":"mqtt.example.test", "port":port});
        assert!(
            serde_json::from_value::<MqttConfig>(value.clone()).is_err(),
            "{value}"
        );
        assert!(rua_config::channel_catalog::resolve_channel_config("mqtt", &value).is_err());
    }
    for port in [0, 1883, 8883, 65535] {
        let value = json!({"host":"mqtt.example.test", "port":port, "vendor":{"x":1}});
        let config: MqttConfig = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(config.port, port);
        assert_eq!(serde_json::to_value(config).unwrap()["port"], port);
        assert_eq!(
            rua_config::channel_catalog::resolve_channel_config("mqtt", &value).unwrap()["vendor"],
            json!({"x":1})
        );
    }
}
