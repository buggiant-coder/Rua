use anyhow::Context;
use clap::{Args, Subcommand};
use std::path::Path;

use crate::http_client::{resolve_base_url, ApiClient};

#[derive(Debug, Args)]
pub struct PluginArgs {
    #[command(subcommand)]
    pub command: PluginCommands,
}

#[derive(Debug, Subcommand)]
pub enum PluginCommands {
    List,

    Info {
        plugin_id: String,
    },

    Install {
        source: String,

        #[arg(long)]
        force: bool,
    },

    Uninstall {
        plugin_id: String,
    },
}

pub fn run(args: PluginArgs, base_url: Option<&str>) -> anyhow::Result<()> {
    let plugins_dir = rua_config::get_plugins_dir(&rua_config::dirs::rua_root());

    match args.command {
        PluginCommands::List => cmd_list(&plugins_dir),
        PluginCommands::Info { plugin_id } => cmd_info(&plugin_id, &plugins_dir),
        PluginCommands::Install { source, force } => cmd_install(&source, force, base_url),
        PluginCommands::Uninstall { plugin_id } => cmd_uninstall(&plugin_id, &plugins_dir),
    }
}

fn cmd_list(plugins_dir: &Path) -> anyhow::Result<()> {
    if !plugins_dir.is_dir() {
        println!("No plugins installed.");
        return Ok(());
    }

    let mut found = false;
    for entry in std::fs::read_dir(plugins_dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let manifest_path = path.join("plugin.json");
        if !manifest_path.is_file() {
            continue;
        }
        if let Ok(content) = std::fs::read_to_string(&manifest_path) {
            if let Ok(manifest) = serde_json::from_str::<serde_json::Value>(&content) {
                if !found {
                    println!("Installed Plugins:");
                    println!();
                    found = true;
                }
                let name = manifest
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                let version = manifest
                    .get("version")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?");
                let id = manifest.get("id").and_then(|v| v.as_str()).unwrap_or("?");
                let description = manifest
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("N/A");
                println!("  - {name} (v{version})");
                println!("    ID: {id}");
                println!("    Description: {description}");
                println!();
            }
        }
    }

    if !found {
        println!("No plugins installed.");
    }

    Ok(())
}

fn cmd_info(plugin_id: &str, plugins_dir: &Path) -> anyhow::Result<()> {
    let plugin_dir = plugins_dir.join(plugin_id);
    if !plugin_dir.is_dir() {
        eprintln!("Plugin '{plugin_id}' not found");
        std::process::exit(1);
    }

    let manifest_path = plugin_dir.join("plugin.json");
    if !manifest_path.is_file() {
        eprintln!("plugin.json not found for '{plugin_id}'");
        std::process::exit(1);
    }

    let content = std::fs::read_to_string(&manifest_path)?;
    let manifest: serde_json::Value = serde_json::from_str(&content)?;

    let name = manifest
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let version = manifest
        .get("version")
        .and_then(|v| v.as_str())
        .unwrap_or("?");
    let id = manifest.get("id").and_then(|v| v.as_str()).unwrap_or("?");
    let description = manifest
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("N/A");
    let author = manifest
        .get("author")
        .and_then(|v| v.as_str())
        .unwrap_or("N/A");
    let entry_point = manifest
        .get("entry_point")
        .and_then(|v| v.as_str())
        .unwrap_or("plugin.py");

    println!();
    println!("{name} (v{version})");
    println!();
    println!("ID: {id}");
    println!("Description: {description}");
    println!("Author: {author}");
    println!("Entry Point: {entry_point}");
    println!();
    println!("Location: {}", plugin_dir.display());

    Ok(())
}

fn cmd_install(source: &str, force: bool, base_url: Option<&str>) -> anyhow::Result<()> {
    println!("Installing plugin from {source}...");

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        let url = resolve_base_url(base_url);
        let client = ApiClient::new(&url, "rua-cli");

        let body = serde_json::json!({
            "source": source,
            "force": force,
        });

        match client.post("/api/plugins/install", &body).await {
            Ok(resp) => {
                let plugin_name = resp["plugin_name"].as_str().unwrap_or("unknown");
                let plugin_id = resp["plugin_id"].as_str().unwrap_or("unknown");
                let status = resp["status"].as_str().unwrap_or("installed");

                println!("\nPlugin '{plugin_name}' installed successfully!");
                println!("Plugin ID: {plugin_id}");

                if status == "partial" {
                    if let Some(errors) = resp["errors"].as_array() {
                        println!("\nWarnings during installation:");
                        for err in errors {
                            println!("  - {err}");
                        }
                    }
                }

                Ok(())
            }
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("onnection refused")
                    || msg.contains("tcp connect error")
                    || msg.contains("os error 61")
                {
                    anyhow::bail!("Server not running. Start Rua first with: rua server start");
                }
                Err(e)
            }
        }
    })
}

pub fn safe_extract_zip(data: &[u8], extract_dir: &Path) -> anyhow::Result<()> {
    rua_plugin_runtime::safe_extract_zip(data, extract_dir).map_err(|e| anyhow::anyhow!("{e}"))
}

fn cmd_uninstall(plugin_id: &str, plugins_dir: &Path) -> anyhow::Result<()> {
    let plugin_dir = plugins_dir.join(plugin_id);

    if !plugin_dir.is_dir() {
        anyhow::bail!("Plugin '{plugin_id}' not found");
    }

    std::fs::remove_dir_all(&plugin_dir)
        .with_context(|| format!("Failed to remove plugin '{plugin_id}'"))?;

    println!("Plugin '{plugin_id}' uninstalled successfully.");
    println!("Restart Rua to apply changes.");

    Ok(())
}
