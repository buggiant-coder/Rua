use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Deserializer, Serialize};

fn null_to_default<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    let opt = Option::<T>::deserialize(d)?;
    Ok(opt.unwrap_or_default())
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(transparent)]
pub struct McpServerId(String);

impl McpServerId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for McpServerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for McpServerId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for McpServerId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "transport", rename_all = "snake_case")]
pub enum McpTransportConfig {
    Stdio {
        #[serde(default, deserialize_with = "null_to_default")]
        command: String,
        #[serde(default, deserialize_with = "null_to_default")]
        args: Vec<String>,
        #[serde(default, deserialize_with = "null_to_default")]
        env: BTreeMap<String, String>,
        #[serde(default, deserialize_with = "null_to_default")]
        cwd: Option<PathBuf>,

        #[serde(default)]
        use_proot: bool,
    },

    #[serde(rename = "streamable_http")]
    StreamableHttp {
        #[serde(default, deserialize_with = "null_to_default")]
        url: String,
        #[serde(default, deserialize_with = "null_to_default")]
        headers: BTreeMap<String, String>,
        #[serde(default = "default_timeout_secs")]
        timeout_secs: u64,
    },

    Sse {
        #[serde(default, deserialize_with = "null_to_default")]
        url: String,
        #[serde(default, deserialize_with = "null_to_default")]
        headers: BTreeMap<String, String>,
        #[serde(default)]
        reconnect: ReconnectPolicy,
    },
}

fn default_timeout_secs() -> u64 {
    30
}

impl McpTransportConfig {
    pub fn transport_type(&self) -> &'static str {
        match self {
            Self::Stdio { .. } => "stdio",
            Self::StreamableHttp { .. } => "streamable_http",
            Self::Sse { .. } => "sse",
        }
    }

    pub fn is_subprocess(&self) -> bool {
        matches!(self, Self::Stdio { .. })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReconnectPolicy {
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,

    #[serde(default = "default_base_delay_ms")]
    pub base_delay_ms: u64,

    #[serde(default = "default_max_delay_ms")]
    pub max_delay_ms: u64,
}

fn default_max_retries() -> u32 {
    3
}
fn default_base_delay_ms() -> u64 {
    1000
}
fn default_max_delay_ms() -> u64 {
    30000
}

impl Default for ReconnectPolicy {
    fn default() -> Self {
        Self {
            max_retries: default_max_retries(),
            max_delay_ms: default_max_delay_ms(),
            base_delay_ms: default_base_delay_ms(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct McpServerConfig {
    #[serde(default, deserialize_with = "null_to_default")]
    pub name: String,

    #[serde(default, deserialize_with = "null_to_default")]
    pub description: String,

    #[serde(default = "default_true")]
    pub enabled: bool,

    #[serde(flatten)]
    pub transport: McpTransportConfig,

    #[serde(default)]
    pub auto_connect: AutoConnectPolicy,

    #[serde(default = "default_call_timeout_secs")]
    pub call_timeout_secs: u64,

    #[serde(default = "default_max_concurrency")]
    pub max_concurrency: usize,

    #[serde(default)]
    pub oauth: Option<McpOAuthConfig>,

    #[serde(default, deserialize_with = "null_to_default")]
    pub disabled_tools: Vec<String>,

    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

fn default_true() -> bool {
    true
}
fn default_call_timeout_secs() -> u64 {
    60
}
fn default_max_concurrency() -> usize {
    4
}

impl McpServerConfig {
    pub fn call_timeout(&self) -> Duration {
        Duration::from_secs(self.call_timeout_secs)
    }

    pub fn is_tool_disabled(&self, tool_name: &str) -> bool {
        self.disabled_tools.iter().any(|name| name == tool_name)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct McpOAuthConfig {
    #[serde(default, deserialize_with = "null_to_default")]
    pub client_id: String,
    #[serde(default, deserialize_with = "null_to_default")]
    pub scope: String,
    #[serde(default, deserialize_with = "null_to_default")]
    pub access_token: String,
    #[serde(default, deserialize_with = "null_to_default")]
    pub refresh_token: String,

    #[serde(default)]
    pub expires_at: f64,
    #[serde(default, deserialize_with = "null_to_default")]
    pub token_endpoint: String,
    #[serde(default, deserialize_with = "null_to_default")]
    pub auth_endpoint: String,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl McpOAuthConfig {
    pub fn has_valid_access_token(&self) -> bool {
        if self.access_token.is_empty() {
            return false;
        }
        self.expires_at <= 0.0 || self.expires_at > current_unix_secs()
    }
}

fn current_unix_secs() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum AutoConnectPolicy {
    #[default]
    OnStartup,

    LazyOnFirstUse,

    Manual,

    DisabledOnMobile,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct McpConfig {
    #[serde(default)]
    pub clients: BTreeMap<McpServerId, McpServerConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct McpToolInfo {
    pub server_id: McpServerId,

    pub name: String,

    #[serde(default)]
    pub description: String,

    pub input_schema: serde_json::Value,

    #[serde(default)]
    pub raw: serde_json::Value,
}

#[derive(Debug, Clone, Default)]
pub struct McpCallContext {
    /// Existing tool-result stream; never controlled by model arguments.
    pub event_tx: Option<tokio::sync::mpsc::Sender<agent_types::AgentEvent>>,
    /// Trusted host context; never merged with model-supplied tool arguments.
    pub metadata: BTreeMap<String, serde_json::Value>,

    pub agent_id: String,

    pub session_id: String,

    pub cancel: Option<tokio::sync::watch::Receiver<bool>>,

    pub timeout: Option<Duration>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpCallResult {
    pub content: Vec<McpContent>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_content: Option<serde_json::Value>,

    #[serde(default)]
    pub is_error: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum McpContent {
    Text { text: String },
    Image { data: String, mime_type: String },
    Resource { uri: String, text: Option<String> },
}

impl McpCallResult {
    pub fn text(&self) -> String {
        self.content
            .iter()
            .filter_map(|c| match c {
                McpContent::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn exceeds_size(&self, max_bytes: usize) -> bool {
        self.content
            .iter()
            .map(|c| match c {
                McpContent::Text { text } => text.len(),
                McpContent::Image { data, .. } => data.len(),
                McpContent::Resource { text, uri, .. } => {
                    text.as_ref().map_or(0, |t| t.len()) + uri.len()
                }
            })
            .sum::<usize>()
            .saturating_add(
                self.structured_content
                    .as_ref()
                    .map_or(0, |value| value.to_string().len()),
            )
            > max_bytes
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpInitializeResult {
    pub server_name: String,

    pub server_version: String,

    pub protocol_version: String,

    #[serde(default)]
    pub capabilities: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct McpReloadReport {
    pub added: Vec<McpServerId>,

    pub removed: Vec<McpServerId>,

    pub updated: Vec<McpServerId>,

    pub unchanged: Vec<McpServerId>,

    #[serde(default)]
    pub errors: BTreeMap<McpServerId, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct McpShutdownReport {
    pub success: Vec<McpServerId>,

    pub errors: BTreeMap<McpServerId, String>,
}

#[derive(Debug, Clone, Default)]
pub struct ToolFilter {
    pub server_id: Option<McpServerId>,

    pub name_prefix: Option<String>,

    pub include_disabled: bool,
}
