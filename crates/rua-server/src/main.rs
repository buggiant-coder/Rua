use std::net::IpAddr;
use std::path::PathBuf;

use clap::Parser;
use rua_server::{ServerOptions, ServerProfile};

#[derive(Parser)]
#[command(name = "rua-server", about = "Rua HTTP server")]
struct Cli {
    #[arg(long, default_value = "127.0.0.1")]
    host: IpAddr,

    #[arg(long, default_value_t = 8765)]
    port: u16,

    #[arg(long, default_value = "desktop")]
    profile: ServerProfile,

    #[arg(long)]
    console_static_dir: Option<PathBuf>,

    #[arg(long)]
    no_auth: bool,

    #[arg(long, short = 'v')]
    debug: bool,

    #[arg(long)]
    rua_root: Option<PathBuf>,

    #[arg(long, hide = true)]
    desktop_endpoint_file: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    rua_server::serve(ServerOptions {
        host: cli.host,
        port: cli.port,
        profile: cli.profile,
        console_static_dir: cli.console_static_dir,
        no_auth: cli.no_auth,
        debug_log: cli.debug,
        rua_root: cli.rua_root,
        desktop_endpoint_file: cli.desktop_endpoint_file,
        desktop_host: rua_server::desktop_host::DesktopHostConfig::from_environment(),
    })
    .await
}
