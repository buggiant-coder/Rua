pub mod app;
pub mod auth;
pub mod channels_reload;
pub mod console_transport;
pub mod control_adapters;
mod cron_bootstrap;
pub use cron_bootstrap::WorkspaceCronChannelSender;
pub mod desktop_automation;
pub mod desktop_endpoint;
pub mod desktop_host;
pub mod endpoints;
pub mod errors;
pub mod extractors;
pub mod install_mirror;
pub mod lifecycle;
pub mod local_model_resume;
pub mod mcp_reload;
pub mod mcp_tools_cache;
pub mod middleware;
pub mod panic_report;
pub mod path_augment;
mod plugin_bootstrap;
pub mod process_liveness;
pub mod profiles;
pub mod router;
pub mod seed;
pub mod services;
pub mod shutdown;
pub mod state;
pub mod static_assets;
pub mod streaming;
pub mod sync_cycle;
pub mod sync_domain_catalog;
pub mod sync_inbound_apply;
pub mod sync_migration;
pub mod sync_network;
pub mod sync_peer_directory;
pub mod sync_publication;
pub mod sync_runtime_controller;
pub mod telemetry;
mod tunnel_bootstrap;
pub mod usage_stats;
pub mod user_timezone;
mod voice_runtime;

pub mod qrcode_auth;
pub mod qrcode_handlers;
pub mod qrcode_image;
pub mod route_compat;
pub mod websocket;

pub use app::{build, serve, ServerOptions};
pub use errors::ApiError;
pub use lifecycle::{
    LifecycleManager, ManagedService, ShutdownContext, StartupContext, WarmupContext,
};
pub use middleware::{AgentExtractorContext, RequestId};
pub use profiles::ServerProfile;
pub use state::AppState;
pub use streaming::{
    sse_done_event, sse_error_event, sse_json_event, sse_response, ActiveStreamRegistry, Priority,
    StreamKey,
};
