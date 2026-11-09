use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ToolParseMode {
    #[default]
    CallTitle,
    UpdateDetail,
    CallDetail,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpAgentConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default = "default_trusted")]
    pub trusted: bool,
    #[serde(default)]
    pub tool_parse_mode: ToolParseMode,
    #[serde(default = "default_stdio_buffer_limit_bytes")]
    pub stdio_buffer_limit_bytes: Option<u64>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

fn default_trusted() -> bool {
    false
}

fn default_stdio_buffer_limit_bytes() -> Option<u64> {
    Some(50 * 1024 * 1024)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpConfig {
    #[serde(default)]
    pub agents: HashMap<String, AcpAgentConfig>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl Default for AcpConfig {
    fn default() -> Self {
        Self {
            agents: HashMap::new(),
            extra: serde_json::Map::new(),
        }
    }
}

fn make_builtin_runner(
    command: &str,
    args: &[&str],
    tool_parse_mode: ToolParseMode,
) -> AcpAgentConfig {
    AcpAgentConfig {
        enabled: true,
        command: command.to_string(),
        args: args.iter().map(|s| s.to_string()).collect(),
        env: HashMap::new(),
        trusted: false,
        tool_parse_mode,
        stdio_buffer_limit_bytes: default_stdio_buffer_limit_bytes(),
        extra: serde_json::Map::new(),
    }
}

pub fn default_builtin_runners() -> HashMap<String, AcpAgentConfig> {
    let mut runners = HashMap::new();

    runners.insert(
        "opencode".into(),
        make_builtin_runner("opencode", &["acp"], ToolParseMode::UpdateDetail),
    );
    runners.insert(
        "qwen".into(),
        make_builtin_runner("qwen", &["--acp"], ToolParseMode::CallDetail),
    );
    runners.insert(
        "claude".into(),
        make_builtin_runner("claude-agent-acp", &[], ToolParseMode::UpdateDetail),
    );
    runners.insert(
        "codex".into(),
        make_builtin_runner("codex-acp", &[], ToolParseMode::CallDetail),
    );

    runners
}

impl AcpConfig {
    pub fn backfill_builtin_runners(&mut self) {
        for (key, value) in default_builtin_runners() {
            self.agents
                .entry(key)
                .and_modify(|runner| {
                    if runner.command.trim().is_empty() {
                        runner.command = value.command.clone();
                        runner.args = value.args.clone();
                    }
                })
                .or_insert(value);
        }
    }
}
