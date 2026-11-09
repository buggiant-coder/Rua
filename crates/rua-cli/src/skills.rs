use std::collections::{BTreeMap, BTreeSet};
use std::io::{Cursor, IsTerminal, Read as _, Seek, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::{Args, Subcommand};
use reqwest::StatusCode;
use serde_json::Value;
use zip::write::{SimpleFileOptions, ZipWriter};
use zip::CompressionMethod;

use crate::http_client::{error_status, resolve_base_url, ApiClient};
use crate::output::{print_value, OutputFormat};

#[derive(Debug, Args)]
pub struct SkillsArgs {
    #[command(subcommand)]
    command: SkillsCommands,
}

#[derive(Debug, Subcommand)]
enum SkillsCommands {
    List {
        #[arg(long)]
        pool: bool,
    },

    #[command(alias = "info")]
    Get {
        name: String,
        #[arg(long)]
        pool: bool,
    },

    Search {
        query: String,

        #[arg(long = "provider")]
        providers: Vec<String>,

        #[arg(long, default_value_t = 10)]
        limit: u32,

        #[arg(long, default_value_t = 1)]
        page: u32,

        #[arg(long, default_value = "en")]
        lang: String,
    },

    Install {
        source_url: String,

        #[arg(long)]
        version: Option<String>,

        #[arg(long)]
        name: Option<String>,

        #[arg(long)]
        pool: bool,

        #[arg(long)]
        disabled: bool,
    },

    Import {
        path: PathBuf,

        #[arg(long)]
        name: Option<String>,

        #[arg(long)]
        pool: bool,

        #[arg(long)]
        replace: bool,
    },

    CheckUpdate {
        name: String,
        #[arg(long)]
        pool: bool,
    },

    Update {
        name: String,
        #[arg(long)]
        pool: bool,

        #[arg(long)]
        force: bool,
    },

    Enable {
        name: String,
    },

    Disable {
        name: String,
    },

    Config {
        name: String,
    },

    #[command(alias = "remove")]
    Uninstall {
        name: String,
        #[arg(long)]
        pool: bool,

        #[arg(long)]
        yes: bool,
    },
}

pub fn run(
    args: SkillsArgs,
    format: OutputFormat,
    base_url: Option<&str>,
    agent_id: &str,
) -> anyhow::Result<()> {
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(run_async(args, format, base_url, agent_id))
}

async fn run_async(
    args: SkillsArgs,
    format: OutputFormat,
    base_url: Option<&str>,
    agent_id: &str,
) -> anyhow::Result<()> {
    let client = ApiClient::new(&resolve_base_url(base_url), agent_id);

    match args.command {
        SkillsCommands::List { pool } => list_skills(&client, format, pool).await,
        SkillsCommands::Get { name, pool } => {
            let skill = get_skill(&client, &name, pool).await?;
            print_value(&skill, format);
            Ok(())
        }
        SkillsCommands::Search {
            query,
            providers,
            limit,
            page,
            lang,
        } => search_skills(&client, format, &query, providers, limit, page, &lang).await,
        SkillsCommands::Install {
            source_url,
            version,
            name,
            pool,
            disabled,
        } => {
            if pool && disabled {
                anyhow::bail!("--disabled is not valid with --pool");
            }
            install_skill(
                &client,
                format,
                &source_url,
                version.as_deref(),
                name.as_deref(),
                pool,
                !disabled,
            )
            .await
        }
        SkillsCommands::Import {
            path,
            name,
            pool,
            replace,
        } => import_skill(&client, format, &path, name.as_deref(), pool, replace).await,
        SkillsCommands::CheckUpdate { name, pool } => {
            let response = client
                .post(
                    &skill_update_path(client.agent_id(), &name, pool, true),
                    &serde_json::json!({}),
                )
                .await?;
            print_value(&response, format);
            Ok(())
        }
        SkillsCommands::Update { name, pool, force } => {
            update_skill(&client, format, &name, pool, force).await
        }
        SkillsCommands::Enable { name } => set_skill_enabled(&client, format, &name, true).await,
        SkillsCommands::Disable { name } => set_skill_enabled(&client, format, &name, false).await,
        SkillsCommands::Config { name } => configure_skill(&client, format, &name).await,
        SkillsCommands::Uninstall { name, pool, yes } => {
            uninstall_skill(&client, format, &name, pool, yes).await
        }
    }
}

async fn list_skills(client: &ApiClient, format: OutputFormat, pool: bool) -> anyhow::Result<()> {
    let response = client
        .get(&skills_list_path(client.agent_id(), pool))
        .await?;
    match format {
        OutputFormat::Json => crate::output::print_json(&response),
        OutputFormat::Human => {
            let skills = skill_array(&response);
            if skills.is_empty() {
                println!("No skills found.");
            } else {
                println!("{}", if pool { "Skill Pool:" } else { "Skills:" });
                for skill in &skills {
                    let name = skill
                        .get("name")
                        .or_else(|| skill.get("id"))
                        .and_then(Value::as_str)
                        .unwrap_or("?");
                    let source = skill
                        .get("source")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown");
                    if pool {
                        println!("  {name} [{source}]");
                    } else {
                        let enabled = skill
                            .get("enabled")
                            .and_then(Value::as_bool)
                            .unwrap_or(false);
                        let status = if enabled { "enabled" } else { "disabled" };
                        println!("  {name} [{status}] [{source}]");
                    }
                }
            }
        }
    }
    Ok(())
}

async fn search_skills(
    client: &ApiClient,
    format: OutputFormat,
    query: &str,
    providers: Vec<String>,
    limit: u32,
    page: u32,
    lang: &str,
) -> anyhow::Result<()> {
    let selected = if providers.is_empty() {
        client
            .get("/api/market/providers")
            .await?
            .as_array()
            .into_iter()
            .flatten()
            .filter(|provider| {
                provider
                    .get("available")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            })
            .filter_map(|provider| provider.get("key").and_then(Value::as_str))
            .map(str::to_string)
            .collect::<BTreeSet<_>>()
    } else {
        providers.into_iter().collect()
    };
    if selected.is_empty() {
        anyhow::bail!("no Skill Market providers are available");
    }

    let provider_pages: BTreeMap<String, u32> = selected
        .iter()
        .cloned()
        .map(|provider| (provider, page.max(1)))
        .collect();
    let response = client
        .post(
            "/api/market/search",
            &serde_json::json!({
                "query": query,
                "provider_pages": provider_pages,
                "limit": limit,
                "lang": lang,
            }),
        )
        .await?;

    let errors = response
        .get("errors")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    match format {
        OutputFormat::Json => crate::output::print_json(&response),
        OutputFormat::Human => {
            for result in response
                .get("results")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let source = result.get("source").and_then(Value::as_str).unwrap_or("?");
                let name = result
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("unnamed");
                let version = result
                    .get("version")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                    .map(|value| format!(" v{value}"))
                    .unwrap_or_default();
                println!("{source}: {name}{version}");
                if let Some(description) = result.get("description").and_then(Value::as_str) {
                    if !description.is_empty() {
                        println!("  {description}");
                    }
                }
                if let Some(url) = result.get("source_url").and_then(Value::as_str) {
                    println!("  {url}");
                }
            }
            for error in &errors {
                let provider = error.get("provider").and_then(Value::as_str).unwrap_or("?");
                let message = error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("search failed");
                eprintln!("warning: [{provider}] {message}");
            }
        }
    }

    if errors.len() == selected.len() {
        anyhow::bail!("all selected Skill Market providers failed");
    }
    Ok(())
}

