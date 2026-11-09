use clap::{Args, Subcommand};
use rua_config::dirs::Paths;

use crate::http_client::{resolve_base_url, ApiClient};
use crate::output::OutputFormat;

#[derive(Debug, Args)]
pub struct ProvidersArgs {
    #[command(subcommand)]
    pub command: ProvidersCommands,
}

#[derive(Debug, Subcommand)]
pub enum ProvidersCommands {
    List,

    Add {
        provider_id: String,

        #[arg(long, short = 'n')]
        name: String,

        #[arg(long, short = 'u', default_value = "")]
        base_url: String,

        #[arg(long, default_value = "")]
        api_key: String,
    },

    Remove {
        provider_id: String,
    },

    Test {
        provider_id: String,
    },

    Get {
        provider_id: String,

        #[arg(long)]
        show_secrets: bool,
    },

    SetApiKey {
        provider_id: String,

        api_key: Option<String>,
    },
}

pub fn run(
    args: ProvidersArgs,
    format: OutputFormat,
    base_url: Option<&str>,
    agent_id: &str,
    paths: &Paths,
) -> anyhow::Result<()> {
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(run_async(args, format, base_url, agent_id, paths))
}

async fn run_async(
    args: ProvidersArgs,
    _format: OutputFormat,
    base_url: Option<&str>,
    agent_id: &str,
    paths: &Paths,
) -> anyhow::Result<()> {
    let client = ApiClient::new(&resolve_base_url(base_url), agent_id);

    match args.command {
        ProvidersCommands::List => list_providers(&client).await,
        ProvidersCommands::Add {
            provider_id,
            name,
            base_url,
            api_key,
        } => add_provider(&client, &provider_id, &name, &base_url, &api_key).await,
        ProvidersCommands::Remove { provider_id } => remove_provider(&client, &provider_id).await,
        ProvidersCommands::Test { provider_id } => test_provider(&client, &provider_id).await,
        ProvidersCommands::Get {
            provider_id,
            show_secrets,
        } => get_local_provider(paths, &provider_id, show_secrets).await,
        ProvidersCommands::SetApiKey {
            provider_id,
            api_key,
        } => set_local_provider_api_key(paths, &provider_id, api_key).await,
    }
}

async fn list_providers(client: &ApiClient) -> anyhow::Result<()> {
    let providers = match client.get("/api/models").await {
        Ok(v) => v,
        Err(_) => return list_providers_from_config(),
    };

    let arr = providers.as_array().map(|a| a.as_slice()).unwrap_or(&[]);
    if arr.is_empty() {
        println!("No providers configured.");
        return Ok(());
    }

    let id_width = arr
        .iter()
        .map(|p| p.get("id").and_then(|v| v.as_str()).unwrap_or("").len())
        .max()
        .unwrap_or(2)
        .max(2);
    let name_width = arr
        .iter()
        .map(|p| p.get("name").and_then(|v| v.as_str()).unwrap_or("").len())
        .max()
        .unwrap_or(4)
        .max(4);

    println!("{:<id_width$}  {:<name_width$}  MODELS", "ID", "NAME");
    for p in arr {
        let id = p.get("id").and_then(|v| v.as_str()).unwrap_or("-");
        let name = p.get("name").and_then(|v| v.as_str()).unwrap_or("-");
        let model_count: usize = ["models", "extra_models"]
            .iter()
            .filter_map(|k| p.get(*k).and_then(|v| v.as_array()).map(|a| a.len()))
            .sum();
        println!("{:<id_width$}  {:<name_width$}  {model_count}", id, name);
    }

    Ok(())
}

fn list_providers_from_config() -> anyhow::Result<()> {
    let config_path = rua_config::get_config_path(&rua_config::dirs::rua_root());
    if !config_path.is_file() {
        println!("No providers configured (config file not found).");
        return Ok(());
    }

    let content = std::fs::read_to_string(&config_path)?;
    let root: serde_json::Value = serde_json::from_str(&content)?;

    let providers = root
        .get("providers")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    if providers.is_empty() {
        println!("No providers configured.");
    } else {
        println!("{:<16}  {:<20}  MODELS", "ID", "NAME");
        for p in &providers {
            let id = p.get("id").and_then(|v| v.as_str()).unwrap_or("-");
            let name = p.get("name").and_then(|v| v.as_str()).unwrap_or("-");
            let model_count: usize = ["models", "extra_models"]
                .iter()
                .filter_map(|k| p.get(*k).and_then(|v| v.as_array()).map(|a| a.len()))
                .sum();
            println!("{:<16}  {:<20}  {model_count}", id, name);
        }
    }

    Ok(())
}

