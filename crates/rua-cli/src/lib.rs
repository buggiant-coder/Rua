pub mod acp;
mod acp_remote;
pub mod agent;
pub mod auth;
pub mod channels;
pub mod chat;
pub mod clean;
pub mod config;
pub mod console;
pub mod cron;
pub mod daemon;
pub mod desktop;
pub mod doctor;
pub mod env_cmd;
pub mod http_client;
pub mod init;
pub mod mcp;
pub mod models;
pub mod output;
pub mod plugin;
pub mod process_utils;
pub mod project;
pub mod providers;
pub mod server;
pub mod setup_env;
pub mod shutdown;
pub mod skills;
pub mod timezone;
pub mod uninstall;
pub mod update;
pub mod usage;
pub mod workspace;

use clap::{Parser, Subcommand};
use output::OutputFormat;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Parser)]
#[command(name = "rua", about = "Rua CLI", version = VERSION)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,

    #[arg(long, value_enum, default_value_t = OutputFormat::Human, global = true)]
    pub output: OutputFormat,

    #[arg(long, global = true)]
    pub base_url: Option<String>,

    #[arg(long, default_value = "default", global = true)]
    pub agent_id: String,

    #[arg(long, short = 'v', global = true)]
    pub verbose: bool,

    #[arg(long, global = true)]
    pub rua_root: Option<String>,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    Acp(acp::AcpArgs),

    Agent(agent::AgentArgs),

    Auth(auth::AuthArgs),

    Channels(channels::ChannelsArgs),

    Chat(chat::ChatArgs),

    Chats(console::ConsoleArgs),

    Clean(clean::CleanArgs),

    Config(config::ConfigArgs),

    Cron(cron::CronArgs),

    Daemon(daemon::DaemonArgs),

    Desktop(desktop::DesktopArgs),

    Doctor(doctor::DoctorArgs),

    Env(env_cmd::EnvArgs),

    Init(init::InitArgs),

    Mcp(mcp::McpArgs),

    Models(models::ModelsArgs),

    Plugin(plugin::PluginArgs),

    Providers(providers::ProvidersArgs),

    Project(project::ProjectArgs),

    #[command(visible_alias = "app")]
    Server(server::ServerArgs),

    SetupEnv(setup_env::SetupEnvArgs),

    Shutdown(shutdown::ShutdownArgs),

    Skills(skills::SkillsArgs),

    Uninstall(uninstall::UninstallArgs),

    Update(update::UpdateArgs),

    Usage(usage::UsageArgs),

    Workspace(workspace::WorkspaceArgs),

    #[command(hide = true, name = "internal-download-task")]
    InternalDownloadTask {
        model_id: String,
    },
}

impl Commands {
    pub fn starts_server(&self) -> bool {
        matches!(self, Self::Server(args) if args.starts_server())
    }
}
