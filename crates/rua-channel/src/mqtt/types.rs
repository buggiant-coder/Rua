use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MqttQos {
    AtMostOnce,
    #[default]
    AtLeastOnce,
    ExactlyOnce,
}

impl From<MqttQos> for rumqttc::QoS {
    fn from(q: MqttQos) -> Self {
        match q {
            MqttQos::AtMostOnce => rumqttc::QoS::AtMostOnce,
            MqttQos::AtLeastOnce => rumqttc::QoS::AtLeastOnce,
            MqttQos::ExactlyOnce => rumqttc::QoS::ExactlyOnce,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MqttCredentials {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MqttConfig {
    #[serde(default = "default_broker_url")]
    pub broker_url: String,

    #[serde(default = "default_port")]
    pub port: u16,

    #[serde(default)]
    pub client_id: Option<String>,

    #[serde(default = "default_subscribe_topic")]
    pub subscribe_topic: String,

    #[serde(default = "default_publish_topic")]
    pub publish_topic: String,

    #[serde(default)]
    pub qos: MqttQos,

    #[serde(default)]
    pub credentials: Option<MqttCredentials>,

    #[serde(default = "default_keep_alive_secs")]
    pub keep_alive_secs: u64,

    #[serde(default = "default_cap")]
    pub channel_capacity: usize,

    #[serde(default)]
    pub tls_enabled: bool,

    #[serde(default)]
    pub tls_ca_cert: Option<String>,

    #[serde(default)]
    pub tls_client_cert: Option<String>,

    #[serde(default)]
    pub tls_client_key: Option<String>,
}

impl Default for MqttConfig {
    fn default() -> Self {
        Self {
            broker_url: default_broker_url(),
            port: default_port(),
            client_id: None,
            subscribe_topic: default_subscribe_topic(),
            publish_topic: default_publish_topic(),
            qos: MqttQos::default(),
            credentials: None,
            keep_alive_secs: default_keep_alive_secs(),
            channel_capacity: default_cap(),
            tls_enabled: false,
            tls_ca_cert: None,
            tls_client_cert: None,
            tls_client_key: None,
        }
    }
}

fn default_broker_url() -> String {
    "localhost".into()
}

fn default_port() -> u16 {
    1883
}

fn default_subscribe_topic() -> String {
    "rua/inbound".into()
}

fn default_publish_topic() -> String {
    "rua/outbound".into()
}

fn default_keep_alive_secs() -> u64 {
    30
}

fn default_cap() -> usize {
    10
}
