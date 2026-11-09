use clap::Args;
use rua_config::dirs::Paths;

use crate::http_client::{resolve_base_url, ApiClient};
use crate::process_utils;

#[derive(Debug, Args)]
pub struct ShutdownArgs {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownAction {
    KillProcess(u32),
    PostShutdown,
}

pub fn run(_args: ShutdownArgs, paths: &Paths) -> anyhow::Result<()> {
    match choose_shutdown_action(process_utils::is_rua_running(paths)) {
        ShutdownAction::KillProcess(pid) => {
            println!("Found Rua process (pid={pid}), killing...");
            process_utils::kill_stale_rua(pid)?;
            println!("Rua service stopped (pid={pid}).");
            Ok(())
        }
        ShutdownAction::PostShutdown => {
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(run_async())
        }
    }
}

async fn run_async() -> anyhow::Result<()> {
    let base_url = resolve_base_url(None);
    let client = ApiClient::new(&base_url, "rua-cli");

    match client
        .post(&shutdown_endpoint(), &shutdown_request_body())
        .await
    {
        Ok(_) => {
            println!("Rua service is shutting down.");
        }
        Err(e) => {
            let msg = e.to_string();
            if is_no_running_service_error(&msg) {
                println!("No running Rua service found.");
                return Ok(());
            }
            return Err(e);
        }
    }

    Ok(())
}

pub fn choose_shutdown_action(local_pid: Option<u32>) -> ShutdownAction {
    match local_pid {
        Some(pid) => ShutdownAction::KillProcess(pid),
        None => ShutdownAction::PostShutdown,
    }
}

pub fn shutdown_endpoint() -> String {
    "/api/shutdown".to_string()
}

pub fn shutdown_request_body() -> serde_json::Value {
    serde_json::json!({})
}

pub fn is_no_running_service_error(message: &str) -> bool {
    message.contains("Connection refused")
        || message.contains("connect error")
        || message.contains("tcp connect error")
}
