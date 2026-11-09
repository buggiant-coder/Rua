use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginCapability {
    Tools,
    Hooks,
    Skills,
    Cli,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<serde_json::Value>,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

pub const PARSE_ERROR: i32 = -32700;
pub const INVALID_REQUEST: i32 = -32600;
pub const METHOD_NOT_FOUND: i32 = -32601;
pub const INVALID_PARAMS: i32 = -32602;
pub const INTERNAL_ERROR: i32 = -32603;

pub const PLUGIN_NOT_FOUND: i32 = -32000;
pub const HOOK_DISPATCH_FAILED: i32 = -32001;
pub const SKILL_NOT_FOUND: i32 = -32002;
pub const CLI_COMMAND_NOT_FOUND: i32 = -32003;
pub const INVOCATION_FAILED: i32 = -32004;

impl JsonRpcResponse {
    pub fn success(id: serde_json::Value, result: serde_json::Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn error(id: serde_json::Value, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.into(),
                data: None,
            }),
        }
    }
}

pub const METHOD_REGISTER_HOOK: &str = "plugin/register_hook";
pub const METHOD_REGISTER_SKILL: &str = "plugin/register_skill";
pub const METHOD_REGISTER_CLI: &str = "plugin/register_cli";
pub const METHOD_INVOKE_HOOK: &str = "plugin/invoke_hook";
pub const METHOD_INVOKE_SKILL: &str = "plugin/invoke_skill";
pub const METHOD_INVOKE_CLI: &str = "plugin/invoke_cli";

pub const ALL_METHODS: &[&str] = &[
    METHOD_REGISTER_HOOK,
    METHOD_REGISTER_SKILL,
    METHOD_REGISTER_CLI,
    METHOD_INVOKE_HOOK,
    METHOD_INVOKE_SKILL,
    METHOD_INVOKE_CLI,
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RegisterHookParams {
    pub plugin_id: String,
    pub hook_point: String,
    pub hook_name: String,
    pub callback: String,
    #[serde(default = "default_priority")]
    pub priority: i32,
}

fn default_priority() -> i32 {
    100
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RegisterHookResult {
    pub registered: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RegisterSkillParams {
    pub plugin_id: String,
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RegisterSkillResult {
    pub registered: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RegisterCliParams {
    pub plugin_id: String,
    pub name: String,
    pub description: String,
    pub callback: String,
    #[serde(default)]
    pub args: Vec<CliArgDef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CliArgDef {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RegisterCliResult {
    pub registered: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InvokeHookParams {
    pub hook_point: String,
    pub payload: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InvokeHookResultEntry {
    pub plugin_id: String,
    pub hook_name: String,
    pub output: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InvokeHookResult {
    pub results: Vec<InvokeHookResultEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InvokeSkillParams {
    pub plugin_id: String,
    pub skill_name: String,
    pub input: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InvokeSkillResult {
    pub output: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InvokeCliParams {
    pub plugin_id: String,
    pub cmd_name: String,
    pub args: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InvokeCliResult {
    pub output: String,
}

pub fn method_not_found_response(id: serde_json::Value, method: &str) -> JsonRpcResponse {
    JsonRpcResponse::error(id, METHOD_NOT_FOUND, format!("Method not found: {method}"))
}
