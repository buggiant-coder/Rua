use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;

use mcp_manager::manager::McpManager;
use rua_contract::agent::AgentRuntime;
use rua_contract::config::AppConfig;
use rua_contract::cron::SystemScheduler;
use rua_plugin_runtime::PluginRegistry;

use rua_channel::core::console::ConsoleChatFacade;
use rua_channel::core::ChannelManager;
use rua_tunnel::CloudflareTunnelDriver;
use rua_workspace::{
    AgentResolver, DefaultWorkspaceRegistry, LayeredConfig, PlatformProfile, RuaRoot,
    WorkspaceContext, WorkspaceRegistry,
};

use chrono::{DateTime, Utc};

use crate::auth::AuthService;
use crate::endpoints::messages::PushMessageStore;
use crate::lifecycle::LifecycleManager;
use crate::mcp_tools_cache::McpToolsCache;
use crate::profiles::ServerProfile;
use crate::qrcode_auth::QRCodeHandlerRegistry;
use crate::qrcode_handlers::dingtalk::DingtalkQRCodeAuthHandler;
use crate::qrcode_handlers::feishu::FeishuQRCodeAuthHandler;
use crate::qrcode_handlers::qq::QqQRCodeAuthHandler;
use crate::qrcode_handlers::wecom::WecomQRCodeAuthHandler;
use crate::qrcode_handlers::weixin::WeixinQRCodeAuthHandler;
use crate::services::agent_manager::AgentManager;
use crate::services::console_chat::{
    ConsoleChatFacadeImpl, ConsoleChatService, SessionSyncEmitter,
};
use crate::services::skills::FileSkillService;
use crate::services::usage::UsageRecordingChatService;
use crate::streaming::ActiveStreamRegistry;
use crate::sync_network::{BoundSyncNetwork, BoundSyncNetworkError, SyncNetworkMembership};
use crate::sync_runtime_controller::{SyncRuntimeController, SyncRuntimeError, SyncRuntimeParts};
use crate::usage_stats::{spawn_flush_task as spawn_usage_flush, UsageStats};
use rua_usage::UsageLedger;

#[derive(Debug, Clone, serde::Serialize)]
pub struct TunnelAlertState {
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_failure_at: Option<DateTime<Utc>>,
}

impl Default for TunnelAlertState {
    fn default() -> Self {
        Self {
            state: "disconnected".to_string(),
            started_at: None,
            last_failure_at: None,
        }
    }
}

pub type SyncHlcClockCell = Arc<std::sync::Mutex<Option<([u8; 8], rua_sync::hlc::HlcClock)>>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncRepairLane {
    Daily,
    Bulk,
    Interactive,
}

impl SyncRepairLane {
    pub fn from_reason(reason: &str) -> Self {
        match reason {
            "targeted_cycle" => Self::Interactive,
            "manual_cycle" | "mobile_manual_cycle" => Self::Bulk,
            _ => Self::Daily,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Daily => "daily",
            Self::Bulk => "bulk",
            Self::Interactive => "interactive",
        }
    }

    fn priority(self) -> u8 {
        match self {
            Self::Daily => 0,
            Self::Bulk => 1,
            Self::Interactive => 2,
        }
    }
}

#[derive(Debug, Default)]
pub struct SyncRepairSchedulerState {
    pub active_lane: Option<SyncRepairLane>,
    pub pending_lane: Option<SyncRepairLane>,
}

impl SyncRepairSchedulerState {
    pub fn schedule(&mut self, lane: SyncRepairLane) -> bool {
        if self.active_lane.is_none() {
            self.active_lane = Some(lane);
            return true;
        }
        self.pending_lane = Some(match self.pending_lane {
            Some(current) if current.priority() >= lane.priority() => current,
            _ => lane,
        });
        false
    }

