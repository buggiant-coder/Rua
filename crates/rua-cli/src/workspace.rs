use std::path::PathBuf;

use clap::{Args, Subcommand};

use crate::http_client::{resolve_base_url, ApiClient};
use crate::output::{print_value, OutputFormat};

#[derive(Debug, Args)]
pub struct WorkspaceArgs {
    #[command(subcommand)]
    command: WorkspaceCommands,
}

#[derive(Debug, Subcommand)]
enum WorkspaceCommands {
    Info,

    Download {
        #[arg(short = 'f', long = "file")]
        out_file: Option<PathBuf>,

        #[arg(long)]
        agent: Option<String>,
    },

    Upload {
        file: PathBuf,

        #[arg(long)]
        agent: Option<String>,
    },
}

pub fn run(
    args: WorkspaceArgs,
    format: OutputFormat,
    base_url: Option<&str>,
    agent_id: &str,
) -> anyhow::Result<()> {
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(run_async(args, format, base_url, agent_id))
}

async fn run_async(
    args: WorkspaceArgs,
    format: OutputFormat,
    base_url: Option<&str>,
    agent_id: &str,
) -> anyhow::Result<()> {
    let client = ApiClient::new(&resolve_base_url(base_url), agent_id);

    match args.command {
        WorkspaceCommands::Info => {
            let resp = client
                .get(&format!("/api/agents/{agent_id}/workspace"))
                .await?;
            print_value(&resp, format);
        }
        WorkspaceCommands::Download { out_file, agent } => {
            let path = match &agent {
                Some(aid) => format!("/api/agents/{aid}/workspace/download"),
                None => "/api/workspace/download".to_string(),
            };
            let data = client.download_bytes(&path).await?;
            let out_path = out_file.unwrap_or_else(|| PathBuf::from("workspace.zip"));
            std::fs::write(&out_path, &data)?;
            match format {
                OutputFormat::Json => {
                    let result = serde_json::json!({
                        "path": out_path.display().to_string(),
                        "size": data.len(),
                    });
                    crate::output::print_json(&result);
                }
                OutputFormat::Human => {
                    println!(
                        "Downloaded workspace to {} ({} bytes)",
                        out_path.display(),
                        data.len()
                    );
                }
            }
        }
        WorkspaceCommands::Upload { file, agent } => {
            if !file.is_file() {
                anyhow::bail!("File not found: {}", file.display());
            }
            let data = std::fs::read(&file)?;
            let path = match &agent {
                Some(aid) => format!("/api/agents/{aid}/workspace/upload"),
                None => "/api/workspace/upload".to_string(),
            };
            let resp = client.upload_bytes(&path, bytes::Bytes::from(data)).await?;
            match format {
                OutputFormat::Json => crate::output::print_json(&resp),
                OutputFormat::Human => {
                    println!("Workspace uploaded successfully from {}", file.display());
                    if let Some(obj) = resp.as_object() {
                        for (k, v) in obj {
                            println!("  {k}: {v}");
                        }
                    }
                }
            }
        }
    }

    Ok(())
}
