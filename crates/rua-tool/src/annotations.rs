use serde::{Deserialize, Serialize};

pub const TOOL_TRANSFER_CONTRACT_METADATA_KEY: &str = "rua_tool_transfer";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalPolicy {
    Never,

    Always,

    UnlessTrusted,

    #[default]
    ToolGuardDecides,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolTransferPolicy {
    Transferable,
    #[default]
    ForegroundOnly,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolTransferContract {
    pub policy: ToolTransferPolicy,
    #[serde(default)]
    pub input_capability: rua_task::TaskInputCapability,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait_argument: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_wait_seconds: Option<u64>,
}

impl ToolTransferContract {
    pub fn foreground_only() -> Self {
        Self {
            policy: ToolTransferPolicy::ForegroundOnly,
            input_capability: rua_task::TaskInputCapability::None,
            wait_argument: None,
            default_wait_seconds: None,
        }
    }

    pub fn transferable_seconds(argument: impl Into<String>, default_wait_seconds: u64) -> Self {
        Self {
            policy: ToolTransferPolicy::Transferable,
            input_capability: rua_task::TaskInputCapability::None,
            wait_argument: Some(argument.into()),
            default_wait_seconds: Some(default_wait_seconds),
        }
    }

    pub fn with_input_capability(
        mut self,
        input_capability: rua_task::TaskInputCapability,
    ) -> Self {
        self.input_capability = input_capability;
        self
    }

    pub fn resolve_wait(&self, arguments: &serde_json::Value) -> Option<std::time::Duration> {
        if self.policy != ToolTransferPolicy::Transferable {
            return None;
        }
        let argument = self.wait_argument.as_deref()?;
        let default_wait_seconds = self.default_wait_seconds?;
        let requested = arguments
            .get(argument)
            .and_then(parse_u64)
            .filter(|value| *value > 0)
            .unwrap_or(default_wait_seconds);
        Some(std::time::Duration::from_secs(requested))
    }
}

impl Default for ToolTransferContract {
    fn default() -> Self {
        Self::foreground_only()
    }
}

fn parse_u64(value: &serde_json::Value) -> Option<u64> {
    value.as_u64()
}

pub fn tool_transfer_contract(definition: &agent_types::ToolDefinition) -> ToolTransferContract {
    definition
        .metadata
        .get(TOOL_TRANSFER_CONTRACT_METADATA_KEY)
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default()
}

pub fn foreground_wait_limit(
    definition: &agent_types::ToolDefinition,
    arguments: &serde_json::Value,
) -> Option<std::time::Duration> {
    tool_transfer_contract(definition).resolve_wait(arguments)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtendedToolAnnotations {
    pub read_only: bool,

    pub destructive: bool,

    pub idempotent: bool,

    pub requires_network: bool,

    pub requires_shell: bool,

    pub requires_approval: ApprovalPolicy,

    pub allowed_on_mobile: bool,

    #[serde(default)]
    pub transfer: ToolTransferContract,
}

impl Default for ExtendedToolAnnotations {
    fn default() -> Self {
        Self {
            read_only: false,
            destructive: false,
            idempotent: false,
            requires_network: false,
            requires_shell: false,
            requires_approval: ApprovalPolicy::ToolGuardDecides,
            allowed_on_mobile: true,
            transfer: ToolTransferContract::default(),
        }
    }
}

impl ExtendedToolAnnotations {
    pub fn read_only() -> Self {
        Self {
            read_only: true,
            destructive: false,
            idempotent: true,
            requires_approval: ApprovalPolicy::Never,
            ..Default::default()
        }
    }

    pub fn destructive_file() -> Self {
        Self {
            read_only: false,
            destructive: true,
            idempotent: false,
            requires_approval: ApprovalPolicy::UnlessTrusted,
            ..Default::default()
        }
    }

    pub fn shell() -> Self {
        Self {
            read_only: false,
            destructive: false,
            idempotent: false,
            requires_shell: true,
            requires_approval: ApprovalPolicy::ToolGuardDecides,
            transfer: ToolTransferContract::transferable_seconds("yield_seconds", 10)
                .with_input_capability(rua_task::TaskInputCapability::Stdin),
            ..Default::default()
        }
    }

    pub fn to_tool_annotations(&self) -> agent_types::ToolAnnotations {
        agent_types::ToolAnnotations {
            title: None,
            read_only: Some(self.read_only),
            destructive: Some(self.destructive),
            idempotent: Some(self.idempotent),
            open_world: Some(self.requires_network),
            requires_approval: Some(matches!(
                self.requires_approval,
                ApprovalPolicy::Always | ApprovalPolicy::UnlessTrusted
            )),
            async_execution: None,
        }
    }

    pub fn to_tool_metadata(&self) -> serde_json::Map<String, serde_json::Value> {
        let mut metadata = serde_json::Map::new();
        metadata.insert(
            TOOL_TRANSFER_CONTRACT_METADATA_KEY.to_string(),
            serde_json::to_value(&self.transfer).expect("Tool transfer contract must serialize"),
        );
        metadata
    }
}