    pub fn finish_active(&mut self) -> Option<SyncRepairLane> {
        if let Some(next) = self.pending_lane.take() {
            self.active_lane = Some(next);
            Some(next)
        } else {
            self.active_lane = None;
            None
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncRepairDiagnostics {
    pub active_lane: Option<SyncRepairLane>,
    pub pending_lane: Option<SyncRepairLane>,
}

#[derive(Default)]
struct PlatformInitialization {
    task: Option<tokio::task::JoinHandle<Result<(), String>>>,
    result: Option<Result<(), String>>,
}

#[derive(Clone)]
pub struct AppState {
    platform_initialization: Arc<tokio::sync::Mutex<PlatformInitialization>>,
    pub config: Arc<AppConfig>,
    pub auth: Arc<AuthService>,

    pub registry: Arc<dyn WorkspaceRegistry>,
    pub task_runtime: Arc<rua_task::TaskRuntime>,
    pub acp_permission_relay: Arc<rua_acp::permissions::AcpPermissionRelay>,

    pub streams: Arc<ActiveStreamRegistry>,
    pub conversation_authority: Arc<crate::services::conversation_authority::ConversationAuthority>,
    pub product_hooks: Arc<crate::services::product_hooks::ProductHookService>,
    pub collaboration: Arc<crate::services::collaboration::CollaborationService>,
    pub collaboration_runtime: Arc<dyn std::any::Any + Send + Sync>,
    pub conversation_input_intake:
        crate::services::conversation_input_intake::ConversationInputIntake,
    pub active_stream_broadcast_hub:
        Arc<crate::services::active_stream_broadcast::ActiveStreamBroadcastHub>,
    pub chat_activity_wake_hub: Arc<crate::services::chat_activity_wake::ChatActivityWakeHub>,
    pub terminal_authority: Arc<crate::services::terminal::TerminalSessionAuthority>,
    pub terminal_remote_client: Arc<dyn crate::services::terminal::TerminalRemoteCommandClient>,
    pub terminal_remote_activity_client:
        Arc<dyn crate::services::terminal::TerminalRemoteActivityClient>,
    pub agent_service: Arc<AgentManager>,
    pub chat_service: Arc<UsageRecordingChatService>,
    pub llm_debug_captures: Arc<crate::services::llm_debug_capture::LlmDebugCaptureRegistry>,
    pub console_facade: Arc<dyn ConsoleChatFacade>,
    pub provider_manager: Arc<rua_provider::manager::ProviderManager>,
    pub providers_loader: Arc<rua_provider::config_loader::ProviderConfigLoader>,

    pub cell_store: Arc<cell_store::FileSystemCellStore>,

    pub provider_cells: Arc<rua_provider::ProviderCells>,

    pub shared_mcp_cells: Arc<rua_mcp::SharedMcpCells>,

    pub cron_cells: Arc<rua_cron::CronCells>,

    pub cron_scheduler: Arc<dyn SystemScheduler>,

    pub agent_cells: Arc<rua_agent::AgentCells>,

    pub agent_mcp_cells: Arc<rua_mcp::AgentMcpCells>,

    pub agent_tools_cells: Arc<rua_tool::AgentToolsCells>,
    pub tool_catalog: Arc<serde_json::Map<String, serde_json::Value>>,
    pub tool_config_store: Arc<rua_tool::ToolConfigStore>,

    pub project_catalog: Arc<rua_project::ProjectCatalog>,

    pub project_bindings: Arc<rua_project::ProjectResourceBindingStore>,
    pub skill_service: Arc<FileSkillService>,
    pub local_model: Arc<rua_local_model::LocalModelManager>,
    pub plugins: Arc<rua_plugin::PluginManager>,
    pub plugin_registry: Arc<PluginRegistry>,
    pub active_plugin_registry: Arc<rua_plugin_runtime::ActivePluginRegistry>,

    pub plugin_skill_registry: Arc<rua_plugin_runtime::PluginSkillRegistry>,

    pub qrcode_registry: Arc<QRCodeHandlerRegistry>,
    pub push_message_store: PushMessageStore,
    pub user_inbox: Arc<crate::services::user_inbox::UserInbox>,
    pub voice: Arc<rua_voice::VoiceManager>,
    pub(crate) voice_runtime: Arc<crate::voice_runtime::VoiceRuntime>,
    pub tunnel_driver: Arc<tokio::sync::Mutex<Option<CloudflareTunnelDriver>>>,
    pub tunnel_alert_state: Arc<tokio::sync::RwLock<TunnelAlertState>>,
    pub lifecycle: Arc<LifecycleManager>,
    pub desktop_automation: Arc<crate::desktop_automation::DesktopAutomationRuntime>,
    pub profile: ServerProfile,
    pub ready: Arc<AtomicBool>,
    pub shutdown_tx: Arc<tokio::sync::watch::Sender<bool>>,
    pub rua_root: PathBuf,
    pub secret_dir: PathBuf,
    pub cache_dir: PathBuf,

    pub usage_stats: Arc<UsageStats>,

    pub mcp_tools_cache: Arc<McpToolsCache>,

    pub pairing_state: Arc<tokio::sync::Mutex<crate::endpoints::pairing::PairingState>>,

    pub security: Arc<rua_security::SecurityModule>,

    pub device_network: Arc<crate::services::device_session_access::DeviceNetworkAccess>,
    pub device_network_availability:
        Arc<crate::services::device_network_availability::DeviceNetworkAvailability>,
    pub user_notifications: Arc<crate::services::user_notification::UserNotificationService>,
    pub user_notification_events:
        Arc<crate::services::console_user_notification::ConsoleUserNotificationAdapter>,

    pub sync_node: Arc<tokio::sync::Mutex<Option<rua_sync::endpoint::SyncNode>>>,
    pub sync_transfer_session_manager:
        Arc<std::sync::RwLock<Option<rua_device_network::DeviceSessionManager>>>,
    pub chat_media_transfers: Arc<crate::services::chat_media_transfer::ChatMediaTransferRegistry>,

    pub sync_trigger: Arc<tokio::sync::RwLock<Option<rua_sync::trigger::SyncTriggerHandle>>>,

    pub sync_runtime: Arc<SyncRuntimeController>,

    pub device_key: Arc<rua_sync::crypto::DeviceKey>,

    pub sync_enabled: Arc<AtomicBool>,
    pub sync_control_lock: Arc<tokio::sync::Mutex<()>>,

    pub sync_power_policy: Arc<std::sync::RwLock<rua_sync::power_policy::LoadedSyncPowerPolicy>>,

    pub sync_network_secret: Arc<std::sync::RwLock<Option<Arc<rua_sync::identity::NetworkSecret>>>>,

    pub sync_device: Arc<std::sync::RwLock<Option<Arc<rua_sync::identity::DeviceIdentity>>>>,
    pub remote_history_sync_tx: tokio::sync::mpsc::UnboundedSender<
        crate::services::device_command_bus::ConversationHistorySyncRequest,
    >,

    pub sync_hlc_clock: SyncHlcClockCell,

    pub commands_metadata: Arc<Vec<crate::endpoints::commands_discovery::CommandEntry>>,

    pub sidecar_store: Arc<rua_sync::sidecar::SidecarStore>,

    pub inbound_inbox: Arc<rua_sync::inbound_inbox::InboundInbox>,

    pub sync_stage_recorder: Arc<rua_sync::stage_diagnostics::SyncStageRecorder>,

    pub inbound_apply_lock: Arc<tokio::sync::Mutex<()>>,

    pub sync_repair_scheduler: Arc<std::sync::Mutex<SyncRepairSchedulerState>>,

    pub sync_transfer_jobs: Arc<crate::services::sync_transfer::TransferJobStore>,

    pub foreground_operations:
        Arc<crate::services::foreground_operation::ForegroundOperationRegistry>,

    pub acp_chat_registry: Arc<crate::services::acp_chat_registry::AcpChatRegistry>,
}

#[derive(Clone)]
struct SyncRekeyRuntimeContext {
    rua_root: PathBuf,
    secret_dir: PathBuf,
    device_key: [u8; 32],
    sync_node: Weak<tokio::sync::Mutex<Option<rua_sync::endpoint::SyncNode>>>,
    sync_network_secret: Weak<std::sync::RwLock<Option<Arc<rua_sync::identity::NetworkSecret>>>>,
    sync_device: Weak<std::sync::RwLock<Option<Arc<rua_sync::identity::DeviceIdentity>>>>,
    sidecar_store: Weak<rua_sync::sidecar::SidecarStore>,
    provider_manager: Weak<rua_provider::manager::ProviderManager>,
    security: Weak<rua_security::SecurityModule>,
    project_catalog: Weak<rua_project::ProjectCatalog>,
    registry: Weak<dyn WorkspaceRegistry>,
    inbound_inbox: Weak<rua_sync::inbound_inbox::InboundInbox>,
    inbound_apply_lock: Weak<tokio::sync::Mutex<()>>,
}

#[derive(Clone)]
struct ForegroundOperationLifecycleContext {
    sync_runtime: Weak<SyncRuntimeController>,
    sync_power_policy: Weak<std::sync::RwLock<rua_sync::power_policy::LoadedSyncPowerPolicy>>,
    inbound_apply_lock: Weak<tokio::sync::Mutex<()>>,
    device_network: Weak<crate::services::device_session_access::DeviceNetworkAccess>,
    device_network_availability:
        Weak<crate::services::device_network_availability::DeviceNetworkAvailability>,
    sync_node: Weak<tokio::sync::Mutex<Option<rua_sync::endpoint::SyncNode>>>,
    sync_transfer_session_manager:
        Weak<std::sync::RwLock<Option<rua_device_network::DeviceSessionManager>>>,
}

impl ForegroundOperationLifecycleContext {
    async fn clear_sync_node(&self) {
        let (
            Some(inbound_apply_lock),
            Some(device_network),
            Some(device_network_availability),
            Some(sync_node),
            Some(sync_transfer_session_manager),
        ) = (
            self.inbound_apply_lock.upgrade(),
            self.device_network.upgrade(),
            self.device_network_availability.upgrade(),
            self.sync_node.upgrade(),
            self.sync_transfer_session_manager.upgrade(),
        )
        else {
            return;
        };
        clear_sync_node_parts(
            &inbound_apply_lock,
            &device_network,
            &device_network_availability,
            &sync_node,
            &sync_transfer_session_manager,
        )
        .await;
    }
}

async fn clear_sync_node_parts(
    inbound_apply_lock: &Arc<tokio::sync::Mutex<()>>,
    device_network: &Arc<crate::services::device_session_access::DeviceNetworkAccess>,
    device_network_availability: &Arc<
        crate::services::device_network_availability::DeviceNetworkAvailability,
    >,
    sync_node: &Arc<tokio::sync::Mutex<Option<rua_sync::endpoint::SyncNode>>>,
    sync_transfer_session_manager: &Arc<
        std::sync::RwLock<Option<rua_device_network::DeviceSessionManager>>,
    >,
) {
    let _apply_guard = inbound_apply_lock.lock().await;
    let (old_runtime, old_node) = {
        let mut network_guard = device_network.lock().await;
        let mut node_guard = sync_node.lock().await;
        (network_guard.take(), node_guard.take())
    };
    *sync_transfer_session_manager.write().unwrap() = None;
    if let Some(old_node) = old_node.as_ref() {
        old_node.shutdown().await;
    } else if let Some(old_runtime) = old_runtime.as_ref() {
        if let Err(error) = old_runtime.shutdown().await {
            tracing::warn!(error = %error, "failed to shut down Device Network Runtime");
        }
    }
    drop(old_node);
    device_network_availability.mark_suspended();
}

async fn remote_conversation_history_sync_from_context(
    context: &SyncRekeyRuntimeContext,
    source_device_id: &str,
) -> Result<(), String> {
    let sync_node = context
        .sync_node
        .upgrade()
        .ok_or_else(|| "sync node state is gone".to_string())?;
    let sync_network_secret = context
        .sync_network_secret
        .upgrade()
        .ok_or_else(|| "sync network secret state is gone".to_string())?;
    let sync_device = context
        .sync_device
        .upgrade()
        .ok_or_else(|| "sync device state is gone".to_string())?;
    let sidecar_store = context
        .sidecar_store
        .upgrade()
        .ok_or_else(|| "sync sidecar store is gone".to_string())?;
    let provider_manager = context
        .provider_manager
        .upgrade()
        .ok_or_else(|| "provider manager is gone".to_string())?;
    let security = context
        .security
        .upgrade()
        .ok_or_else(|| "security module is gone".to_string())?;
    let project_catalog = context
        .project_catalog
        .upgrade()
        .ok_or_else(|| "Project Catalog is gone".to_string())?;
    let registry = context
        .registry
        .upgrade()
        .ok_or_else(|| "workspace registry is gone".to_string())?;
    let inbound_inbox = context
        .inbound_inbox
        .upgrade()
        .ok_or_else(|| "sync inbound inbox is gone".to_string())?;
    let inbound_apply_lock = context
        .inbound_apply_lock
        .upgrade()
        .ok_or_else(|| "sync inbound apply lock is gone".to_string())?;

    crate::sync_cycle::run_sync_cycle_to_device_id(
        &sync_node,
        &context.rua_root.join("sync"),
        source_device_id,
    )
    .await?;
    crate::sync_inbound_apply::InboundApplyRuntime::from_parts(
        sidecar_store,
        sync_node,
        sync_network_secret,
        sync_device,
        provider_manager,
        security,
        Some(registry),
        context.device_key,
        context.rua_root.clone(),
        context.secret_dir.clone(),
        inbound_inbox,
        inbound_apply_lock,
        project_catalog,
    )
    .repair_scan_and_drain()
    .await
    .map(|_| ())
    .map_err(|error| format!("repair scan/drain after remote history sync: {error}"))
}

impl AppState {
    pub fn load(
        config: AppConfig,
        data_dir: PathBuf,
        secret_dir: PathBuf,
        profile: ServerProfile,
    ) -> rua_core::Result<Self> {
        let root_config = rua_config::load_config(&data_dir.join("config.json"), &data_dir)
            .map_err(|error| rua_core::RuaError::Config {
                message: error.to_string(),
            })?;
        let cache_dir = rua_config::dirs::resolve_cache_dir(&data_dir);
        let (shutdown_tx, _) = tokio::sync::watch::channel(false);
        let streams = Arc::new(ActiveStreamRegistry::new());
        let foreground_operations = Arc::new(
            crate::services::foreground_operation::ForegroundOperationRegistry::new(
                profile.sync_runtime_host_profile()
                    == rua_sync::power_policy::SyncRuntimeHostProfile::MobileApk,
            ),
        );
        let product_hooks = Arc::new(crate::services::product_hooks::ProductHookService::new());
        let conversation_authority = Arc::new(
            crate::services::conversation_authority::ConversationAuthority::with_product_hooks(
                Arc::clone(&foreground_operations),
                Arc::clone(&product_hooks),
            ),
        );

        let device_key = Arc::new(
            match rua_sync::crypto::DeviceKey::generate_or_load(&secret_dir) {
                Ok(k) => k,
                Err(e) => {
                    tracing::error!(error = %e, "[device_key] failed to generate/load device key");
                    rua_sync::crypto::DeviceKey::generate()
                }
            },
        );
        let device_key_bytes = *device_key.as_bytes();
        let sync_dir = data_dir.join("sync");
        let device = Arc::new(
            rua_sync::identity::DeviceIdentity::generate_or_load(&data_dir)
                .expect("[sync] device identity init failed"),
        );
        let sync_device_cell: Arc<
            std::sync::RwLock<Option<Arc<rua_sync::identity::DeviceIdentity>>>,
        > = Arc::new(std::sync::RwLock::new(Some(Arc::clone(&device))));

        let providers_loader =
            Arc::new(rua_provider::config_loader::ProviderConfigLoader::with_key(
                secret_dir.clone(),
                device_key_bytes,
            ));
        let provider_manager =
            Arc::new(rua_provider::manager::ProviderManager::with_config_loader(
                rua_provider::config_loader::ProviderConfigLoader::with_key(
                    secret_dir.clone(),
                    device_key_bytes,
                ),
            ));
        let cell_store = Arc::new(cell_store::FileSystemCellStore::new());
        let project_stores = rua_project::ProjectStores::open(data_dir.join("projects"))
            .expect("Project stores open must succeed");
        let project_catalog = project_stores.catalog;
        let project_bindings = project_stores.bindings;
        let provider_cells = Arc::new(rua_provider::ProviderCells::with_key(
            Arc::clone(&cell_store),
            Arc::clone(&provider_manager),
            secret_dir.join("providers").join("providers"),
            device_key_bytes,
        ));
        let shared_mcp_cells = Arc::new(rua_mcp::SharedMcpCells::with_key(
            Arc::clone(&cell_store),
            data_dir.join("shared_mcp.json"),
            device_key_bytes,
        ));

        let agent_cells = Arc::new(rua_agent::AgentCells::new(
            Arc::clone(&cell_store),
            data_dir.join("workspaces"),
        ));
        let agent_profile_store = Arc::new(rua_agent::AgentProfileStore::new(data_dir.clone()));
        let agent_mcp_cells = Arc::new(rua_mcp::AgentMcpCells::new(
            Arc::clone(&cell_store),
            data_dir.join("workspaces"),
        ));
        let agent_tools_cells = Arc::new(rua_tool::AgentToolsCells::new(
            Arc::clone(&cell_store),
            data_dir.join("workspaces"),
        ));
        let tool_catalog = Arc::new(
            crate::services::collaboration_runtime::server_tool_catalog()
                .expect("server Tool Registry must build"),
        );
        let tool_config_store = Arc::new(
            rua_tool::ToolConfigStore::new(Arc::clone(&agent_tools_cells))
                .with_catalog((*tool_catalog).clone())
                .with_device_key(device_key_bytes),
        );
        let workspace_skill_cells = Arc::new(rua_skills::WorkspaceSkillCells::new(
            Arc::clone(&cell_store),
            data_dir.join("workspaces"),
        ));

        {
            let pm = Arc::clone(&provider_manager);
            let loader = Arc::clone(&providers_loader);
            let _ = std::thread::spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("failed to build bootstrap runtime");
                rt.block_on(async move {
                    match loader.load_default_model().await {
                        Ok(active) => pm.store_default_model(active).await,
                        Err(e) => {
                            tracing::debug!(
                                error = %e,
                                "no default_model.json at startup (or unreadable), continuing without"
                            );
                        }
                    }
                });
            })
            .join();
        }

        {
            let pm = Arc::clone(&provider_manager);
            let cells = Arc::clone(&provider_cells);
            let _ = std::thread::spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("failed to build provider seed runtime");
                rt.block_on(async move {
                    for template in rua_provider::manager::PROVIDER_TEMPLATES
                        .iter()
                        .filter(|template| template.is_zero_config_free_tier())
                    {
                        let cell = cells.cell(template.id);
                        match cell.read().await {
                            Ok(Some(config)) => {
                                match rua_provider::manager::create_provider_from_config(&config) {
                                    Ok(provider) => pm.register(provider),
                                    Err(e) => tracing::warn!(
                                        provider_id = %config.id,
                                        error = %e,
                                        "failed to create runtime instance for seeded provider"
                                    ),
                                }
                            }
                            Ok(None) => {
                                if let Err(e) = cell.write(template.to_config()).await {
                                    tracing::warn!(
                                        provider_id = %template.id,
                                        error = %e,
                                        "failed to seed zero-config provider"
                                    );
                                }
                            }
                            Err(e) => tracing::warn!(
                                provider_id = %template.id,
                                error = %e,
                                "failed to read zero-config provider during seed"
                            ),
                        }
                    }
                });
            })
            .join();
        }

        {
            let pm = Arc::clone(&provider_manager);
            let cells = Arc::clone(&provider_cells);
            let secret = secret_dir.clone();
            let _ = std::thread::Builder::new()
                .name("provider-hydration".into())
                .spawn(move || {
                    let rt = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .expect("failed to build provider hydration runtime");
                    rt.block_on(async move {
                        if let Err(e) = rua_provider::migrate_legacy_providers(&secret) {
                            tracing::warn!(
                                error = %e,
                                "legacy provider migration failed at startup, continuing without"
                            );
                        }
                        let configs = match cells.list().await {
                            Ok(c) => c,
                            Err(e) => {
                                tracing::warn!(
                                    error = %e,
                                    "failed to list provider configs at startup, continuing without"
                                );
                                return;
                            }
                        };
                        for config in &configs {
                            match rua_provider::manager::create_provider_from_config(config) {
                                Ok(provider) => pm.register(provider),
                                Err(e) => tracing::warn!(
                                    provider_id = %config.id,
                                    error = %e,
                                    "failed to create runtime instance for provider, skipping"
                                ),
                            }
                        }
                        let working_dir = rua_config::dirs::rua_root();
                        let resumed = rua_provider::resume_local_models(&working_dir);
                        for entry in &resumed {
                            tracing::info!(
                                model_id = %entry.model_id,
                                pid = entry.pid,
                                status = ?entry.status,
                                "resumed local model"
                            );
                        }
                    });
                });
        }

        crate::seed::agents::ensure_builtin_agents_seeded(&data_dir);
        crate::seed::templates::ensure_builtin_templates_seeded(&data_dir);

        let active_agent_id = Self::resolve_active_agent_id(&data_dir);

