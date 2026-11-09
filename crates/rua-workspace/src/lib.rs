#![forbid(unsafe_code)]

pub mod channel_conflict;
pub mod channel_process_adapter;
pub mod chats_cleanup_store;
pub mod chats_store;
pub mod config;
pub mod context;
pub mod error;
pub mod files;
pub mod health;
pub mod import_export;
pub mod lifecycle;
pub mod registry;
pub mod root;
pub mod runner;
pub mod runtime;
pub mod service_manager;
pub mod services;
pub mod task_tool_execution;

pub use config::{deep_merge, merge_layers, ConfigLayer, LayeredConfig};
pub use context::WorkspaceContext;
pub use error::{WorkspaceError, WorkspaceErrorKind};
pub use files::{FileContentRecord, FileManager, FileRecord};
pub use health::{ComponentState, WorkspaceHealth, WorkspaceHealthBuilder};
pub use import_export::{WorkspaceImportExport, WorkspaceImportSummary, WorkspaceInfoRecord};
pub use lifecycle::{
    AgentWork, AgentWorkAuthority, InitPhase, PlatformProfile, ShutdownPolicy, ShutdownReport,
    WorkKind, WorkUnit, WorkspaceLifecycleState,
};
pub use registry::{
    AgentResolveRequest, AgentResolver, DefaultWorkspaceRegistry, WorkspaceRegistry,
};
pub use root::{DefaultWorkspaceFs, RuaRoot, WorkspaceFs, WorkspaceRoot};
pub use runner::{ActiveSessionTracker, SessionAcquireResult, SessionConcurrencyPolicy};
pub use runtime::{WorkspaceRuntime, WorkspaceSummary};
pub use service_manager::ImPlatformConfigs;
pub use service_manager::{
    default_service_descriptors, platform_service_descriptors, ActiveTaskTracker,
    ServiceDescriptor, ServiceLifecycle, ServiceManager, ServicePriority, ServiceStopOutcome,
};
pub use services::AgentServiceAdapter;
pub use task_tool_execution::WorkspaceToolTaskExecution;
