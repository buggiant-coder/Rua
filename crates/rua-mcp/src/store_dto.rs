use serde::{Deserialize, Serialize};

pub use mcp_manager::types::{McpOAuthConfig, McpServerConfig, McpTransportConfig};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpClientConfig {
    #[serde(default)]
    pub key: String,
    #[serde(flatten)]
    pub server: McpServerConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpClientCreate {
    pub key: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub transport: String,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: Option<serde_json::Value>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub headers: Option<serde_json::Value>,
    #[serde(default)]
    pub timeout: Option<u64>,
    #[serde(default)]
    pub auto_start: Option<bool>,
    #[serde(default)]
    pub oauth: Option<McpOAuthConfig>,
    #[serde(default)]
    pub disabled_tools: Vec<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct McpClientCreateNested {
    pub client_key: String,
    pub client: serde_json::Value,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl McpClientCreateNested {
    pub fn into_flat(self) -> Result<McpClientCreate, serde_json::Error> {
        let mut client_obj = self.client;
        if let Some(obj) = client_obj.as_object_mut() {
            obj.insert(
                "key".to_string(),
                serde_json::Value::String(self.client_key),
            );
        }
        serde_json::from_value(client_obj)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct McpClientUpdate {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub transport: Option<String>,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Option<Vec<String>>,
    #[serde(default)]
    pub env: Option<serde_json::Value>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub headers: Option<serde_json::Value>,
    #[serde(default)]
    pub timeout: Option<u64>,
    #[serde(default)]
    pub auto_start: Option<bool>,
    #[serde(default)]
    pub oauth: Option<McpOAuthConfig>,
    #[serde(default)]
    pub disabled_tools: Option<Vec<String>>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToggleRequest {
    #[serde(default)]
    pub enabled: bool,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolInfo {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub schema: Option<serde_json::Value>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

fn default_true() -> bool {
    true
}

pub type SharedMcpClientConfig = McpClientConfig;

pub type SharedMcpClientCreate = McpClientCreate;

pub type SharedMcpClientUpdate = McpClientUpdate;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SharedMcpBroadcastRequest {
    #[serde(default)]
    pub agent_ids: Vec<String>,
    #[serde(default)]
    pub client_keys: Vec<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SharedMcpSyncRequest {
    #[serde(default)]
    pub agent_id: String,
    #[serde(default)]
    pub client_keys: Vec<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SharedMcpAgentResult {
    pub agent_id: String,
    pub success: bool,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SharedMcpBroadcastResponse {
    pub results: Vec<SharedMcpAgentResult>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

pub fn normalize_legacy_json(value: &serde_json::Value) -> serde_json::Value {
    let mut v = value.clone();
    let obj = match v.as_object_mut() {
        Some(o) => o,
        None => return v,
    };

    if !obj.contains_key("transport") {
        if let Some(t) = obj.remove("type") {
            obj.insert("transport".to_string(), t);
        }
    }

    if !obj.contains_key("enabled") {
        if let Some(a) = obj.remove("isActive") {
            obj.insert("enabled".to_string(), a);
        }
    }

    if !obj.contains_key("url") {
        if let Some(u) = obj.remove("baseUrl") {
            obj.insert("url".to_string(), u);
        }
    }

    if !obj.contains_key("transport") {
        obj.insert(
            "transport".to_string(),
            serde_json::Value::String("stdio".into()),
        );
    }

    for field in ["name", "description", "command", "url"] {
        if obj.get(field) == Some(&serde_json::Value::Null) {
            obj.insert(field.to_string(), serde_json::Value::String(String::new()));
        }
    }

    for field in ["env", "headers"] {
        if obj.get(field) == Some(&serde_json::Value::Null) {
            obj.insert(field.to_string(), serde_json::json!({}));
        }
    }

    if let Some(env_val) = obj.get_mut("env") {
        if let Some(env_obj) = env_val.as_object_mut() {
            env_obj.retain(|_, v| v.is_string());
        }
    }

    for field in ["args", "disabled_tools"] {
        if obj.get(field) == Some(&serde_json::Value::Null) {
            obj.insert(field.to_string(), serde_json::json!([]));
        }
    }

    if obj.get("cwd") == Some(&serde_json::Value::Null) {
        obj.remove("cwd");
    }

    v
}