        let (cron_fire_tx, cron_fire_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let cron_scheduler_impl = Arc::new(rua_cron::TokioScheduler::new(Arc::new(
            move |job_id: String| {
                let _ = cron_fire_tx.send(job_id);
            },
        )));
        let cron_scheduler: Arc<dyn SystemScheduler> = cron_scheduler_impl.clone();

        let cron_jobs_path = data_dir
            .join("workspaces")
            .join(&active_agent_id)
            .join("jobs.json");
        let cron_cells = Arc::new(rua_cron::CronCells::new(
            Arc::clone(&cell_store),
            Arc::clone(&cron_scheduler),
            cron_jobs_path.clone(),
        ));

        let security_module = Arc::new(rua_security::SecurityModule::new(
            Arc::new(rua_security::SecurityConfigStore::new(data_dir.clone())),
            Some(Arc::new(rua_config::dirs::Paths {
                rua_root: data_dir.clone(),
                secret_dir: secret_dir.clone(),
                cache_dir: cache_dir.clone(),
            })),
        ));
        let plugin_registry = Arc::new(PluginRegistry::new());
        let active_plugin_registry = Arc::new(rua_plugin_runtime::ActivePluginRegistry::new());
        let plugin_skill_registry = Arc::new(rua_plugin_runtime::PluginSkillRegistry::new());

        let compactor = Arc::new(rua_agent::compaction::ConversationCompactor::new(
            Default::default(),
        ));
        let usage_recorder = Arc::new(UsageLedger::new(&data_dir));
        let usage_recorder_for_runtime: Arc<dyn rua_contract::UsageRecorder> =
            usage_recorder.clone();
        let mut task_runtime_config = rua_task::TaskRuntimeConfig::new(data_dir.clone());
        let local_device_id = hex::encode(device.device_id_short());
        task_runtime_config.device = rua_task::TaskDeviceContext {
            device_id: Some(local_device_id.clone()),
            device_name: crate::services::device_context::device_name_snapshot(
                &data_dir,
                &local_device_id,
            )
            .or_else(|| {
                Some(rua_sync::device_list::default_friendly_name(
                    &profile.to_string(),
                    &device.public_key().to_bytes(),
                ))
            }),
            host_profile: Some(profile.to_string()),
            authority_epoch: None,
        };
        task_runtime_config.limits = rua_task::TaskRuntimeLimits {
            max_active_tasks_per_chat: root_config.task_runtime.max_active_tasks_per_chat,
            max_concurrent_tasks_per_chat: root_config.task_runtime.max_concurrent_tasks_per_chat,
            max_concurrent_tasks_per_runner: root_config
                .task_runtime
                .max_concurrent_tasks_per_runner,
            activity_channel_capacity: root_config.task_runtime.activity_channel_capacity,
            max_result_output_chars: root_config.task_runtime.max_result_output_chars,
            max_raw_result_bytes: root_config.task_runtime.max_raw_result_bytes,
            max_attempt_history: root_config.task_runtime.max_attempt_history,
            max_dependencies_per_task: root_config.task_runtime.max_dependencies_per_task,
            max_dependency_depth: root_config.task_runtime.max_dependency_depth,
            cancellation_grace: std::time::Duration::from_secs(
                root_config.task_runtime.cancellation_grace_secs,
            ),
        };
        let task_notification_sink = Arc::new(
            crate::services::task_notification::ServerTaskNotificationSink::new(data_dir.clone()),
        );
        let task_runtime = Arc::new(
            rua_task::TaskRuntime::open_with_sink_and_authority(
                task_runtime_config,
                task_notification_sink.clone(),
                Arc::new(
                    crate::services::task_authority::ServerTaskAuthorityResolver::new(
                        data_dir.clone(),
                        profile.to_string(),
                        Arc::clone(&sync_device_cell),
                    ),
                ),
            )
            .expect("failed to initialize Task Runtime"),
        );
        let acp_permission_relay = Arc::new(rua_acp::permissions::AcpPermissionRelay::new());
        let product_tool_invoker = Arc::new(
            crate::services::product_tool_before::HandlerExecutorProductInvoker::new(
                Arc::clone(&active_plugin_registry),
                local_device_id.clone(),
            ),
        );
        let candidate_completion_runtime = Arc::new(
            crate::services::candidate_completion::CandidateCompletionRuntime::new(
                Arc::clone(&product_hooks),
                Arc::new(
                    crate::services::candidate_completion::HandlerExecutorCandidateCompletionInvoker::new(
                        Arc::clone(&active_plugin_registry),
                        local_device_id.clone(),
                    ),
                ),
            ),
        );
        let terminal_observer_runtime = Arc::new(
            crate::services::terminal_observer::TerminalObserverRuntime::new(
                Arc::clone(&product_hooks),
                Arc::new(
                    crate::services::terminal_observer::HandlerExecutorTerminalObserverInvoker::new(
                        Arc::clone(&active_plugin_registry),
                        local_device_id.clone(),
                    ),
                ),
            ),
        );
        conversation_authority.set_terminal_observer_runtime(terminal_observer_runtime);
        let tool_before_boundary: Arc<dyn react_agent::ToolBeforeBoundary> = Arc::new(
            crate::services::product_tool_before::ProductToolBeforeBoundary::new(
                Arc::clone(&product_hooks),
                product_tool_invoker.clone(),
            ),
        );
        let tool_observer_boundary: Arc<dyn react_agent::ToolObserverBoundary> = Arc::new(
            crate::services::product_tool_observer::ProductToolObserverBoundaryImpl::new(
                Arc::clone(&product_hooks),
                Arc::new(
                    crate::services::product_tool_observer::HandlerExecutorProductObserverInvoker::new(
                        Arc::clone(&product_tool_invoker),
                    ),
                ),
            ),
        );
        let permission_request_boundary: Arc<dyn react_agent::PermissionRequestBoundary> = Arc::new(
            crate::services::product_permission_request::ProductPermissionRequestBoundary::new(
                Arc::clone(&product_hooks),
                product_tool_invoker,
            ),
        );

        let collaboration_binding = Arc::new(std::sync::OnceLock::new());
        let sync_transfer_binding = Arc::new(std::sync::OnceLock::new());
        let collaboration_runtime: Arc<dyn std::any::Any + Send + Sync> = Arc::new(
            rua_tool::extended::collaboration::CollaborationRuntimeHandle::new(Arc::new(
                crate::services::collaboration_runtime::CollaborationRuntimeAdapter::new(
                    Arc::clone(&collaboration_binding),
                ),
            )),
        );
        let sync_transfer_runtime: Arc<dyn std::any::Any + Send + Sync> = Arc::new(
            rua_tool::extended::sync_transfer::SyncTransferRuntimeHandle::new(Arc::new(
                crate::services::sync_transfer_tools::SyncTransferRuntimeAdapter::new(Arc::clone(
                    &sync_transfer_binding,
                )),
            )),
        );
        let workspace_context = Arc::new(WorkspaceContext {
            agent_cells: Arc::clone(&agent_cells),
            agent_profile_store,
            provider_manager: Arc::clone(&provider_manager),
            plugin_registry: Arc::clone(&plugin_registry),
            active_plugin_registry: Arc::clone(&active_plugin_registry),
            security: Arc::clone(&security_module),
            task_runtime: Arc::clone(&task_runtime),
            collaboration_runtime: Arc::clone(&collaboration_runtime),
            sync_transfer_runtime,
            tool_config_store: Arc::clone(&tool_config_store),
            tool_before_boundary: Some(tool_before_boundary),
            tool_observer_boundary: Some(tool_observer_boundary),
            permission_request_boundary: Some(permission_request_boundary),
            rua_root: data_dir.clone(),
            secret_dir: secret_dir.clone(),
            cache_dir: cache_dir.clone(),
            compactor: Arc::clone(&compactor),
            usage_recorder: Some(usage_recorder_for_runtime),
            push_store_dir: Some(data_dir.clone()),
            agent_resolver: Arc::new(AgentResolver::new(&active_agent_id)),
            command_runtime_factory: std::sync::OnceLock::new(),
            session_replication_hook: std::sync::OnceLock::new(),
            memory_publication_hook: Arc::new(std::sync::OnceLock::new()),
        });

        let workspace_context_ref = Arc::clone(&workspace_context);

        let workspace_root = RuaRoot::from_paths(data_dir.clone(), secret_dir.clone());
        let active_workspace_root = workspace_root
            .agent_workspace(&active_agent_id)
            .expect("active Agent Workspace Root must be valid");
        let _layered_config = LayeredConfig::for_workspace(&data_dir, &active_workspace_root);
        let default_registry = Arc::new(DefaultWorkspaceRegistry::new(
            workspace_root.clone(),
            PlatformProfile::Desktop,
            &active_agent_id,
            workspace_context,
        ));
        // Agent shutdown must terminate real Conversation/Task work through its
        // owner rather than reporting a mirrored count as success. Installation
        // is synchronous because process assembly is: no runtime exists yet, so
        // there is nothing to retrofit and no need to enter an async context.
        let work_authority: Arc<dyn rua_workspace::AgentWorkAuthority> = Arc::new(
            crate::services::workspace_work_lifecycle::WorkspaceWorkAuthority::new(
                Arc::clone(&conversation_authority),
                Arc::clone(&streams),
                Arc::clone(&task_runtime),
                workspace_root.clone(),
                Some(Arc::clone(&project_catalog)),
            ),
        );
        default_registry.install_work_authority(work_authority);

        let acp_chat_registry =
            Arc::new(crate::services::acp_chat_registry::AcpChatRegistry::new());

        {
            use crate::control_adapters::*;

            let streams_f = Arc::clone(&streams);
            let registry_f = Arc::clone(&default_registry);
            let approval_authority = Arc::clone(security_module.approval_authority());
            let conversation_authority_f = Arc::clone(&conversation_authority);
            let dd_f = data_dir.clone();
            let cache_dir_f = cache_dir.clone();
            let acp_chat_registry_f = Arc::clone(&acp_chat_registry);
            let acp_permission_relay_f = Arc::clone(&acp_permission_relay);

            let factory: rua_contract::agent::CommandRuntimeFactory =
                Arc::new(move |agent_id: &str, workspace_dir: &std::path::Path| {
                    let acp_config =
                        crate::services::acp_chat_registry::load_acp_config(workspace_dir);
                    let acp_evaluator: Arc<dyn rua_acp::permissions::PermissionEvaluator> =
                        Arc::new(rua_tool::builtin::SecurityPermissionEvaluator::new());
                    let acp_service = Arc::new(
                        rua_acp::service::AcpService::with_runtime_cache_dir_and_permission_relay(
                            Arc::new(acp_config),
                            workspace_dir.to_string_lossy().to_string(),
                            cache_dir_f.clone(),
                            Some(acp_evaluator),
                            Arc::clone(&acp_permission_relay_f),
                            None,
                            agent_id.to_string(),
                        ),
                    );
                    let acp_chat_manager = Arc::new(rua_command::AcpChatSessionManager::new(
                        std::time::Duration::from_secs(3600),
                    ));
                    let acp_chat_controller: Arc<dyn rua_command::AcpChatController> =
                        Arc::new(rua_agent::acp_chat_controller::AcpChatControllerImpl::new(
                            Arc::clone(&acp_service),
                            Arc::clone(&acp_chat_manager),
                            workspace_dir.to_path_buf(),
                        ));

                    acp_chat_registry_f.register(
                        agent_id.to_string(),
                        Arc::new(crate::services::acp_chat_registry::AcpChatRuntimeHandle {
                            service: Arc::clone(&acp_service),
                            manager: Arc::clone(&acp_chat_manager),
                            controller: Arc::clone(&acp_chat_controller),
                            workspace_dir: workspace_dir.to_path_buf(),
                        }),
                    );

                    let cancel_adapter = Arc::new(
                        StreamCancelAdapter::new(Arc::clone(&streams_f), agent_id)
                            .with_acp(Arc::clone(&acp_chat_manager), Arc::clone(&acp_service)),
                    );
                    let approval_adapter = Arc::new(
                        ApprovalAdapter::new(Arc::clone(&approval_authority), agent_id)
                            .with_conversation_authority(Arc::clone(&conversation_authority_f)),
                    );
                    let reload_adapter = Arc::new(WorkspaceReloadAdapter::new(Arc::clone(
                        &registry_f,
                    )
                        as Arc<dyn WorkspaceRegistry>));
                    let skills_lister = Arc::new(SkillsListerAdapter::new(Some(
                        dd_f.join("workspaces").join(agent_id),
                    )));

                    let control_registry = rua_command::build_control_registry();

                    let daemon_registry = rua_command::build_daemon_registry();
                    let started_at = std::time::Instant::now();

                    let dispatcher = rua_agent::runtime::CommandRuntimeDispatcherAdapter::new(
                        daemon_registry,
                        control_registry,
                        cancel_adapter as Arc<dyn rua_command::SessionCanceller>,
                        approval_adapter as Arc<dyn rua_command::ApprovalService>,
                        Arc::new(NoopQueueClearer) as Arc<dyn rua_command::QueueClearer>,
                        skills_lister as Arc<dyn rua_command::SkillsLister>,
                        reload_adapter as Arc<dyn rua_command::WorkspaceReloader>,
                        acp_chat_manager,
                        acp_chat_controller,
                        acp_service,
                        dd_f.clone(),
                        workspace_dir.to_path_buf(),
                        started_at,
                        None,
                    );

                    rua_contract::agent::CommandRuntimeConfig {
                        dispatcher: Arc::new(dispatcher),
                    }
                });

            let _ = workspace_context_ref.command_runtime_factory.set(factory);
        }

        let usage_stats = UsageStats::load(&data_dir);
        drop(spawn_usage_flush(
            Arc::clone(&usage_stats),
            shutdown_tx.subscribe(),
        ));
        let workspace_registry: Arc<dyn WorkspaceRegistry> = default_registry.clone();
        let task_tool_execution: Arc<dyn rua_tool::ToolTaskExecution> = Arc::new(
            rua_workspace::WorkspaceToolTaskExecution::new(Arc::downgrade(&workspace_registry)),
        );
        task_runtime.register_runner(Arc::new(rua_tool::ToolTaskRunner::new(task_tool_execution)));

        let plugins: Arc<rua_plugin::PluginManager> = Arc::new(
            rua_plugin::PluginManager::with_persistence_store(
                data_dir.join("plugins.json"),
                Arc::clone(&cell_store),
            )
            .unwrap_or_default(),
        );

        {
            let pm = Arc::clone(&plugins);
            plugin_registry.set_enabled_checker(Arc::new(move |id| pm.is_enabled_sync(id)));
        }

        let channel_manager = Arc::new(ChannelManager::new());
        default_registry.install_channel_manager(&active_agent_id, Arc::clone(&channel_manager));

        let sync_enabled_flag =
            rua_sync::control_store::SyncControlStore::from_sync_dir(&sync_dir).read_enabled();

        let sync_enabled = Arc::new(AtomicBool::new(sync_enabled_flag));
        let sync_trigger = Arc::new(tokio::sync::RwLock::new(None));
        let sync_runtime = Arc::new(SyncRuntimeController::new(Arc::clone(&sync_trigger)));
        let sync_ns_cell: Arc<std::sync::RwLock<Option<Arc<rua_sync::identity::NetworkSecret>>>> =
            Arc::new(std::sync::RwLock::new(None));
        let sync_hlc_clock = Arc::new(std::sync::Mutex::new(None));
        let sync_transfer_session_manager_cell = Arc::new(std::sync::RwLock::new(
            None::<rua_device_network::DeviceSessionManager>,
        ));
        let user_notification_events = Arc::new(
            crate::services::console_user_notification::ConsoleUserNotificationAdapter::new(),
        );
        let user_notifications = Arc::new(
            crate::services::user_notification::UserNotificationService::new(
                local_device_id.clone(),
                Arc::clone(&user_notification_events)
                    as Arc<dyn crate::services::user_notification::LocalUserNotificationAdapter>,
                Arc::new(
                    crate::services::user_notification::DeviceSessionUserNotificationTransport::new(
                        Arc::clone(&sync_transfer_session_manager_cell),
                        Duration::from_secs(3),
                    ),
                ),
            ),
        );
        let chat_media_transfers =
            Arc::new(crate::services::chat_media_transfer::ChatMediaTransferRegistry::default());
        let device_network_availability = Arc::new(
            crate::services::device_network_availability::DeviceNetworkAvailability::new(),
        );
        let device_network_cell = Arc::new(
            crate::services::device_session_access::DeviceNetworkAccess::new(
                None,
                Arc::clone(&device_network_availability),
            ),
        );
        let active_stream_broadcast_hub =
            Arc::new(crate::services::active_stream_broadcast::ActiveStreamBroadcastHub::new());
        let chat_activity_wake_hub =
            crate::services::chat_activity_wake::ChatActivityWakeHub::new();
        active_stream_broadcast_hub
            .set_navigation_activity_hub(Arc::clone(&chat_activity_wake_hub));
        conversation_authority
            .set_active_stream_broadcast_hub(Arc::clone(&active_stream_broadcast_hub));
        conversation_authority
            .set_gossip_broadcast(Arc::clone(&device_network_cell), Arc::clone(&sync_ns_cell));
        crate::services::chat_activity_wake::spawn_chat_placement_convergence(
            Arc::clone(&chat_activity_wake_hub),
            Arc::clone(&conversation_authority),
            Arc::clone(&workspace_registry),
        );
        crate::services::chat_activity_wake::spawn_chat_activity_gossip_publisher(
            Arc::clone(&active_stream_broadcast_hub),
            Arc::clone(&device_network_cell),
            Arc::clone(&sync_ns_cell),
            shutdown_tx.subscribe(),
        );
        let terminal_authority = Arc::new(
            crate::services::terminal::TerminalSessionAuthority::new(
                crate::services::terminal::TerminalAuthorityConfig {
                    default_home: dirs::home_dir().unwrap_or_else(|| data_dir.clone()),
                    detached_ttl: Duration::from_secs(600),
                },
                Arc::new(crate::services::terminal::NativeTerminalProcessSpawner),
            )
            .with_active_stream_broadcast_hub(Arc::clone(&active_stream_broadcast_hub)),
        );
        let (remote_history_sync_tx, mut remote_history_sync_rx) =
            tokio::sync::mpsc::unbounded_channel::<
                crate::services::device_command_bus::ConversationHistorySyncRequest,
            >();
        if !sync_enabled_flag && !matches!(profile, ServerProfile::Test) {
            tracing::info!("[sync] disabled, skipping init");
        } else {
            let ns_opt = match rua_sync::identity::NetworkSecret::load(&sync_dir) {
                Ok(ns) => ns,
                Err(e) => {
                    tracing::warn!(error = %e, "[sync] failed to load network secret");
                    None
                }
            };

            if let Some(ns) = ns_opt {
                *sync_ns_cell.write().unwrap() = Some(Arc::new(ns));
            } else {
                tracing::info!("[sync] no network secret found, sync node not started");
            }
        }

        let sync_node_cell = Arc::new(tokio::sync::Mutex::new(
            None::<rua_sync::endpoint::SyncNode>,
        ));
        let terminal_remote_client: Arc<
            dyn crate::services::terminal::TerminalRemoteCommandClient,
        > = Arc::new(
            crate::services::terminal::DeviceSessionTerminalRemoteCommandClient::new(Arc::clone(
                &device_network_cell,
            )),
        );
        let terminal_remote_activity_client: Arc<
            dyn crate::services::terminal::TerminalRemoteActivityClient,
        > = Arc::new(
            crate::services::terminal::DeviceSessionTerminalRemoteActivityClient::new(Arc::clone(
                &device_network_cell,
            )),
        );
        {
            let runtime = crate::sync_publication::SyncPublicationRuntime::new(
                sync_dir.clone(),
                Arc::clone(&sync_enabled),
                Arc::clone(&sync_node_cell),
                Arc::clone(&sync_ns_cell),
                Arc::clone(&sync_device_cell),
                Some(Arc::clone(&sync_hlc_clock)),
                Arc::clone(&sync_trigger),
            );
            let _ = task_notification_sink.install_sync_runtime(runtime.clone());
            let hook = crate::services::memory_publication::SyncMemoryPublicationHook::new(runtime);
            let _ = workspace_context_ref.memory_publication_hook.set(hook);
        }
        let push_store = PushMessageStore::new(Some(data_dir.clone()));
        let user_inbox = Arc::new(
            crate::services::user_inbox::UserInbox::open(&data_dir)
                .expect("failed to open User Inbox"),
        );

        let agent_service: Arc<AgentManager> = Arc::new(
            AgentManager::new(data_dir.clone())
                .with_agent_mcp_cells(Arc::clone(&agent_mcp_cells))
                .with_agent_cells(Arc::clone(&agent_cells))
                .with_agent_tools_cells(Arc::clone(&agent_tools_cells)),
        );
        conversation_authority.set_chat_completion_notifier(Arc::new(
            crate::services::chat_completion_notification::ChatCompletionNotifier::new(
                local_device_id.clone(),
                data_dir.clone(),
                sync_dir.clone(),
                Arc::clone(&user_notifications),
                Arc::clone(&agent_service),
                Arc::clone(&collaboration_binding),
            ),
        ));

        let session_sync_emitter = SessionSyncEmitter::new(
            data_dir.clone(),
            Arc::clone(&sync_enabled),
            Arc::clone(&sync_node_cell),
            Arc::clone(&sync_ns_cell),
            Arc::clone(&sync_device_cell),
            Arc::clone(&sync_hlc_clock),
            Arc::clone(&sync_trigger),
        )
        .with_device_key(*device_key.as_bytes());
        {
            let hook = crate::services::session_replication::SyncSessionReplicationHook::new(
                session_sync_emitter.clone(),
            );
            let _ = workspace_context_ref.session_replication_hook.set(hook);
        }

        let console_chat: Arc<ConsoleChatService> = Arc::new({
            let mut svc = ConsoleChatService::new(
                Arc::clone(&workspace_registry),
                Arc::clone(&streams),
                Arc::clone(&sync_device_cell),
                profile.to_string(),
            )
            .with_push_store(push_store.clone())
            .with_agent_service(Arc::clone(&agent_service))
            .with_provider_manager(Arc::clone(&provider_manager))
            .with_conversation_authority(Arc::clone(&conversation_authority))
            .with_acp_chat_registry(Arc::clone(&acp_chat_registry))
            .with_project_bindings(Arc::clone(&project_bindings))
            .with_tool_config_context(
                Arc::clone(&agent_tools_cells),
                device_key_bytes,
                Arc::clone(&tool_catalog),
            );
            svc = svc.with_session_sync_emitter(session_sync_emitter.clone());
            svc
        });

        let chat_service: Arc<UsageRecordingChatService> =
            Arc::new(UsageRecordingChatService::new(console_chat));

        let mut conversation_input_intake =
            crate::services::conversation_input_intake::ConversationInputIntake::new(
                Arc::clone(&chat_service),
                Arc::clone(&streams),
                Arc::clone(&workspace_registry),
                session_sync_emitter.clone(),
                Arc::clone(&sync_device_cell),
                Some(Arc::clone(&acp_chat_registry)),
                Some(Arc::clone(&active_stream_broadcast_hub)),
                crate::services::remote_conversation_input::RemoteConversationInputDelivery::new(
                    Arc::clone(&sync_node_cell),
                    Arc::clone(&device_network_cell),
                    data_dir.clone(),
                ),
            )
            .with_agent_service(Arc::clone(&agent_service))
            .with_provider_manager(Arc::clone(&provider_manager))
            .with_provider_cells(Arc::clone(&provider_cells))
            .with_conversation_authority(Arc::clone(&conversation_authority))
            .with_collaboration_binding(Arc::clone(&collaboration_binding))
            .with_product_hooks(Arc::clone(&product_hooks))
            .with_candidate_completion_runtime(candidate_completion_runtime)
            .with_project_runtime(
                data_dir.clone(),
                Arc::clone(&project_catalog),
                Arc::clone(&project_bindings),
            )
            .with_security(Arc::clone(&security_module))
            .with_approval_authority(Arc::clone(security_module.approval_authority()))
            .with_acp_permission_relay(Arc::clone(&acp_permission_relay))
            .with_task_runtime(Arc::clone(&task_runtime));
        task_notification_sink.install_tasks(&task_runtime);
        conversation_input_intake = conversation_input_intake
            .with_task_notifications(Arc::downgrade(&task_notification_sink));
        let _ = task_notification_sink.install_watch_delivery(conversation_input_intake.clone());
        conversation_authority.install_background_work(Arc::new(
            crate::services::background_work::BackgroundWork::new(
                data_dir.clone(),
                Arc::clone(&task_runtime),
                Arc::downgrade(&conversation_authority),
            ),
        ));

        let console_facade: Arc<dyn ConsoleChatFacade> = Arc::new(
            ConsoleChatFacadeImpl::new(
                conversation_input_intake.clone(),
                Arc::clone(&streams),
                Arc::clone(&workspace_registry),
                Arc::clone(&sync_device_cell),
                Arc::clone(&sync_node_cell),
                Arc::clone(&device_network_cell),
                data_dir.clone(),
            )
            .with_conversation_authority(Arc::clone(&conversation_authority))
            .with_project_catalog(Arc::clone(&project_catalog))
            .with_history_sync(remote_history_sync_tx.clone())
            .with_chat_activity_wake_hub(Arc::clone(&chat_activity_wake_hub))
            .with_acp_chat_registry(Arc::clone(&acp_chat_registry))
            .with_active_stream_broadcast_hub(Arc::clone(&active_stream_broadcast_hub)),
        );

        {
            let ns = sync_ns_cell.read().ok().and_then(|guard| guard.clone());
            let device = sync_device_cell.read().ok().and_then(|guard| guard.clone());
            if let (Some(ns), Some(device)) = (ns, device) {
                let local_device_id = hex::encode(device.device_id_short());
                let command_handler = Arc::new(
                    crate::services::device_command_bus::DeviceCommandHandler::new(
                        data_dir.join("device_commands"),
                        local_device_id,
                    )
                    .with_remote_history_sync(remote_history_sync_tx.clone())
                    .with_acp_chat_registry(
                        Arc::clone(&acp_chat_registry),
                        Arc::clone(&workspace_registry),
                    )
                    .with_acp_permission_relay(Arc::clone(&acp_permission_relay))
                    .with_remote_chat_control_streams(Arc::clone(&streams))
                    .with_conversation_input_intake(conversation_input_intake.clone())
                    .with_collaboration_binding(Arc::clone(&collaboration_binding))
                    .with_conversation_authority(Arc::clone(&conversation_authority))
                    .with_terminal_authority(Arc::clone(&terminal_authority))
                    .with_sync_transfer_session_manager(Arc::clone(
                        &sync_transfer_session_manager_cell,
                    ))
                    .with_chat_media_transfers(Arc::clone(&chat_media_transfers))
                    .with_project_resource_editor(
                        crate::services::project_lifecycle::ProjectResourceEditor::new(
                            data_dir.clone(),
                            Arc::clone(&project_catalog),
                            Arc::clone(&project_bindings),
                            Arc::clone(&conversation_authority),
                            Arc::clone(security_module.approval_authority()),
                            Arc::clone(&task_runtime),
                            crate::sync_publication::SyncPublicationRuntime::new(
                                sync_dir.clone(),
                                Arc::clone(&sync_enabled),
                                Arc::clone(&sync_node_cell),
                                Arc::clone(&sync_ns_cell),
                                Arc::clone(&sync_device_cell),
                                Some(Arc::clone(&sync_hlc_clock)),
                                Arc::clone(&sync_trigger),
                            ),
                        ),
                    )
                    .with_project_runtime(
                        Arc::clone(&project_catalog),
                        Arc::clone(&project_bindings),
                        crate::sync_publication::SyncPublicationRuntime::new(
                            sync_dir.clone(),
                            Arc::clone(&sync_enabled),
                            Arc::clone(&sync_node_cell),
                            Arc::clone(&sync_ns_cell),
                            Arc::clone(&sync_device_cell),
                            Some(Arc::clone(&sync_hlc_clock)),
                            Arc::clone(&sync_trigger),
                        ),
                    )
                    .with_task_runtime(Arc::clone(&task_runtime))
                    .with_user_notifications(Arc::clone(&user_notifications))
                    .with_foreground_operations(Arc::clone(&foreground_operations)),
                );

                let rt_handle = tokio::runtime::Handle::try_current().ok();
                let sd = sync_dir.clone();
                let active_stream_broadcast_hub_for_node = Arc::clone(&active_stream_broadcast_hub);
                let conversation_authority_for_live = Arc::clone(&conversation_authority);
                let conversation_input_intake_for_live = conversation_input_intake.clone();
                let chat_activity_wake_hub_for_node = Arc::clone(&chat_activity_wake_hub);
                let sync_ns_cell_for_node = Arc::clone(&sync_ns_cell);
                let sync_transfer_session_manager_for_node =
                    Arc::clone(&sync_transfer_session_manager_cell);
                let foreground_operations_for_node = Arc::clone(&foreground_operations);
                let chat_media_transfers_for_node = Arc::clone(&chat_media_transfers);
                let chat_media_root_for_node = data_dir.clone();
                let sync_node_init = std::thread::spawn(move || {
                    block_on_with_optional_handle(rt_handle, async move {
                        let sync_blob_store =
                            rua_sync::sync_blob_store::build_sync_blob_store_from_dir(&sd).await;
                        let blobs_api: Option<rua_sync::endpoint::BlobsApiStore> =
                            sync_blob_store.as_ref().map(|s| s.api_store());

                        match rua_sync::endpoint::SyncNode::bind_with_command_handler(
                            &sd,
                            ns.namespace_secret(),
                            &device,
                            blobs_api,
                            command_handler,
                        )
                        .await
                        {
                            Ok(node) => {
                                node.network_runtime()
                                    .session_manager()
                                    .set_live_activity_handler(Arc::new(
                                        crate::services::active_stream_broadcast::ActiveStreamBroadcastDeviceSessionHandler::new(
                                            Arc::clone(&active_stream_broadcast_hub_for_node),
                                        )
                                        .with_conversation_control_initial_state(
                                            conversation_authority_for_live,
                                            conversation_input_intake_for_live,
                                            hex::encode(device.device_id_short()),
                                        )
                                        .with_foreground_operations(Arc::clone(
                                            &foreground_operations_for_node,
                                        )),
                                    ));
                                node.network_runtime()
                                    .session_manager()
                                    .set_bulk_transfer_handler(Arc::new(
                                        crate::services::sync_transfer::SyncTransferBulkHandler::new(
                                            hex::encode(device.device_id_short()),
                                        )
                                        .with_foreground_operations(Arc::clone(
                                            &foreground_operations_for_node,
                                        ))
                                        .with_media_transfers(chat_media_transfers_for_node)
                                        .with_chat_media_root(chat_media_root_for_node)
                                        .with_sync_blobs(node.blobs_store().clone()),
                                    ));
                                crate::services::chat_activity_wake::spawn_chat_activity_gossip_listener(
                                    node.network_runtime().clone(),
                                    Arc::clone(&chat_activity_wake_hub_for_node),
                                    Arc::clone(&sync_ns_cell_for_node),
                                );
                                *sync_transfer_session_manager_for_node.write().unwrap() =
                                    Some(node.network_runtime().session_manager().clone());
                                Some(node)
                            }
                            Err(error) => {
                                tracing::warn!(error = %error, "[sync] SyncNode bind failed");
                                None
                            }
                        }
                    })
                })
                .join()
                .expect("sync-node bootstrap thread panicked");
                if let Some(node) = sync_node_init {
                    if let Ok(mut network_guard) = device_network_cell.try_lock() {
                        *network_guard = Some(node.network_runtime().clone());
                    }
                    if let Ok(mut guard) = sync_node_cell.try_lock() {
                        *guard = Some(node);
                    }
                }
            }
        }

        let local_model = rua_local_model::LocalModelManager::init_instance(
            data_dir.clone(),
            profile.scan_local_models(),
        );

        {
            let local = Arc::clone(&local_model);
            let pm = Arc::clone(&provider_manager);
            let cells = Arc::clone(&provider_cells);
            tokio::spawn(async move {
                crate::local_model_resume::resume_on_startup(local, pm, cells).await;
            });
        }

        let skill_service = Arc::new({
            let svc = FileSkillService::new(data_dir.clone())
                .with_workspace_skill_cells(Arc::clone(&workspace_skill_cells))
                .with_plugin_skill_registry(Arc::clone(&plugin_skill_registry));
            let svc = if let Some(builtin_dir) = find_builtin_skills_dir() {
                svc.with_builtin_skills_dir(builtin_dir)
            } else {
                svc
            };
            svc.initialize_pool();
            svc
        });

        {
            let cm = Arc::clone(&channel_manager);
            let streams_for_ctrl = Arc::clone(&streams);
            let dd = data_dir.clone();
            let reg_for_ctrl = Arc::clone(&workspace_registry);
            let approval_for_ctrl = Arc::clone(security_module.approval_authority());
            let conversation_authority_for_ctrl = Arc::clone(&conversation_authority);
            let agent_id_for_ctrl = active_agent_id.clone();
            let runtime_handle = tokio::runtime::Handle::try_current().ok();
            let _ = std::thread::spawn(move || {
                block_on_with_optional_handle(runtime_handle, async move {
                    use crate::control_adapters::*;

                    let cancel_adapter =
                        StreamCancelAdapter::new(Arc::clone(&streams_for_ctrl), &agent_id_for_ctrl);
                    let cancel_adapter: Arc<StreamCancelAdapter> = Arc::new(cancel_adapter);
                    let approval_adapter: Arc<ApprovalAdapter> = Arc::new(
                        ApprovalAdapter::new(Arc::clone(&approval_for_ctrl), &agent_id_for_ctrl)
                            .with_conversation_authority(Arc::clone(
                                &conversation_authority_for_ctrl,
                            )),
                    );
                    let reload_adapter: Arc<WorkspaceReloadAdapter> =
                        Arc::new(WorkspaceReloadAdapter::new(Arc::clone(&reg_for_ctrl)));
                    let noop_queue: Arc<NoopQueueClearer> = Arc::new(NoopQueueClearer);
                    let skills_lister: Arc<SkillsListerAdapter> =
                        Arc::new(SkillsListerAdapter::new(Some(
                            dd.join("workspaces").join(&agent_id_for_ctrl),
                        )));
                    let started_at = std::time::Instant::now();

                    let channel_ctrl_ctx =
                        Arc::new(rua_channel::core::control_handlers::ControlContext {
                            session_canceller: Arc::clone(&cancel_adapter)
                                as Arc<dyn rua_channel::core::control_handlers::SessionCanceller>,
                            workspace_reloader: Arc::clone(&reload_adapter)
                                as Arc<dyn rua_channel::core::control_handlers::WorkspaceReloader>,
                            approval_service: Arc::clone(&approval_adapter)
                                as Arc<dyn rua_channel::core::control_handlers::ApprovalService>,
                            started_at,
                            data_dir: dd.clone(),
                            exit_fn: None,
                        });
                    cm.set_control_context(channel_ctrl_ctx).await;

                    let control_registry = rua_command::build_control_registry();

                    let daemon_registry = rua_command::build_daemon_registry();

                    let dispatcher = Arc::new(rua_command::CommandDispatcher::new(
                        daemon_registry,
                        control_registry,
                        rua_command::ConversationCommandRegistry::new(),
                    ));

                    cm.set_command_dispatcher(
                        dispatcher,
                        cancel_adapter as Arc<dyn rua_command::SessionCanceller>,
                        Arc::clone(&approval_adapter) as Arc<dyn rua_command::ApprovalService>,
                        noop_queue as Arc<dyn rua_command::QueueClearer>,
                        skills_lister as Arc<dyn rua_command::SkillsLister>,
                        dd,
                        started_at,
                        reload_adapter as Arc<dyn rua_command::WorkspaceReloader>,
                        None,
                    )
                    .await;

                    tracing::info!("[control] control + daemon command path wired");
                });
            })
            .join();
        }

        let platform_initialization = {
            let reg = Arc::clone(&workspace_registry);
            let agent_id = active_agent_id.clone();
            let cm = Arc::clone(&channel_manager);
            let dd = data_dir.clone();
            let ac = Arc::clone(&agent_cells);
            tokio::spawn(async move {
                let channels_value = ac
                    .section::<serde_json::Value>(&agent_id, "channels")
                    .read()
                    .await
                    .map_err(|error| {
                        format!("failed to read channels from agent cell store: {error}")
                    })?;
                let initial_channels = channels_value
                    .clone()
                    .unwrap_or_else(|| serde_json::json!({}));
                let im_configs = load_im_platform_configs(channels_value);
                let workspace_rt = match reg.get_or_create(&agent_id).await {
                    Ok(rt) => rt,
                    Err(e) => {
                        tracing::error!(
                            error = %e,
                            "failed to get workspace runtime for platform services"
                        );
                        return Err(e.to_string());
                    }
                };

                let react_rt: Arc<dyn AgentRuntime> = match workspace_rt.react_runtime().await {
                    Some(rt) => rt,
                    None => {
                        tracing::error!("react_runtime not available for platform services");
                        return Err("react_runtime not available for platform services".to_string());
                    }
                };

                let ws_dir = dd.join("workspaces").join(&agent_id);
                let skill_svc = match rua_skills::SkillService::new(&ws_dir) {
                    Ok(svc) => Arc::new(svc),
                    Err(e) => {
                        tracing::error!(error = %e, "failed to create SkillService");
                        return Err(e.to_string());
                    }
                };

                workspace_rt
                    .service_manager()
                    .register_platform_services(
                        PlatformProfile::Desktop,
                        skill_svc,
                        cm,
                        im_configs,
                        Some(react_rt),
                    )
                    .await;

                // The workspace already finished its Runtime phase before
                // platform capabilities were attached, so start them now.
                if let Err(e) = workspace_rt
                    .service_manager()
                    .start_phase(rua_workspace::InitPhase::Runtime)
                    .await
                {
                    tracing::warn!(
                        error = %e,
                        "platform service start reported failures"
                    );
                    return Err(e.to_string());
                } else {
                    workspace_rt.record_initial_channels(initial_channels).await;
                }

                tracing::info!("[workspace] platform services registered (channels→skills)");
                Ok(())
            })
        };

        let voice_manager = Arc::new(rua_voice::VoiceManager::with_dirs(
            uuid::Uuid::new_v4().to_string(),
            data_dir.clone(),
            secret_dir.clone(),
            Arc::clone(&workspace_registry),
            active_agent_id.clone(),
        )?);
        {
            let voice_channel = Arc::new(rua_voice::VoiceChannel::new(Arc::clone(&voice_manager)));
            let cm = Arc::clone(&channel_manager);
            let runtime_handle = tokio::runtime::Handle::try_current().ok();
            let _ = std::thread::spawn(move || {
                block_on_with_optional_handle(runtime_handle, async {
                    if let Err(e) = cm.register(voice_channel).await {
                        tracing::error!(error = %e, "failed to register VoiceChannel");
                    }
                });
            })
            .join();
        }

        crate::cron_bootstrap::spawn_cron_scheduler(
            data_dir.clone(),
            Arc::clone(&workspace_registry),
            Arc::clone(&cron_scheduler_impl),
            cron_fire_rx,
            push_store.clone(),
        );
        let sidecar_store = Arc::new(
            rua_sync::sidecar::SidecarStore::open(&sync_dir)
                .expect("SidecarStore open must succeed"),
        );
        let inbound_inbox = Arc::new(
            rua_sync::inbound_inbox::InboundInbox::open(&sync_dir)
                .expect("InboundInbox open must succeed"),
        );
        let sync_stage_recorder =
            Arc::new(rua_sync::stage_diagnostics::SyncStageRecorder::default());
        let inbound_apply_lock = Arc::new(tokio::sync::Mutex::new(()));
        let sync_host_profile = profile.sync_runtime_host_profile();
        let sync_power_policy = Arc::new(std::sync::RwLock::new(
            match rua_sync::power_policy::LoadedSyncPowerPolicy::load(&data_dir, sync_host_profile)
            {
                Ok(policy) => policy,
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "[sync-power-policy] failed to load policy; using host defaults"
                    );
                    let fallback = rua_sync::power_policy::SyncPowerPolicyConfig::default_for_host(
                        sync_host_profile,
                    );
                    let effective_policy = fallback
                        .normalize(sync_host_profile)
                        .expect("host default sync power policy is valid");
                    rua_sync::power_policy::LoadedSyncPowerPolicy {
                        host_profile: sync_host_profile,
                        stored_policy: None,
                        effective_policy,
                    }
                }
            },
        ));

        let sync_transfer_jobs = Arc::new(
            crate::services::sync_transfer::TransferJobStore::with_foreground_operations(
                Arc::clone(&foreground_operations),
            ),
        );
        let sync_transfer_tool_service = Arc::new(
            crate::services::sync_transfer_tools::SyncTransferToolService::new(
                Arc::clone(&sync_enabled),
                Arc::clone(&sync_device_cell),
                Arc::clone(&device_network_cell),
                Arc::clone(&sync_transfer_jobs),
            ),
        );
        let _ = sync_transfer_binding.set(sync_transfer_tool_service);

        let collaboration = Arc::new(
            crate::services::collaboration::CollaborationService::new(
                data_dir.clone(),
                Arc::downgrade(&workspace_registry),
                Arc::clone(&agent_service),
                Arc::clone(&product_hooks),
                conversation_input_intake.clone(),
                hex::encode(
                    sync_device_cell
                        .read()
                        .expect("device identity lock")
                        .as_ref()
                        .expect("Device identity initialized")
                        .device_id_short(),
                ),
                4,
            )
            .with_device_network(Arc::clone(&device_network_cell))
            .with_sync_device(Arc::clone(&sync_device_cell))
            .with_progress_summarizer(Arc::new(
                crate::services::collaboration_progress::ProviderCollaborationProgressSummarizer::new(
                    Arc::clone(&provider_manager),
                ),
            )),
        );
        assert!(
            collaboration_binding
                .set(Arc::downgrade(&collaboration))
                .is_ok(),
            "collaboration service initialized once"
        );
        if !matches!(profile, ServerProfile::Test) {
            preheat_workspace_registry(Arc::clone(&default_registry), active_agent_id.clone());
        }
        preheat_used_workspace_runtimes(
            default_registry,
            Arc::clone(&usage_stats),
            active_agent_id.clone(),
        );
        let state = Self {
            platform_initialization: Arc::new(tokio::sync::Mutex::new(PlatformInitialization {
                task: Some(platform_initialization),
                result: None,
            })),
            config: Arc::new(config),
            auth: Arc::new(AuthService::from_secret_dir(&secret_dir)),
            registry: workspace_registry,
            task_runtime,
            acp_permission_relay,
            streams: Arc::clone(&streams),
            conversation_authority: Arc::clone(&conversation_authority),
            product_hooks,
            collaboration,
            collaboration_runtime,
            conversation_input_intake: conversation_input_intake.clone(),
            active_stream_broadcast_hub: Arc::clone(&active_stream_broadcast_hub),
            chat_activity_wake_hub,
            terminal_authority: Arc::clone(&terminal_authority),
            terminal_remote_client,
            terminal_remote_activity_client,
            agent_service,
            provider_manager: Arc::clone(&provider_manager),
            providers_loader,
            cell_store: Arc::clone(&cell_store),
            provider_cells: Arc::clone(&provider_cells),
            shared_mcp_cells: Arc::clone(&shared_mcp_cells),
            cron_cells: Arc::clone(&cron_cells),
            cron_scheduler,
            agent_cells: Arc::clone(&agent_cells),
            agent_mcp_cells: Arc::clone(&agent_mcp_cells),
            agent_tools_cells: Arc::clone(&agent_tools_cells),
            tool_catalog,
            tool_config_store,
            project_catalog: Arc::clone(&project_catalog),
            project_bindings: Arc::clone(&project_bindings),
            chat_service,
            llm_debug_captures: Arc::new(
                crate::services::llm_debug_capture::LlmDebugCaptureRegistry::new(&data_dir),
            ),
            console_facade,
            skill_service,
            local_model,
            plugins,
            plugin_registry: Arc::clone(&plugin_registry),
            active_plugin_registry: Arc::clone(&active_plugin_registry),
            plugin_skill_registry: Arc::clone(&plugin_skill_registry),
            qrcode_registry: {
                let reg = Arc::new(QRCodeHandlerRegistry::new());
                reg.register_blocking("dingtalk", DingtalkQRCodeAuthHandler::new().into_arc());
                reg.register_blocking("feishu", FeishuQRCodeAuthHandler::new().into_arc());
                reg.register_blocking("qq", QqQRCodeAuthHandler::new().into_arc());
                reg.register_blocking("wecom", WecomQRCodeAuthHandler::new().into_arc());
                reg.register_blocking("wechat", WeixinQRCodeAuthHandler::new().into_arc());
                reg.register_blocking("weixin", WeixinQRCodeAuthHandler::new().into_arc());
                reg
            },
            push_message_store: push_store,
            user_inbox,
            voice: Arc::clone(&voice_manager),
            voice_runtime: Arc::default(),
            tunnel_driver: Arc::new(tokio::sync::Mutex::new(None)),
            tunnel_alert_state: Arc::new(tokio::sync::RwLock::new(TunnelAlertState::default())),
            lifecycle: Arc::new(LifecycleManager::new()),
            desktop_automation: Arc::new(crate::desktop_automation::DesktopAutomationRuntime::new(
                rua_mcp::bundled_computer::BundledComputer::from_runtime(
                    &rua_core::runtime_authority::RuntimeAuthority::from_current_process(
                        Some(&profile.to_string()),
                        &data_dir,
                        &cache_dir,
                    ),
                    &cache_dir,
                ),
            )),
            profile,
            ready: Arc::new(AtomicBool::new(false)),
            shutdown_tx: Arc::new(shutdown_tx),
            rua_root: data_dir,
            secret_dir: secret_dir.clone(),
            cache_dir,
            usage_stats,
            mcp_tools_cache: McpToolsCache::new(),
            pairing_state: Arc::new(tokio::sync::Mutex::new(
                crate::endpoints::pairing::PairingState::default(),
            )),
            security: security_module,
            device_key,
            device_network: device_network_cell,
            device_network_availability,
            user_notifications,
            user_notification_events,
            sync_node: sync_node_cell,
            sync_transfer_session_manager: sync_transfer_session_manager_cell,
            chat_media_transfers,
            sync_trigger,
            sync_runtime,
            sync_enabled,
            sync_control_lock: Arc::new(tokio::sync::Mutex::new(())),
            sync_power_policy,
            sync_network_secret: sync_ns_cell,
            sync_device: sync_device_cell,
            remote_history_sync_tx,
            sync_hlc_clock,
            commands_metadata: Arc::new(
                crate::endpoints::commands_discovery::build_commands_metadata(),
            ),
            sidecar_store,
            inbound_inbox,
            sync_stage_recorder,
            inbound_apply_lock,
            sync_repair_scheduler: Arc::new(std::sync::Mutex::new(
                SyncRepairSchedulerState::default(),
            )),
            sync_transfer_jobs,
            foreground_operations,
            acp_chat_registry,
        };

        {
            let history_context = SyncRekeyRuntimeContext {
                rua_root: state.rua_root.clone(),
                secret_dir: state.secret_dir.clone(),
                device_key: *state.device_key.as_bytes(),
                sync_node: Arc::downgrade(&state.sync_node),
                sync_network_secret: Arc::downgrade(&state.sync_network_secret),
                sync_device: Arc::downgrade(&state.sync_device),
                sidecar_store: Arc::downgrade(&state.sidecar_store),
                provider_manager: Arc::downgrade(&state.provider_manager),
                security: Arc::downgrade(&state.security),
                project_catalog: Arc::downgrade(&state.project_catalog),
                registry: Arc::downgrade(&state.registry),
                inbound_inbox: Arc::downgrade(&state.inbound_inbox),
                inbound_apply_lock: Arc::downgrade(&state.inbound_apply_lock),
            };
            tokio::spawn(async move {
                while let Some(request) = remote_history_sync_rx.recv().await {
                    let result = remote_conversation_history_sync_from_context(
                        &history_context,
                        &request.source_device_id,
                    )
                    .await;
                    let _ = request.result_tx.send(result);
                }
            });
        }

        state.spawn_foreground_operation_lifecycle();

        if sync_enabled_flag {
            let state_for_runtime = state.clone();
            tokio::spawn(async move {
                if let Err(e) = state_for_runtime
                    .sync_runtime
                    .resume_foreground(|| async {
                        state_for_runtime.build_sync_runtime_parts().await
                    })
                    .await
                {
                    tracing::warn!(
                        reason = e.reason(),
                        detail = e.detail(),
                        "[sync-runtime] failed to start runtime at startup"
                    );
                }
            });
        }

        crate::plugin_bootstrap::load_persisted_plugins(&state);

        crate::tunnel_bootstrap::spawn_tunnel_event_subscriber(
            Arc::clone(&state.tunnel_driver),
            Arc::clone(&state.tunnel_alert_state),
            channel_manager,
        );

        Ok(state)
    }

    pub async fn wait_platform_initialization(&self) -> anyhow::Result<()> {
        let mut initialization = self.platform_initialization.lock().await;
        if let Some(result) = initialization.result.as_ref() {
            return result.clone().map_err(anyhow::Error::msg);
        }
        if let Some(task) = initialization.task.as_mut() {
            let result = match task.await {
                Ok(result) => result,
                Err(error) => Err(format!("Platform initialization task failed: {error}")),
            };
            initialization.task = None;
            initialization.result = Some(result.clone());
            return result.map_err(anyhow::Error::msg);
        }
        Err(anyhow::anyhow!(
            "Platform initialization has no task or result"
        ))
    }

    pub fn from_tempdir(data_dir: PathBuf, profile: ServerProfile) -> Self {
        if !data_dir.join("config.json").exists() {
            rua_config::save_config(
                &rua_config::RootConfig::default(),
                &data_dir.join("config.json"),
            )
            .expect("initialize test root configuration");
        }
        let secret_dir = data_dir.join("secret");
        std::fs::create_dir_all(&secret_dir).expect("create secret_dir");
        Self::load(AppConfig::default(), data_dir, secret_dir, profile)
            .expect("valid test AppState configuration")
    }

    pub fn is_ready(&self) -> bool {
        self.ready.load(Ordering::SeqCst)
    }

    pub fn set_ready(&self) {
        self.ready.store(true, Ordering::SeqCst);
    }

    pub fn shutdown_rx(&self) -> tokio::sync::watch::Receiver<bool> {
        self.shutdown_tx.subscribe()
    }

    pub fn signal_shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }

    pub fn pairing_state(
        &self,
    ) -> Arc<tokio::sync::Mutex<crate::endpoints::pairing::PairingState>> {
        Arc::clone(&self.pairing_state)
    }

    pub async fn mcp_manager_for(&self, agent_id: &str) -> Option<Arc<dyn McpManager>> {
        let rt = self.registry.get_or_create(agent_id).await.ok()?;
        rt.mcp_manager().await
    }

    pub async fn memory_manager_for(
        &self,
        agent_id: &str,
    ) -> Option<Arc<dyn rua_agent::memory_manager::MemoryManager>> {
        let rt = self.registry.get_or_create(agent_id).await.ok()?;
        rt.memory_manager().await
    }

    pub async fn channel_manager_for(&self, agent_id: &str) -> Option<Arc<ChannelManager>> {
        let rt = self.registry.get_or_create(agent_id).await.ok()?;
        rt.channel_manager().await
    }

    pub async fn compactor_for(
        &self,
        agent_id: &str,
    ) -> Option<Arc<rua_agent::compaction::ConversationCompactor>> {
        let rt = self.registry.get_or_create(agent_id).await.ok()?;
        rt.compactor().await
    }

    pub fn resolve_active_agent_id(data_dir: &std::path::Path) -> String {
        let mut active_id = "default".to_string();

        let config_json = data_dir.join("config.json");
        if let Ok(bytes) = std::fs::read(&config_json) {
            if let Ok(root) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                if let Some(agents_val) = root.get("agents") {
                    if let Ok(agents_cfg) =
                        serde_json::from_value::<rua_config::AgentsConfig>(agents_val.clone())
                    {
                        active_id = agents_cfg.active_agent;
                    }
                }
            }
        }

        let mut available: Vec<String> = Vec::new();
        let ws_dir = data_dir.join("workspaces");
        if ws_dir.exists() {
            if let Ok(entries) = std::fs::read_dir(&ws_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if !path.is_dir() {
                        continue;
                    }
                    let agent_json = path.join("agent.json");
                    let bytes = match std::fs::read(&agent_json) {
                        Ok(b) => b,
                        Err(_) => continue,
                    };
                    match serde_json::from_slice::<rua_config::AgentProfileConfig>(&bytes) {
                        Ok(cfg) => available.push(cfg.id),
                        Err(e) => {
                            tracing::warn!(
                                path = %agent_json.display(),
                                error = %e,
                                "Failed to parse agent.json, skipping"
                            );
                        }
                    }
                }
            }
        }

        if available.iter().any(|id| id == &active_id) {
            active_id
        } else if let Some(first) = available.into_iter().next() {
            first
        } else {
            "default".to_string()
        }
    }

    pub async fn spawn_liveactor_subscriber(
        &self,
        _secret_dir: &std::path::Path,
    ) -> Result<rua_sync::liveactor_apply::LiveApplyHandle, SyncRuntimeError> {
        let node = self
            .sync_node
            .lock()
            .await
            .as_ref()
            .cloned()
            .ok_or_else(|| SyncRuntimeError::not_paired("sync node is not initialized"))?;
        let docs = node.docs().clone();
        let namespace_id = node.namespace_id();

        let inbound_runtime = crate::sync_inbound_apply::InboundApplyRuntime::from_state(self);

        let apply_fn: rua_sync::liveactor_apply::ApplyFn = Arc::new(move |mode| {
            let inbound_runtime = inbound_runtime.clone();
            Box::pin(async move { inbound_runtime.apply_liveactor_mode(mode).await })
        });

        let handle = rua_sync::liveactor_apply::subscribe_and_apply(docs, namespace_id, apply_fn);
        Ok(handle)
    }

    pub async fn build_sync_runtime_parts(&self) -> Result<SyncRuntimeParts, SyncRuntimeError> {
        self.ensure_device_network_runtime().await?;

        let sync_node = Arc::clone(&self.sync_node);
        let sync_dir = self.rua_root.join("sync");
        let sync_fn: rua_sync::trigger::SyncCycleFn = Arc::new(move |_reason| {
            let sync_node = Arc::clone(&sync_node);
            let sync_dir = sync_dir.clone();
            Box::pin(async move { crate::sync_cycle::run_sync_cycle(&sync_node, &sync_dir).await })
        });

        let scheduler =
            rua_sync::trigger::SyncScheduler::start(sync_fn, self.sync_trigger_config());
        let liveactor = self.spawn_liveactor_subscriber(&self.secret_dir).await?;
        Ok(SyncRuntimeParts::new(scheduler, Some(liveactor)))
    }

    pub async fn ensure_device_network_runtime(&self) -> Result<(), SyncRuntimeError> {
        if !self.sync_enabled.load(Ordering::SeqCst) {
            return Err(SyncRuntimeError::not_paired("sync is disabled"));
        }
        if self.sync_node.lock().await.is_some() {
            self.device_network_availability.mark_ready();
            return Ok(());
        }
        self.device_network_availability.mark_starting();
        let result = async {
            if !self.sync_enabled.load(Ordering::SeqCst) {
                return Err(SyncRuntimeError::not_paired("sync is disabled"));
            }

            let sync_dir = self.rua_root.join("sync");
            let network_secret = rua_sync::identity::NetworkSecret::load(&sync_dir)
                .map_err(|error| {
                    SyncRuntimeError::unavailable(format!("load network secret: {error}"))
                })?
                .ok_or_else(|| SyncRuntimeError::not_paired("sync network is not paired"))?;
            let device = self
                .sync_device
                .read()
                .ok()
                .and_then(|guard| guard.as_ref().map(Arc::clone))
                .map(Ok)
                .unwrap_or_else(|| {
                    rua_sync::identity::DeviceIdentity::generate_or_load(&self.rua_root)
                        .map(Arc::new)
                        .map_err(|error| {
                            SyncRuntimeError::unavailable(format!("load Device identity: {error}"))
                        })
                })?;

            *self.sync_device.write().unwrap() = Some(Arc::clone(&device));
            *self.sync_network_secret.write().unwrap() = Some(Arc::new(
                rua_sync::identity::NetworkSecret::from_bytes(&network_secret.to_bytes()),
            ));
            let Some(command_handler) = self.device_command_handler() else {
                return Err(SyncRuntimeError::unavailable(
                    "Device Command handler is unavailable",
                ));
            };
            let node = rua_sync::endpoint::SyncNode::bind_with_command_handler(
                &sync_dir,
                network_secret.namespace_secret(),
                &device,
                None,
                command_handler,
            )
            .await
            .map_err(|error| {
                SyncRuntimeError::unavailable(format!("bind Device Network Runtime: {error}"))
            })?;
            self.install_sync_node(node).await;
            Ok(())
        }
        .await;
        if let Err(error) = &result {
            self.device_network_availability.mark_failed(error.detail());
        }
        result
    }

    pub fn device_command_handler(
        &self,
    ) -> Option<Arc<crate::services::device_command_bus::DeviceCommandHandler>> {
        let local_device_id = self.sync_device.read().ok().and_then(|guard| {
            guard
                .as_ref()
                .map(|device| hex::encode(device.device_id_short()))
        })?;
        let collaboration_binding = Arc::new(std::sync::OnceLock::new());
        let _ = collaboration_binding.set(Arc::downgrade(&self.collaboration));
        let handler = Arc::new(
            crate::services::device_command_bus::DeviceCommandHandler::new(
                self.rua_root.join("device_commands"),
                local_device_id,
            )
            .with_remote_history_sync(self.remote_history_sync_tx.clone())
            .with_acp_chat_registry(
                Arc::clone(&self.acp_chat_registry),
                Arc::clone(&self.registry),
            )
            .with_acp_permission_relay(Arc::clone(&self.acp_permission_relay))
            .with_remote_chat_control_streams(Arc::clone(&self.streams))
            .with_conversation_input_intake(
                crate::services::conversation_input_intake::ConversationInputIntake::new(
                    Arc::clone(&self.chat_service),
                    Arc::clone(&self.streams),
                    Arc::clone(&self.registry),
                    SessionSyncEmitter::new(
                        self.rua_root.clone(),
                        Arc::clone(&self.sync_enabled),
                        Arc::clone(&self.sync_node),
                        Arc::clone(&self.sync_network_secret),
                        Arc::clone(&self.sync_device),
                        Arc::clone(&self.sync_hlc_clock),
                        Arc::clone(&self.sync_trigger),
                    )
                    .with_device_key(*self.device_key.as_bytes()),
                    Arc::clone(&self.sync_device),
                    Some(Arc::clone(&self.acp_chat_registry)),
                    Some(Arc::clone(&self.active_stream_broadcast_hub)),
                    crate::services::remote_conversation_input::RemoteConversationInputDelivery::new(
                        Arc::clone(&self.sync_node),
                        Arc::clone(&self.device_network),
                        self.rua_root.clone(),
                    ),
                )
                .with_agent_service(Arc::clone(&self.agent_service))
                .with_provider_manager(Arc::clone(&self.provider_manager))
                .with_provider_cells(Arc::clone(&self.provider_cells))
                .with_conversation_authority(Arc::clone(&self.conversation_authority))
                .with_collaboration_binding(Arc::clone(&collaboration_binding))
                .with_product_hooks(Arc::clone(&self.product_hooks))
                .with_project_runtime(
                    self.rua_root.clone(),
                    Arc::clone(&self.project_catalog),
                    Arc::clone(&self.project_bindings),
                )
                .with_security(Arc::clone(&self.security))
                .with_approval_authority(Arc::clone(self.security.approval_authority()))
                .with_acp_permission_relay(Arc::clone(&self.acp_permission_relay))
                .with_task_runtime(Arc::clone(&self.task_runtime)),
            )
            .with_collaboration_binding(collaboration_binding)
            .with_conversation_authority(Arc::clone(&self.conversation_authority))
            .with_terminal_authority(Arc::clone(&self.terminal_authority))
            .with_sync_transfer_session_manager(Arc::clone(
                &self.sync_transfer_session_manager,
            ))
            .with_chat_media_transfers(Arc::clone(&self.chat_media_transfers))
            .with_project_resource_editor(
                crate::services::project_lifecycle::ProjectResourceEditor::from_app_state(self),
            )
            .with_project_runtime(
                Arc::clone(&self.project_catalog),
                Arc::clone(&self.project_bindings),
                crate::sync_publication::SyncPublicationRuntime::from_app_state(self),
            )
            .with_task_runtime(Arc::clone(&self.task_runtime))
            .with_user_notifications(Arc::clone(&self.user_notifications))
            .with_foreground_operations(Arc::clone(&self.foreground_operations)),
        );
        Some(handler)
    }

    pub async fn bound_sync_network(&self) -> Result<BoundSyncNetwork, BoundSyncNetworkError> {
        let sync_dir = self.rua_root.join("sync");
        let network_secret = self
            .sync_network_secret
            .read()
            .ok()
            .and_then(|guard| guard.as_ref().map(Arc::clone));
        let device = self
            .sync_device
            .read()
            .ok()
            .and_then(|guard| guard.as_ref().map(Arc::clone));
        let node = self.sync_node.lock().await.as_ref().cloned();
        BoundSyncNetwork::from_optional_parts(&sync_dir, node.as_ref(), network_secret, device)
    }

    pub async fn install_sync_node(&self, node: rua_sync::endpoint::SyncNode) {
        let runtime = node.network_runtime().clone();
        let local_device_id =
            crate::services::device_context::current_sync_device_id(&self.sync_device);
        let mut live_handler =
            crate::services::active_stream_broadcast::ActiveStreamBroadcastDeviceSessionHandler::new(
                Arc::clone(&self.active_stream_broadcast_hub),
            );
        if let Some(local_device_id) = local_device_id.as_ref() {
            live_handler = live_handler.with_conversation_control_initial_state(
                Arc::clone(&self.conversation_authority),
                self.conversation_input_intake.clone(),
                local_device_id.clone(),
            );
        }
        live_handler =
            live_handler.with_foreground_operations(Arc::clone(&self.foreground_operations));
        runtime
            .session_manager()
            .set_live_activity_handler(Arc::new(live_handler));
        if let Some(local_device_id) = local_device_id {
            runtime
                .session_manager()
                .set_bulk_transfer_handler(Arc::new(
                    crate::services::sync_transfer::SyncTransferBulkHandler::new(local_device_id)
                        .with_foreground_operations(Arc::clone(&self.foreground_operations))
                        .with_media_transfers(Arc::clone(&self.chat_media_transfers))
                        .with_chat_media_root(self.rua_root.clone()),
                ));
        }
        crate::services::chat_activity_wake::spawn_chat_activity_gossip_listener(
            runtime.clone(),
            Arc::clone(&self.chat_activity_wake_hub),
            Arc::clone(&self.sync_network_secret),
        );
        let session_manager = runtime.session_manager().clone();
        let (old_runtime, old_node) = {
            let mut network_guard = self.device_network.lock().await;
            let mut node_guard = self.sync_node.lock().await;
            let old_runtime = network_guard.replace(runtime);
            let old_node = node_guard.replace(node);
            (old_runtime, old_node)
        };
        *self.sync_transfer_session_manager.write().unwrap() = Some(session_manager);
        self.device_network_availability.mark_ready();
        if let Some(old_node) = old_node.as_ref() {
            old_node.shutdown().await;
        } else if let Some(old_runtime) = old_runtime.as_ref() {
            if let Err(error) = old_runtime.shutdown().await {
                tracing::warn!(error = %error, "failed to shut down replaced Device Network Runtime");
            }
        }
        drop(old_node);
    }

    pub async fn clear_sync_node(&self) {
        self.sync_runtime.mark_not_paired().await;
        clear_sync_node_parts(
            &self.inbound_apply_lock,
            &self.device_network,
            &self.device_network_availability,
            &self.sync_node,
            &self.sync_transfer_session_manager,
        )
        .await;
    }

    pub async fn suspend_device_network_runtime(&self) {
        clear_sync_node_parts(
            &self.inbound_apply_lock,
            &self.device_network,
            &self.device_network_availability,
            &self.sync_node,
            &self.sync_transfer_session_manager,
        )
        .await;
    }

    pub async fn retain_device_network_for_foreground_operation(
        &self,
    ) -> Result<(), SyncRuntimeError> {
        let app_state = self.clone();
        self.sync_runtime
            .retain_for_foreground_operation(move || async move {
                app_state.build_sync_runtime_parts().await
            })
            .await
    }

    fn spawn_foreground_operation_lifecycle(&self) {
        let context = ForegroundOperationLifecycleContext {
            sync_runtime: Arc::downgrade(&self.sync_runtime),
            sync_power_policy: Arc::downgrade(&self.sync_power_policy),
            inbound_apply_lock: Arc::downgrade(&self.inbound_apply_lock),
            device_network: Arc::downgrade(&self.device_network),
            device_network_availability: Arc::downgrade(&self.device_network_availability),
            sync_node: Arc::downgrade(&self.sync_node),
            sync_transfer_session_manager: Arc::downgrade(&self.sync_transfer_session_manager),
        };
        let foreground_operations = Arc::clone(&self.foreground_operations);
        let mut transitions = self.foreground_operations.subscribe_transitions();
        tokio::spawn(async move {
            let mut reaper = tokio::time::interval(Duration::from_secs(30));
            reaper.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    transition = transitions.recv() => {
                        match transition {
                            Ok(crate::services::foreground_operation::ForegroundOperationTransition::AcquiredWhileBackground) => {
                                let Some(sync_runtime) = context.sync_runtime.upgrade() else {
                                    return;
                                };
                                if let Err(error) = sync_runtime
                                    .retain_for_foreground_operation(|| async {
                                        Err(SyncRuntimeError::runtime_suspended(
                                            "background transfer cannot rebind a fully suspended runtime",
                                        ))
                                    })
                                    .await {
                                    tracing::warn!(
                                        reason = error.reason(),
                                        detail = error.detail(),
                                        "failed to retain Device Network Runtime for foreground operation"
                                    );
                                }
                            }
                            Ok(crate::services::foreground_operation::ForegroundOperationTransition::FinalReleasedWhileBackground {
                                grace_generation,
                            }) => {
                                let Some(sync_runtime) = context.sync_runtime.upgrade() else {
                                    return;
                                };
                                let policy = context
                                    .sync_power_policy
                                    .upgrade()
                                    .and_then(|policy| policy.read().ok().map(|guard| {
                                        (
                                            guard.effective_policy.allow_background_runtime,
                                            guard.effective_policy.background_flush_grace_secs,
                                        )
                                    }));
                                let (allow_background_runtime, grace_secs) =
                                    policy.unwrap_or((false, 0));
                                if !allow_background_runtime {
                                    let grace = Duration::from_secs(grace_secs);
                                    let suspend_context = context.clone();
                                    let foreground_operations =
                                        Arc::clone(&foreground_operations);
                                    if let Err(error) = sync_runtime
                                        .notify_background_with_suspend(grace, move || async move {
                                            suspend_context.clear_sync_node().await;
                                            foreground_operations
                                                .mark_suspended(grace_generation);
                                        })
                                        .await {
                                        tracing::warn!(
                                            reason = error.reason(),
                                            detail = error.detail(),
                                            "failed to suspend Device Network Runtime after final foreground operation"
                                        );
                                    }
                                }
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                                tracing::warn!(
                                    skipped,
                                    "foreground-operation lifecycle listener lagged"
                                );
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                        }
                    }
                    _ = reaper.tick() => {
                        if context.sync_runtime.upgrade().is_none() {
                            return;
                        }
                        let expired = foreground_operations
                            .expire_stale(Duration::from_secs(5 * 60));
                        if !expired.is_empty() {
                            tracing::warn!(
                                expired_leases = ?expired,
                                "expired stale foreground-operation leases"
                            );
                        }
                    }
                }
            }
        });
    }

    pub async fn sync_network_membership(&self) -> SyncNetworkMembership {
        if !self.sync_enabled.load(Ordering::SeqCst) {
            return SyncNetworkMembership::disabled();
        }

        let sync_dir = self.rua_root.join("sync");
        let disk_secret = match rua_sync::identity::NetworkSecret::load(&sync_dir) {
            Ok(Some(secret)) => Some(secret),
            Ok(None) => None,
            Err(error) => return SyncNetworkMembership::corrupt(error.to_string()),
        };

        let Some(disk_secret) = disk_secret else {
            return SyncNetworkMembership::enabled_no_network();
        };

        match self.bound_sync_network().await {
            Ok(bound) => SyncNetworkMembership::bound(bound.namespace_id()),
            Err(BoundSyncNetworkError::NotPaired | BoundSyncNetworkError::MissingDiskSecret) => {
                SyncNetworkMembership::network_created(disk_secret.namespace_id())
            }
            Err(error) => SyncNetworkMembership::identity_mismatch(error),
        }
    }

    pub async fn ensure_sync_runtime_active(&self) -> Result<(), SyncRuntimeError> {
        self.sync_runtime
            .ensure_active(|| async { self.build_sync_runtime_parts().await })
            .await
    }

    pub fn sync_runtime_host_profile(&self) -> rua_sync::power_policy::SyncRuntimeHostProfile {
        self.profile.sync_runtime_host_profile()
    }

    pub fn effective_sync_power_policy(&self) -> rua_sync::power_policy::EffectiveSyncPowerPolicy {
        self.sync_power_policy
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .effective_policy
            .clone()
    }

    fn sync_trigger_config(&self) -> rua_sync::trigger::TriggerConfig {
        rua_sync::trigger::TriggerConfig {
            periodic_interval: Duration::from_secs(
                self.effective_sync_power_policy().periodic_interval_secs,
            ),
            ..Default::default()
        }
    }

    pub async fn repair_scan_and_drain_sync_inbox(
        &self,
    ) -> Result<rua_sync::inbound_inbox::RepairScanReport, rua_sync::SyncError> {
        crate::sync_inbound_apply::InboundApplyRuntime::from_state(self)
            .repair_scan_and_drain()
            .await
    }

    pub async fn repair_scan_and_drain_sync_inbox_staged(
        &self,
    ) -> Result<
        rua_sync::inbound_inbox::RepairScanReport,
        crate::sync_inbound_apply::LocalSyncCompletionError,
    > {
        crate::sync_inbound_apply::InboundApplyRuntime::from_state(self)
            .repair_scan_and_drain_staged()
            .await
    }

    pub fn schedule_repair_scan_and_drain_sync_inbox(&self, reason: &'static str) {
        let lane = SyncRepairLane::from_reason(reason);
        let should_spawn = {
            let mut guard = self
                .sync_repair_scheduler
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            guard.schedule(lane)
        };
        if !should_spawn {
            tracing::debug!(
                reason,
                lane = lane.as_str(),
                "[sync-v4-repair] background repair scan/drain already active; queued lane"
            );
            return;
        }

        let state = self.clone();
        tokio::spawn(async move {
            loop {
                let active_lane = state
                    .sync_repair_diagnostics()
                    .active_lane
                    .unwrap_or(SyncRepairLane::Daily);
                match state.repair_scan_and_drain_sync_inbox().await {
                    Ok(report) => {
                        tracing::info!(
                            reason,
                            lane = active_lane.as_str(),
                            scanned = report.scanned,
                            recorded = report.recorded,
                            already_applied = report.already_applied,
                            errors = report.errors.len(),
                            "[sync-v4-repair] background repair scan/drain completed"
                        );
                    }
                    Err(error) => {
                        tracing::warn!(
                            reason,
                            lane = active_lane.as_str(),
                            error = %error,
                            "[sync-v4-repair] background repair scan/drain failed"
                        );
                    }
                }
                let next = {
                    let mut guard = state
                        .sync_repair_scheduler
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    guard.finish_active()
                };
                if next.is_none() {
                    break;
                }
                tracing::debug!(
                    reason,
                    lane = next.expect("checked next is some").as_str(),
                    "[sync-v4-repair] continuing queued repair scan/drain lane"
                );
            }
        });
    }

    pub fn sync_repair_diagnostics(&self) -> SyncRepairDiagnostics {
        let guard = self
            .sync_repair_scheduler
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        SyncRepairDiagnostics {
            active_lane: guard.active_lane,
            pending_lane: guard.pending_lane,
        }
    }

    pub async fn drain_sync_inbox_once(&self) -> Result<(), rua_sync::SyncError> {
        crate::sync_inbound_apply::InboundApplyRuntime::from_state(self)
            .drain_once()
            .await?;
        if let Err(e) = crate::sync_migration::apply_pending_network_migration(self).await {
            tracing::warn!(error = %e, "[sync-migration] apply pending migration");
        }
        Ok(())
    }
}

