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
    /// Store a credential for this machine's Runtime, registering the first
    /// user when none exists yet.
    Login(LoginArgs),

    SetPassword,
}

/// Credential filename inside the Secret Root. Kept apart from
/// `desktop-endpoint.json` (a discovery record in the Rua Root) because this
/// file is a secret and must inherit the Secret Root's 0700/0600 protection.
const CLI_CREDENTIAL_FILE: &str = "cli_credential.json";

#[derive(Debug, Args)]
pub struct LoginArgs {
    /// Username. Defaults to `admin`.
    #[arg(long)]
    pub username: Option<String>,

    /// Read the password from stdin instead of prompting.
    #[arg(long)]
    pub password_stdin: bool,

    /// Runtime base URL. Defaults to the same discovery the rest of the CLI uses.
    #[arg(long)]
    pub base_url: Option<String>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct CliCredential {
    version: u8,
    base_url: String,
    username: String,
    token: String,
}

const CLI_CREDENTIAL_VERSION: u8 = 1;

pub fn run(args: AuthArgs) -> anyhow::Result<()> {
    match args.command {
        AuthCommands::Login(args) => login(args),
        AuthCommands::SetPassword => set_password(),
    }
}

fn resolve_secret_dir() -> PathBuf {
    rua_config::dirs::resolve_secret_dir()
}

/// Credential file for CLI HTTP calls.
pub fn credential_path() -> PathBuf {
    resolve_secret_dir().join(CLI_CREDENTIAL_FILE)
}

/// Load the stored CLI credential, if any.
///
/// Returns `None` when the file is missing or was written for a different
/// Runtime base URL: a credential minted by another Runtime's JWT secret would
/// only produce a confusing 401 later.
pub fn load_credential_for(base_url: &str) -> Option<String> {
    let path = credential_path();
    let content = std::fs::read_to_string(path).ok()?;
    let credential: CliCredential = serde_json::from_str(&content).ok()?;
    if credential.version != CLI_CREDENTIAL_VERSION
        || !base_urls_match(&credential.base_url, base_url)
    {
        return None;
    }
    (!credential.token.is_empty()).then_some(credential.token)
}

/// Canonical form used to decide whether a stored credential belongs to a
/// Runtime. `ApiClient` already trims trailing slashes, so a credential written
/// from a URL like `http://127.0.0.1:8765/` must still match the trimmed form;
/// otherwise a valid login would silently stop being sent.
fn normalize_base_url(base_url: &str) -> &str {
    base_url.trim_end_matches('/')
}

/// Whether a stored credential was minted for the given Runtime URL.
fn base_urls_match(stored: &str, requested: &str) -> bool {
    normalize_base_url(stored) == normalize_base_url(requested)
}

fn write_credential(credential: &CliCredential) -> anyhow::Result<()> {
    let path = credential_path();
    let parent = path.parent().unwrap_or_else(|| std::path::Path::new("."));
    std::fs::create_dir_all(parent)
        .with_context(|| format!("failed to create secret dir: {}", parent.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
    }

    let payload = serde_json::to_vec_pretty(credential).context("failed to serialize credential")?;
    let tmp = path.with_extension("json.tmp");
    {
        use std::io::Write;
        let mut options = std::fs::OpenOptions::new();
        options.create(true).truncate(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&tmp)
            .with_context(|| format!("failed to open {}", tmp.display()))?;
        file.write_all(&payload)?;
        file.sync_all()?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(&tmp, &path)
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

fn login(args: LoginArgs) -> anyhow::Result<()> {
    // Store the canonical form so the credential matches what `ApiClient`
    // compares against, regardless of a trailing slash on `--base-url`.
    let base_url = normalize_base_url(&crate::http_client::resolve_base_url(
        args.base_url.as_deref(),
    ))
    .to_string();
    let username = args.username.unwrap_or_else(|| "admin".to_string());

    let password = if args.password_stdin {
        let mut buffer = String::new();
        use std::io::Read;
        std::io::stdin()
            .read_to_string(&mut buffer)
            .context("failed to read password from stdin")?;
        buffer.trim_end_matches(['\r', '\n']).to_string()
    } else {
        rpassword::prompt_password_stderr("Password: ").context("failed to read password")?
    };

    if password.is_empty() {
        bail!("Password cannot be empty.");
    }

    let url = format!("{base_url}/api/auth/login");
    // `auth login` is a synchronous command; the blocking client keeps it off
    // an async runtime that the rest of the CLI entrypoint does not provide.
    let client = rua_http::build_blocking_client_or_default(
        reqwest::blocking::Client::builder().no_proxy(),
        "rua-cli",
    );
    let response = client
        .post(&url)
        .json(&serde_json::json!({ "username": username, "password": password }))
        .send()
        .with_context(|| format!("POST {url}"))
        .map_err(|error| {
            error.context(
                "failed to reach the Rua Runtime; start it with `rua server start` first",
            )
        })?;

    let status = response.status();
    let body: serde_json::Value = response.json().unwrap_or(serde_json::Value::Null);
    if !status.is_success() {
        let detail = body
            .get("detail")
            .and_then(|value| value.as_str())
            .unwrap_or("login failed");
        bail!("{detail}");
    }

    let token = body
        .get("token")
        .and_then(|value| value.as_str())
        .filter(|token| !token.is_empty())
        .context("Runtime returned an empty token")?;

    write_credential(&CliCredential {
        version: CLI_CREDENTIAL_VERSION,
        base_url: base_url.clone(),
        username,
        token: token.to_string(),
    })?;

    println!(
        "\u{2713} Logged in. Credential stored at {}",
        credential_path().display()
    );
    Ok(())
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
        // Every previously issued credential is now dead, including the one on
        // disk, so leaving it in place would make the next CLI call fail with a
        // bare 401 and no explanation.
        let credential = credential_path();
        if credential.is_file() {
            match std::fs::remove_file(&credential) {
                Ok(()) => println!(
                    "  Stored CLI credential removed. Run `rua auth login` to sign in again."
                ),
                Err(error) => println!(
                    "  Warning: could not remove {}: {error}\n  Run `rua auth login` to sign in again.",
                    credential.display()
                ),
            }
        } else {
            println!("  Run `rua auth login` to sign in from this machine.");
        }
    } else {
        bail!("Failed to set password (unexpected state).");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_credential_matches_the_runtime_regardless_of_trailing_slash() {
        // `ApiClient` trims the URL before looking the credential up, while
        // `--base-url` is passed through verbatim. A mismatch here would drop a
        // perfectly valid credential and turn every call into a bare 401.
        assert!(base_urls_match("http://127.0.0.1:8765", "http://127.0.0.1:8765"));
        assert!(base_urls_match("http://127.0.0.1:8765/", "http://127.0.0.1:8765"));
        assert!(base_urls_match("http://127.0.0.1:8765", "http://127.0.0.1:8765/"));
        assert!(base_urls_match("http://127.0.0.1:8765//", "http://127.0.0.1:8765"));
    }

    #[test]
    fn stored_credential_is_not_reused_for_a_different_runtime() {
        // A different port or host is a different Runtime with a different JWT
        // secret, so the stored token must not be offered to it.
        assert!(!base_urls_match("http://127.0.0.1:8765", "http://127.0.0.1:9000"));
        assert!(!base_urls_match("http://127.0.0.1:8765", "http://localhost:8765"));
        assert!(!base_urls_match("http://127.0.0.1:8765", "https://127.0.0.1:8765"));
    }
}