async fn add_provider(
    client: &ApiClient,
    provider_id: &str,
    name: &str,
    base_url: &str,
    api_key: &str,
) -> anyhow::Result<()> {
    let body = serde_json::json!({
        "id": provider_id,
        "name": name,
        "base_url": base_url,
        "api_key": api_key,
        "models": [],
    });

    let resp = client.post("/api/models/custom-providers", &body).await?;

    let resp_id = resp
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or(provider_id);
    let resp_name = resp.get("name").and_then(|v| v.as_str()).unwrap_or(name);

    println!("Provider '{resp_name}' ({resp_id}) added.");
    if !base_url.is_empty() {
        println!("  base_url: {base_url}");
    }
    println!("  Next: rua models add-model {provider_id} -m <model_id> --model-name <name>");

    Ok(())
}

async fn remove_provider(client: &ApiClient, provider_id: &str) -> anyhow::Result<()> {
    let path = format!("/api/models/providers/{provider_id}");
    client.delete(&path).await?;
    println!("Provider '{provider_id}' removed.");
    Ok(())
}

async fn test_provider(client: &ApiClient, provider_id: &str) -> anyhow::Result<()> {
    let providers = client.get("/api/models").await?;

    let arr = providers.as_array().map(|a| a.as_slice()).unwrap_or(&[]);
    let provider = arr
        .iter()
        .find(|p| p.get("id").and_then(|v| v.as_str()) == Some(provider_id));

    let provider = match provider {
        Some(p) => p,
        None => {
            anyhow::bail!("Provider '{provider_id}' not found. Run `rua providers list`.");
        }
    };

    let base = provider
        .get("base_url")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if base.is_empty() {
        println!(
            "Provider '{provider_id}' has no base_url configured — skipping connectivity test."
        );
        return Ok(());
    }

    println!("Testing connectivity to {base} ...");
    let http = rua_http::build_client(
        reqwest::Client::builder().timeout(std::time::Duration::from_secs(10)),
    )?;
    match http.get(base).send().await {
        Ok(resp) => {
            println!("  Connection OK (HTTP {})", resp.status().as_u16());
        }
        Err(e) => {
            println!("  Connection FAILED: {e}");
        }
    }

    Ok(())
}

async fn get_local_provider(
    paths: &Paths,
    provider_id: &str,
    show_secrets: bool,
) -> anyhow::Result<()> {
    let loader = local_provider_loader(paths)?;
    let config = loader.load_provider(provider_id).await?;
    let mut value = serde_json::to_value(&config)?;
    if !show_secrets {
        redact_provider_value(&mut value);
    }
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

async fn set_local_provider_api_key(
    paths: &Paths,
    provider_id: &str,
    api_key: Option<String>,
) -> anyhow::Result<()> {
    let api_key = match api_key {
        Some(v) => v,
        None => rpassword::prompt_password_stderr(&format!("  API key for {provider_id}: "))?,
    };
    let loader = local_provider_loader(paths)?;
    let mut config = loader.load_provider(provider_id).await?;
    config.api_key = api_key;
    loader.save_provider(&config).await?;
    println!("Provider '{provider_id}' API key updated.");
    Ok(())
}

fn local_provider_loader(paths: &Paths) -> anyhow::Result<rua_provider::ProviderConfigLoader> {
    let device_key = rua_sync::crypto::DeviceKey::generate_or_load(&paths.secret_dir)?;
    Ok(rua_provider::ProviderConfigLoader::with_key(
        paths.secret_dir.clone(),
        *device_key.as_bytes(),
    ))
}

fn redact_provider_value(value: &mut serde_json::Value) {
    if let Some(obj) = value.as_object_mut() {
        if obj.get("api_key").and_then(|v| v.as_str()).is_some() {
            obj.insert("api_key".into(), serde_json::Value::String("***".into()));
        }
        if obj
            .get("api_keys")
            .and_then(|v| v.as_array())
            .is_some_and(|keys| !keys.is_empty())
        {
            obj.insert("api_keys".into(), serde_json::json!(["***"]));
        }
        if let Some(headers) = obj.get_mut("headers").and_then(|v| v.as_object_mut()) {
            for (key, value) in headers {
                if key.to_lowercase().contains("authorization") {
                    *value = serde_json::Value::String("***".into());
                }
            }
        }
    }
}