async fn install_skill(
    client: &ApiClient,
    format: OutputFormat,
    source_url: &str,
    version: Option<&str>,
    target_name: Option<&str>,
    pool: bool,
    enable: bool,
) -> anyhow::Result<()> {
    if pool {
        let response = client
            .post(
                "/api/skills/hub/install_pool",
                &serde_json::json!({
                    "url": source_url,
                    "version": version.unwrap_or_default(),
                    "target_name": target_name,
                }),
            )
            .await?;
        print_value(&response, format);
        return Ok(());
    }

    let started = client
        .post(
            &format!(
                "{}/hub/install/start",
                skills_list_endpoint(client.agent_id())
            ),
            &serde_json::json!({
                "bundle_url": source_url,
                "version": version.unwrap_or_default(),
                "enable": enable,
                "target_name": target_name.unwrap_or_default(),
            }),
        )
        .await?;
    let task_id = started
        .get("task_id")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("Skill install did not return task_id"))?
        .to_string();
    wait_for_install(client, format, &task_id).await
}

async fn wait_for_install(
    client: &ApiClient,
    format: OutputFormat,
    task_id: &str,
) -> anyhow::Result<()> {
    let mut last_status = String::new();
    let mut cancel_requested = false;
    loop {
        if cancel_requested {
            tokio::time::sleep(Duration::from_millis(300)).await;
        } else {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {
                    let _ = client
                        .post(
                            &format!("{}/hub/install/cancel/{task_id}", skills_list_endpoint(client.agent_id())),
                            &serde_json::json!({}),
                        )
                        .await;
                    cancel_requested = true;
                    eprintln!("cancellation requested; waiting for a stable install state...");
                }
                _ = tokio::time::sleep(Duration::from_millis(300)) => {}
            }
        }

        let status = client
            .get(&format!(
                "{}/hub/install/status/{task_id}",
                skills_list_endpoint(client.agent_id())
            ))
            .await?;
        let kind = status
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        if matches!(format, OutputFormat::Human) && kind != last_status {
            eprintln!("Skill install: {kind}");
            last_status = kind.to_string();
        }
        match kind {
            "completed" => {
                let output = status.get("result").unwrap_or(&status);
                print_value(output, format);
                return Ok(());
            }
            "failed" => {
                let message = status
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("Skill install failed");
                anyhow::bail!("{message}");
            }
            "cancelled" => anyhow::bail!("Skill install cancelled"),
            _ => {}
        }
    }
}