pub(crate) fn load_im_platform_configs(
    channels: Option<serde_json::Value>,
) -> rua_workspace::service_manager::ImPlatformConfigs {
    fn extract<T: serde::de::DeserializeOwned>(
        channels: &Option<serde_json::Value>,
        key: &str,
    ) -> Option<T> {
        let channels = channels.as_ref()?;
        let saved = channels
            .get(key)
            .or_else(|| (key == "wechat").then(|| channels.get("weixin")).flatten())?;
        let resolved = match rua_config::channel_catalog::resolve_channel_config(key, saved) {
            Ok(config) => config,
            Err(error) => {
                tracing::warn!(channel = key, %error, "invalid channel configuration");
                return None;
            }
        };
        match serde_json::from_value(resolved) {
            Ok(config) => Some(config),
            Err(error) => {
                tracing::warn!(channel = key, %error, "invalid channel configuration");
                None
            }
        }
    }

    rua_workspace::service_manager::ImPlatformConfigs {
        dingtalk: extract(&channels, "dingtalk"),
        wecom: extract(&channels, "wecom"),
        feishu: extract(&channels, "feishu"),
        telegram: extract(&channels, "telegram"),
        discord: extract(&channels, "discord"),
        onebot: extract(&channels, "onebot"),
        mattermost: extract(&channels, "mattermost"),
        slack: extract(&channels, "slack"),
        sip: extract(&channels, "sip"),
        qq: extract(&channels, "qq"),
        matrix: extract(&channels, "matrix"),
        mqtt: extract(&channels, "mqtt"),
        wechat: extract(&channels, "wechat"),
        xiaoyi: extract(&channels, "xiaoyi"),
        yuanbao: extract(&channels, "yuanbao"),
        imessage: extract(&channels, "imessage"),
    }
}

