use std::net::IpAddr;
use std::path::PathBuf;

use clap::{Args, Subcommand};
use rua_server::{ServerOptions, ServerProfile};

use crate::http_client::resolve_server_port;
use crate::output::OutputFormat;

#[derive(Debug, Args)]
pub struct ServerArgs {
    #[command(subcommand)]
    command: Option<ServerCommands>,

    #[arg(long, default_value = "127.0.0.1", global = true)]
    host: IpAddr,

    #[arg(long, global = true)]
    port: Option<u16>,

    #[arg(long, default_value = "desktop", global = true)]
    profile: ServerProfile,

    #[arg(long, global = true)]
    console_static_dir: Option<PathBuf>,

    #[arg(long, global = true)]
    no_auth: bool,

    #[arg(long, global = true, hide = true)]
    desktop_endpoint_file: Option<PathBuf>,
}

impl ServerArgs {
    pub fn starts_server(&self) -> bool {
        matches!(self.command.as_ref(), None | Some(ServerCommands::Start))
    }
}

#[derive(Debug, Subcommand)]
enum ServerCommands {
    Start,

    Stop,

    Status,
}

pub fn run(
    args: ServerArgs,
    format: OutputFormat,
    debug_log: bool,
    rua_root: PathBuf,
) -> anyhow::Result<()> {
    let port = resolve_server_port(args.port);
    match args.command {
        None | Some(ServerCommands::Start) => run_start(args, port, debug_log, rua_root),
        Some(ServerCommands::Stop) => run_stop(args, port, format),
        Some(ServerCommands::Status) => run_status(args, port, format),
    }
}

fn run_start(
    args: ServerArgs,
    port: u16,
    debug_log: bool,
    rua_root: PathBuf,
) -> anyhow::Result<()> {
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        rua_server::serve(ServerOptions {
            host: args.host,
            port,
            profile: args.profile,
            console_static_dir: args.console_static_dir,
            no_auth: args.no_auth,
            debug_log,
            rua_root: Some(rua_root),
            desktop_endpoint_file: args.desktop_endpoint_file,
            desktop_host: rua_server::desktop_host::DesktopHostConfig::from_environment(),
        })
        .await
    })
}

fn run_stop(args: ServerArgs, port: u16, format: OutputFormat) -> anyhow::Result<()> {
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        let url = format!("http://{}:{port}/api/shutdown", args.host);
        let client = rua_http::default_client_or_default("rua-cli");
        match client.post(&url).send().await {
            Ok(resp) if resp.status().is_success() => {
                match format {
                    OutputFormat::Json => {
                        crate::output::print_json(&serde_json::json!({"status": "stopped"}));
                    }
                    OutputFormat::Human => println!("Server stopped."),
                }
                Ok(())
            }
            Ok(resp) => {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                anyhow::bail!("Server stop failed ({status}): {body}");
            }
            Err(e) => {
                match format {
                    OutputFormat::Json => {
                        crate::output::print_json(
                            &serde_json::json!({"status": "not_running", "error": e.to_string()}),
                        );
                    }
                    OutputFormat::Human => {
                        println!("Server is not running (or not reachable): {e}");
                    }
                }
                Ok(())
            }
        }
    })
}

fn run_status(args: ServerArgs, port: u16, format: OutputFormat) -> anyhow::Result<()> {
    let addr = format!("{}:{port}", args.host);
    let health_url = format!("http://{addr}/healthz");

    match std::net::TcpStream::connect_timeout(
        &addr
            .parse()
            .unwrap_or_else(|_| std::net::SocketAddr::from(([127, 0, 0, 1], port))),
        std::time::Duration::from_secs(3),
    ) {
        Ok(_) => match format {
            OutputFormat::Json => {
                crate::output::print_json(&serde_json::json!({
                    "status": "running",
                    "address": addr,
                    "health_url": health_url,
                }));
            }
            OutputFormat::Human => {
                println!("Server is running at {addr}");
                println!("  Health: {health_url}");
            }
        },
        Err(_) => match format {
            OutputFormat::Json => {
                crate::output::print_json(&serde_json::json!({
                    "status": "not_running",
                    "address": addr,
                }));
            }
            OutputFormat::Human => {
                println!("Server is not running at {addr}");
            }
        },
    }

    Ok(())
}