async fn import_skill(
    client: &ApiClient,
    format: OutputFormat,
    path: &Path,
    target_name: Option<&str>,
    pool: bool,
    replace: bool,
) -> anyhow::Result<()> {
    let archive = skill_archive_bytes(path)?;
    let mut params = vec![
        ("overwrite", replace.to_string()),
        ("enable", (!pool).to_string()),
    ];
    if let Some(name) = target_name {
        params.push(("target_name", name.to_string()));
    }
    let query = params
        .into_iter()
        .map(|(key, value)| format!("{key}={}", percent_encode_query(&value)))
        .collect::<Vec<_>>()
        .join("&");
    let endpoint = if pool {
        format!("/api/skills/pool/upload-zip?{query}")
    } else {
        format!("{}/upload?{query}", skills_list_endpoint(client.agent_id()))
    };
    let response = client.upload_bytes(&endpoint, archive.into()).await?;
    let conflicts = response
        .get("conflicts")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    print_value(&response, format);
    if conflicts > 0 {
        anyhow::bail!("Skill import has {conflicts} conflict(s)");
    }
    Ok(())
}

async fn update_skill(
    client: &ApiClient,
    format: OutputFormat,
    name: &str,
    pool: bool,
    force: bool,
) -> anyhow::Result<()> {
    let check = client
        .post(
            &skill_update_path(client.agent_id(), name, pool, true),
            &serde_json::json!({}),
        )
        .await?;
    let has_update = check
        .get("has_update")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !has_update && !force {
        print_value(
            &serde_json::json!({
                "name": name,
                "updated": false,
                "current_version": check.get("current_version"),
                "latest_version": check.get("latest_version"),
            }),
            format,
        );
        return Ok(());
    }
    let response = client
        .post(
            &skill_update_path(client.agent_id(), name, pool, false),
            &serde_json::json!({"force": force}),
        )
        .await?;
    print_value(&response, format);
    Ok(())
}

async fn set_skill_enabled(
    client: &ApiClient,
    format: OutputFormat,
    name: &str,
    enabled: bool,
) -> anyhow::Result<()> {
    let current = client.get(&skill_endpoint(client.agent_id(), name)).await?;
    if current.get("enabled").and_then(Value::as_bool) == Some(enabled) {
        print_value(
            &serde_json::json!({"name": name, "enabled": enabled, "changed": false}),
            format,
        );
        return Ok(());
    }
    let endpoint = if enabled {
        skill_enable_endpoint(client.agent_id(), name)
    } else {
        skill_disable_endpoint(client.agent_id(), name)
    };
    client.post(&endpoint, &serde_json::json!({})).await?;
    print_value(
        &serde_json::json!({"name": name, "enabled": enabled, "changed": true}),
        format,
    );
    Ok(())
}

