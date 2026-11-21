use std::path::PathBuf;
use std::sync::Arc;

use rua_agent::compaction::ConversationCompactor;
use rua_contract::UsageRecorder;
use rua_plugin_runtime::{ActivePluginRegistry, PluginRegistry};
use rua_provider::manager::ProviderManager;
use rua_security::SecurityModule;

use crate::registry::AgentResolver;

pub struct WorkspaceContext {
    pub agent_cells: Arc<rua_agent::AgentCells>,
    pub agent_profile_store: Arc<rua_agent::AgentProfileStore>,
    pub provider_manager: Arc<ProviderManager>,
    pub plugin_registry: Arc<PluginRegistry>,
    pub active_plugin_registry: Arc<ActivePluginRegistry>,
    pub security: Arc<SecurityModule>,
    pub task_runtime: Arc<rua_task::TaskRuntime>,
    pub task_tool_router: Option<Arc<dyn rua_tool::extended::task_tools::TaskToolRouter>>,
    pub collaboration_runtime: Arc<dyn std::any::Any + Send + Sync>,
    pub chat_media_loader: Option<Arc<dyn rua_core::ChatMediaLoader>>,
    pub device_read_access: Option<Arc<dyn rua_core::tool::DeviceReadAccess>>,
    pub agents_instruction_loader:
        Option<Arc<dyn rua_core::agents_instruction_discovery::AgentsInstructionLoader>>,
    pub sync_transfer_runtime: Arc<dyn std::any::Any + Send + Sync>,
    pub remote_command_runtime: Option<Arc<dyn std::any::Any + Send + Sync>>,
    pub tool_config_store: Arc<rua_tool::ToolConfigStore>,
    pub tool_before_boundary: Option<Arc<dyn react_agent::ToolBeforeBoundary>>,
    pub tool_observer_boundary: Option<Arc<dyn react_agent::ToolObserverBoundary>>,
    pub permission_request_boundary: Option<Arc<dyn react_agent::PermissionRequestBoundary>>,

    pub rua_root: PathBuf,
    pub secret_dir: PathBuf,

    pub cache_dir: PathBuf,
    pub compactor: Arc<ConversationCompactor>,
    pub usage_recorder: Option<Arc<dyn UsageRecorder>>,
    pub push_store_dir: Option<PathBuf>,
    pub agent_resolver: Arc<AgentResolver>,

    pub channel_chat_facade:
        std::sync::OnceLock<std::sync::Weak<dyn rua_channel::core::console::ConsoleChatFacade>>,
    pub command_runtime_factory: std::sync::OnceLock<rua_contract::agent::CommandRuntimeFactory>,

    pub session_replication_hook:
        std::sync::OnceLock<Arc<dyn rua_contract::agent::SessionReplicationHook>>,

    pub memory_publication_hook:
        Arc<std::sync::OnceLock<Arc<dyn rua_agent::memory_manager::MemoryPublicationHook>>>,
}
