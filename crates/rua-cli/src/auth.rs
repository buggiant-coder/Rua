use std::path::PathBuf;

use anyhow::{bail, Context};
use clap::{Args, Subcommand};

#[derive(Debug, Args)]
pub struct AuthArgs {
    #[command(subcommand)]
    pub command: AuthCommands,
}

#[derive(Debug, Subcommand)]
pub enum AuthCommands {
    SetPassword,
}

pub fn run(args: AuthArgs) -> anyhow::Result<()> {
    match args.command {
        AuthCommands::SetPassword => set_password(),
    }
}

fn resolve_secret_dir() -> PathBuf {
    rua_config::dirs::resolve_secret_dir()
}

fn set_password() -> anyhow::Result<()> {
    let secret_dir = resolve_secret_dir();
    std::fs::create_dir_all(&secret_dir)
        .with_context(|| format!("failed to create secret dir: {}", secret_dir.display()))?;

    let password =
        rpassword::prompt_password_stderr("New password: ").context("failed to read password")?;

    if password.trim().is_empty() {
        bail!("Password cannot be empty.");
    }

    let confirm = rpassword::prompt_password_stderr("Confirm password: ")
        .context("failed to read password confirmation")?;

    if password != confirm {
        bail!("Passwords do not match.");
    }

    let auth_file = secret_dir.join("auth.json");

    if auth_file.is_file() {
        let content = std::fs::read_to_string(&auth_file)
            .with_context(|| format!("failed to read {}", auth_file.display()))?;
        if let Ok(mut data) =
            serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&content)
        {
            if data.contains_key("user") {
                data.remove("user");

                data.remove("jwt_secret");
                data.remove("revoked_tokens");
                data.remove("revoked_tokens_meta");
                let json =
                    serde_json::to_string_pretty(&data).context("failed to serialize auth data")?;
                std::fs::write(&auth_file, json)
                    .with_context(|| format!("failed to write {}", auth_file.display()))?;
            }
        }
    }

    let auth = rua_server::auth::AuthService::from_secret_dir(&secret_dir);
    let token = auth.register_user("admin", &password, None);

    if token.is_some() {
        println!(
            "\u{2713} Password set successfully. All existing sessions have been invalidated."
        );
    } else {
        bail!("Failed to set password (unexpected state).");
    }

    Ok(())
}