async fn configure_skill(
    client: &ApiClient,
    format: OutputFormat,
    name: &str,
) -> anyhow::Result<()> {
    let skill = match client.get(&skill_endpoint(client.agent_id(), name)).await {
        Ok(value) => value,
        Err(error) if error_status(&error) == Some(StatusCode::NOT_FOUND) => {
            anyhow::bail!("Skill '{name}' not found")
        }
        Err(error) => return Err(error),
    };
    let current_content = skill.get("content").and_then(Value::as_str).unwrap_or("");
    let edited = open_in_editor(current_content)?;
    if edited.trim() == current_content.trim() {
        println!("No changes made.");
        return Ok(());
    }

    let body = serde_json::json!({
        "name": name,
        "source_name": skill.get("source_name").and_then(Value::as_str).unwrap_or(""),
        "content": edited,
        "description": skill.get("description").and_then(Value::as_str).unwrap_or(""),
        "overwrite": true,
    });
    let response = client
        .put(&skill_save_endpoint(client.agent_id()), &body)
        .await?;
    print_value(&response, format);
    Ok(())
}

async fn uninstall_skill(
    client: &ApiClient,
    format: OutputFormat,
    name: &str,
    pool: bool,
    yes: bool,
) -> anyhow::Result<()> {
    if !skill_exists(client, name, pool).await? {
        print_value(
            &serde_json::json!({"name": name, "removed": false, "changed": false}),
            format,
        );
        return Ok(());
    }
    if !yes && !confirm_destructive(&format!("Uninstall skill '{name}'?"))? {
        anyhow::bail!("aborted");
    }
    let endpoint = if pool {
        format!("/api/skills/pool/{name}")
    } else {
        skill_endpoint(client.agent_id(), name)
    };
    client.delete(&endpoint).await?;
    print_value(
        &serde_json::json!({"name": name, "removed": true, "changed": true}),
        format,
    );
    Ok(())
}

async fn skill_exists(client: &ApiClient, name: &str, pool: bool) -> anyhow::Result<bool> {
    if pool {
        let response = client
            .get(&skills_list_path(client.agent_id(), true))
            .await?;
        Ok(skill_array(&response).iter().any(|skill| {
            skill
                .get("name")
                .or_else(|| skill.get("id"))
                .and_then(Value::as_str)
                == Some(name)
        }))
    } else {
        match client.get(&skill_endpoint(client.agent_id(), name)).await {
            Ok(_) => Ok(true),
            Err(error) if error_status(&error) == Some(StatusCode::NOT_FOUND) => Ok(false),
            Err(error) => Err(error),
        }
    }
}

async fn get_skill(client: &ApiClient, name: &str, pool: bool) -> anyhow::Result<Value> {
    if !pool {
        return client.get(&skill_endpoint(client.agent_id(), name)).await;
    }
    let response = client
        .get(&skills_list_path(client.agent_id(), true))
        .await?;
    skill_array(&response)
        .into_iter()
        .find(|skill| {
            skill
                .get("name")
                .or_else(|| skill.get("id"))
                .and_then(Value::as_str)
                == Some(name)
        })
        .ok_or_else(|| anyhow::anyhow!("Skill Pool entry '{name}' not found"))
}

fn skill_array(response: &Value) -> Vec<Value> {
    response
        .get("skills")
        .and_then(Value::as_array)
        .cloned()
        .or_else(|| response.as_array().cloned())
        .unwrap_or_default()
}

fn skill_update_path(agent_id: &str, name: &str, pool: bool, check: bool) -> String {
    match (pool, check) {
        (true, true) => format!("/api/skills/pool/{name}/check-update"),
        (true, false) => format!("/api/skills/pool/{name}/update"),
        (false, true) => format!("{}/check-update", skill_endpoint(agent_id, name)),
        (false, false) => format!("{}/update", skill_endpoint(agent_id, name)),
    }
}

