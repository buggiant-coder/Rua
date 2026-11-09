use std::collections::HashMap;
use std::path::PathBuf;

use clap::{Args, Subcommand};
use rua_config::dirs::Paths;

#[derive(Debug, Args)]
pub struct EnvArgs {
    #[command(subcommand)]
    pub command: EnvCommands,
}

#[derive(Debug, Subcommand)]
pub enum EnvCommands {
    List,

    Set { key: String, value: Option<String> },

    Delete { key: String },
}

fn envs_path(paths: &Paths) -> PathBuf {
    paths.secret_dir.join("envs.json")
}

fn load_envs(paths: &Paths) -> anyhow::Result<HashMap<String, String>> {
    let path = envs_path(paths);
    if !path.is_file() {
        return Ok(HashMap::new());
    }
    let content = std::fs::read_to_string(&path)?;
    let map: HashMap<String, String> = serde_json::from_str(&content)?;
    Ok(map)
}

fn is_sensitive_key(key: &str) -> bool {
    let lower = key.to_lowercase();
    lower.contains("api_key")
        || lower.contains("apikey")
        || lower.contains("secret")
        || lower.contains("token")
        || lower.contains("password")
        || lower.contains("authorization")
}

fn redact_value(value: &str) -> String {
    if value.is_empty() {
        return String::new();
    }
    if value.len() <= 10 {
        return "***".to_string();
    }
    format!("{}...{}", &value[..4], &value[value.len() - 4..])
}

pub fn run(args: EnvArgs, paths: &Paths) -> anyhow::Result<()> {
    match args.command {
        EnvCommands::List => {
            let envs = load_envs(paths)?;
            if envs.is_empty() {
                println!("No environment variables configured.");
                return Ok(());
            }
            let mut keys: Vec<&String> = envs.keys().collect();
            keys.sort();
            println!();
            println!("  {:<30}  Value", "Key");
            println!("  {}", "\u{2500}".repeat(56));
            for key in keys {
                let display_value = if is_sensitive_key(key) {
                    redact_value(&envs[key])
                } else {
                    envs[key].clone()
                };
                println!("  {key:<30}  {display_value}");
            }
            println!();
            Ok(())
        }
        EnvCommands::Set { key, value } => {
            let value = match value {
                Some(v) => v,
                None => rpassword::prompt_password_stderr(&format!("  Value for {key}: "))?,
            };
            let mut envs = load_envs(paths)?;
            envs.insert(key.clone(), value.clone());

            let path = envs_path(paths);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let json_value = serde_json::to_value(&envs)?;
            rua_config::atomic_write_json(&path, &json_value)?;

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let perms = std::fs::Permissions::from_mode(0o600);
                let _ = std::fs::set_permissions(&path, perms);
            }

            let display_value = if is_sensitive_key(&key) {
                redact_value(&value)
            } else {
                value
            };
            println!("\u{2713} {key} = {display_value}");
            Ok(())
        }
        EnvCommands::Delete { key } => {
            let mut envs = load_envs(paths)?;
            if !envs.contains_key(&key) {
                anyhow::bail!("Env var '{}' not found.", key);
            }
            envs.remove(&key);

            let path = envs_path(paths);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let json_value = serde_json::to_value(&envs)?;
            rua_config::atomic_write_json(&path, &json_value)?;

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let perms = std::fs::Permissions::from_mode(0o600);
                let _ = std::fs::set_permissions(&path, perms);
            }

            println!("\u{2713} Deleted: {key}");
            Ok(())
        }
    }
}