fn block_on_with_optional_handle<F>(handle: Option<tokio::runtime::Handle>, future: F) -> F::Output
where
    F: std::future::Future,
{
    if let Some(handle) = handle {
        return handle.block_on(future);
    }

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("failed to build startup helper runtime");
    rt.block_on(future)
}

fn preheat_used_workspace_runtimes(
    registry: Arc<DefaultWorkspaceRegistry>,
    stats: Arc<UsageStats>,
    active_agent_id: String,
) {
    const MIN_CALLS: u64 = 3;
    const MAX_AGE_DAYS: i64 = 30;

    let handle = match tokio::runtime::Handle::try_current() {
        Ok(h) => h,
        Err(e) => {
            tracing::warn!(
                error = %e,
                "no current tokio runtime; skipping usage-driven preheat"
            );
            return;
        }
    };

    handle.spawn(async move {
        let agents = stats
            .agents_to_preheat(MIN_CALLS, MAX_AGE_DAYS, &active_agent_id)
            .await;
        if agents.is_empty() {
            tracing::debug!("[workspace] no agents qualify for usage-driven preheat");
            return;
        }
        tracing::info!(
            count = agents.len(),
            min_calls = MIN_CALLS,
            max_age_days = MAX_AGE_DAYS,
            "[workspace] background-preheating used agents"
        );
        for agent_id in agents {
            let reg = Arc::clone(&registry);
            tokio::spawn(async move {
                reg.register_agent(&agent_id, &agent_id).await;
                match reg.get_or_create(&agent_id).await {
                    Ok(_) => tracing::info!(
                        agent_id = %agent_id,
                        "[workspace] background-preheated agent"
                    ),
                    Err(e) => tracing::warn!(
                        agent_id = %agent_id,
                        error = %e,
                        "[workspace] background preheat failed"
                    ),
                }
            });
        }
    });
}