fn skill_archive_bytes(path: &Path) -> anyhow::Result<Vec<u8>> {
    if path.is_file() {
        if path.extension().and_then(|ext| ext.to_str()) != Some("zip") {
            anyhow::bail!("local Skill import file must be a .zip archive");
        }
        return std::fs::read(path)
            .map_err(|error| anyhow::anyhow!("failed to read '{}': {error}", path.display()));
    }
    if !path.is_dir() {
        anyhow::bail!("Skill import path does not exist: {}", path.display());
    }

    let cursor = Cursor::new(Vec::new());
    let mut writer = ZipWriter::new(cursor);
    append_directory_to_zip(&mut writer, path, path)?;
    let cursor = writer
        .finish()
        .map_err(|error| anyhow::anyhow!("finish Skill ZIP: {error}"))?;
    Ok(cursor.into_inner())
}

fn append_directory_to_zip<W: Write + Seek>(
    writer: &mut ZipWriter<W>,
    root: &Path,
    directory: &Path,
) -> anyhow::Result<()> {
    let mut entries: Vec<_> = std::fs::read_dir(directory)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            anyhow::bail!("Skill import does not follow symlink: {}", path.display());
        }
        let relative = path
            .strip_prefix(root)
            .expect("directory entry is below root");
        let archive_name = relative.to_string_lossy().replace('\\', "/");
        if metadata.is_dir() {
            writer.add_directory(
                format!("{archive_name}/"),
                SimpleFileOptions::default().compression_method(CompressionMethod::Deflated),
            )?;
            append_directory_to_zip(writer, root, &path)?;
        } else if metadata.is_file() {
            let options =
                SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
            #[cfg(unix)]
            let options = {
                use std::os::unix::fs::PermissionsExt as _;
                options.unix_permissions(metadata.permissions().mode())
            };
            writer.start_file(archive_name, options)?;
            let mut file = std::fs::File::open(&path)?;
            std::io::copy(&mut file, writer)?;
        }
    }
    Ok(())
}

fn percent_encode_query(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

fn confirm_destructive(prompt: &str) -> anyhow::Result<bool> {
    if !std::io::stdin().is_terminal() {
        anyhow::bail!("confirmation requires a terminal; pass --yes for non-interactive use");
    }
    eprint!("{prompt} [y/N] ");
    std::io::stderr().flush()?;
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    Ok(matches!(
        input.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

pub fn skills_list_endpoint(agent_id: &str) -> String {
    skill_path_segments(&["api", "agents", agent_id, "skills"])
}

fn skill_path_segments(parts: &[&str]) -> String {
    let mut url = reqwest::Url::parse("http://localhost").expect("valid base URL");
    url.path_segments_mut()
        .expect("base URL supports path segments")
        .extend(parts);
    url.path().to_string()
}

fn skills_list_path(agent_id: &str, pool: bool) -> String {
    if pool {
        "/api/skills/pool".into()
    } else {
        skills_list_endpoint(agent_id)
    }
}

pub fn skill_endpoint(agent_id: &str, skill_id: &str) -> String {
    skill_path_segments(&["api", "agents", agent_id, "skills", skill_id])
}
pub fn skill_enable_endpoint(agent_id: &str, skill_id: &str) -> String {
    format!("{}/enable", skill_endpoint(agent_id, skill_id))
}
pub fn skill_disable_endpoint(agent_id: &str, skill_id: &str) -> String {
    format!("{}/disable", skill_endpoint(agent_id, skill_id))
}
pub fn skill_save_endpoint(agent_id: &str) -> String {
    format!("{}/save", skills_list_endpoint(agent_id))
}

fn open_in_editor(content: &str) -> anyhow::Result<String> {
    let editor = std::env::var("EDITOR")
        .or_else(|_| std::env::var("VISUAL"))
        .unwrap_or_else(|_| "vi".to_string());
    let path = std::env::temp_dir().join(format!("rua-skill-{}.txt", std::process::id()));
    std::fs::write(&path, content)?;
    let status = std::process::Command::new(&editor).arg(&path).status()?;
    if !status.success() {
        let _ = std::fs::remove_file(&path);
        anyhow::bail!("Editor exited with non-zero status");
    }
    let mut edited = String::new();
    std::fs::File::open(&path)?.read_to_string(&mut edited)?;
    let _ = std::fs::remove_file(&path);
    Ok(edited)
}
