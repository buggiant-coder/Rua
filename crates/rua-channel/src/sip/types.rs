use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SipConfig {
    #[serde(default = "default_bind_addr")]
    pub bind_addr: String,
    #[serde(default)]
    pub public_addr: String,
    #[serde(default = "default_rtp_bind_addr")]
    pub rtp_bind_addr: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub domain: String,
    #[serde(default)]
    pub media_dir: String,
    #[serde(default = "default_max_call_secs")]
    pub max_call_secs: u64,
}

impl Default for SipConfig {
    fn default() -> Self {
        Self {
            bind_addr: default_bind_addr(),
            public_addr: String::new(),
            rtp_bind_addr: default_rtp_bind_addr(),
            username: String::new(),
            domain: String::new(),
            media_dir: String::new(),
            max_call_secs: default_max_call_secs(),
        }
    }
}

fn default_bind_addr() -> String {
    "127.0.0.1:5060".into()
}

fn default_rtp_bind_addr() -> String {
    "127.0.0.1:0".into()
}

fn default_max_call_secs() -> u64 {
    300
}

/// QwenPaw SIP settings. The native packet API remains available for older configs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SipOptions {
    pub sip_mode: String,
    pub sip_host: String,
    pub sip_port: u16,
    pub sip_username: String,
    pub sip_password: String,
    pub sip_server: String,
    pub sip_transport: String,
    pub rtp_port_low: u16,
    pub rtp_port_high: u16,
    pub dashscope_api_key: String,
    pub tts_provider: String,
    pub tts_voice: String,
    pub stt_provider: String,
    pub language: String,
    pub welcome_greeting: String,
    pub call_timeout: f64,
    pub livekit_url: String,
    pub livekit_api_key: String,
    pub livekit_api_secret: String,
    pub livekit_sip_trunk_id: String,
    pub livekit_room_name: String,
    pub livekit_output_sample_rate: u32,
    pub max_concurrent_calls: usize,
    #[serde(flatten)]
    pub extra: std::collections::BTreeMap<String, serde_json::Value>,
}

impl Default for SipOptions {
    fn default() -> Self {
        Self {
            sip_mode: "dev".into(),
            sip_host: "0.0.0.0".into(),
            sip_port: 5061,
            sip_username: String::new(),
            sip_password: String::new(),
            sip_server: String::new(),
            sip_transport: "UDP".into(),
            rtp_port_low: 10000,
            rtp_port_high: 20000,
            dashscope_api_key: String::new(),
            tts_provider: "aliyun".into(),
            tts_voice: String::new(),
            stt_provider: "aliyun".into(),
            language: "zh-CN".into(),
            welcome_greeting: "你好，我是Rua".into(),
            call_timeout: 120.0,
            livekit_url: String::new(),
            livekit_api_key: String::new(),
            livekit_api_secret: String::new(),
            livekit_sip_trunk_id: String::new(),
            livekit_room_name: "sip-inbound".into(),
            livekit_output_sample_rate: 24000,
            max_concurrent_calls: 5,
            extra: Default::default(),
        }
    }
}

impl SipOptions {
    pub(super) fn api_key(&self) -> String {
        if self.dashscope_api_key.is_empty() {
            std::env::var("DASHSCOPE_API_KEY").unwrap_or_default()
        } else {
            self.dashscope_api_key.clone()
        }
    }

    pub(super) fn speech_url(&self) -> &str {
        self.extra
            .get("dashscope_ws_url")
            .and_then(|v| v.as_str())
            .unwrap_or("wss://dashscope.aliyuncs.com/api-ws/v1/inference/")
    }

    pub(super) fn validate(&self) -> rua_core::Result<()> {
        let error = |message: &str| rua_core::RuaError::Validation {
            message: message.into(),
        };
        if !["native", "dev", "livekit"].contains(&self.sip_mode.as_str()) {
            return Err(error("sip_mode must be native, dev or livekit"));
        }
        if self.sip_mode == "native" {
            return Ok(());
        }
        if self.api_key().is_empty() {
            return Err(error(
                "SIP voice requires dashscope_api_key or DASHSCOPE_API_KEY",
            ));
        }
        if self.stt_provider != "aliyun" || self.tts_provider != "aliyun" {
            return Err(error("QwenPaw SIP supports aliyun STT/TTS"));
        }
        if self.max_concurrent_calls == 0
            || !self.call_timeout.is_finite()
            || self.call_timeout > 31_536_000.0
        {
            return Err(error("invalid SIP call limit or timeout"));
        }
        if self.sip_mode == "dev" {
            if !self.sip_transport.eq_ignore_ascii_case("UDP") {
                return Err(error(
                    "SIP dev mode uses UDP, matching QwenPaw's pyVoIP backend",
                ));
            }
            if self.rtp_port_low > self.rtp_port_high {
                return Err(error("rtp_port_low must not exceed rtp_port_high"));
            }
        }
        if self.sip_mode == "livekit" {
            if self.livekit_url.is_empty()
                || self.livekit_api_key.is_empty()
                || self.livekit_api_secret.is_empty()
            {
                return Err(error("LiveKit requires URL, API key and API secret"));
            }
            if ![8000, 16000, 22050, 24000].contains(&self.livekit_output_sample_rate) {
                return Err(error(
                    "LiveKit TTS sample rate must be 8000, 16000, 22050 or 24000",
                ));
            }
        }
        Ok(())
    }
}