fn preheat_workspace_registry(registry: Arc<DefaultWorkspaceRegistry>, agent_id: String) {
    let handle = match tokio::runtime::Handle::try_current() {
        Ok(h) => h,
        Err(e) => {
            tracing::warn!(error = %e, "no current tokio runtime; cannot preheat active agent");
            return;
        }
    };
    let (tx, rx) = std::sync::mpsc::sync_channel::<()>(1);
    let agent_id_log = agent_id.clone();
    handle.spawn(async move {
        registry.register_agent(&agent_id, &agent_id).await;
        match registry.get_or_create(&agent_id).await {
            Ok(_) => {
                tracing::info!(
                    agent_id = %agent_id,
                    "[workspace] preheated agent {}, services=NoOp (platform wired next)",
                    agent_id
                );
            }
            Err(e) => {
                tracing::warn!(
                    agent_id = %agent_id,
                    error = %e,
                    "[workspace] failed to preheat agent runtime"
                );
            }
        }
        let _ = tx.send(());
    });
    if rx.recv().is_err() {
        tracing::warn!(
            agent_id = %agent_id_log,
            "[workspace] preheat task channel closed before completion"
        );
    }
}

fn find_builtin_skills_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("RUA_BUILTIN_SKILLS_DIR") {
        let p = PathBuf::from(dir);
        if p.is_dir() {
            return Some(p);
        }
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            let candidate = exe_dir.join("../share/rua/builtin_skills");
            if candidate.is_dir() {
                return Some(candidate);
            }
        }
    }

    let dev_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../rua-agent/builtin_skills");
    if dev_path.is_dir() {
        return Some(dev_path);
    }

    None
}

pub use crate::tunnel_bootstrap::tunnel_event_subscriber_loop;

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState")
            .field("profile", &self.profile)
            .field("ready", &self.is_ready())
            .finish()
    }
}
