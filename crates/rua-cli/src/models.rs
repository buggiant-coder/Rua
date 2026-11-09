use clap::{Args, Subcommand};

use crate::http_client::{resolve_base_url, ApiClient};

#[derive(Debug, Args)]
pub struct ModelsArgs {
    #[command(subcommand)]
    pub command: ModelsCommands,
}

#[derive(Debug, Subcommand)]
pub enum ModelsCommands {
    List,

    Set {
        model: String,
    },

    AddProvider {
        provider_id: String,

        #[arg(long, short = 'n')]
        name: String,

        #[arg(long, short = 'u', default_value = "")]
        base_url: String,

        #[arg(long, default_value = "")]
        api_key: String,
    },

    RemoveProvider {
        provider_id: String,
    },

    AddModel {
        provider_id: String,

        #[arg(long, short = 'm')]
        model_id: String,

        #[arg(long)]
        model_name: String,
    },

    RemoveModel {
        provider_id: String,

        #[arg(long, short = 'm')]
        model_id: String,
    },
}

pub fn run(args: ModelsArgs) -> anyhow::Result<()> {
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(run_async(args))
}

async fn run_async(args: ModelsArgs) -> anyhow::Result<()> {
    let client = ApiClient::new(&resolve_base_url(None), "default");

    match args.command {
        ModelsCommands::List => list_models(&client).await,
        ModelsCommands::Set { model } => set_model(&client, &model).await,
        ModelsCommands::AddProvider {
            provider_id,
            name,
            base_url,
            api_key,
        } => add_provider(&client, &provider_id, &name, &base_url, &api_key).await,
        ModelsCommands::RemoveProvider { provider_id } => {
            remove_provider(&client, &provider_id).await
        }
        ModelsCommands::AddModel {
            provider_id,
            model_id,
            model_name,
        } => add_model(&client, &provider_id, &model_id, &model_name).await,
        ModelsCommands::RemoveModel {
            provider_id,
            model_id,
        } => remove_model(&client, &provider_id, &model_id).await,
    }
}

struct ModelRecord {
    provider_id: String,
    model_id: String,
}

type ActiveInfo = Option<(String, String)>;
type ModelListResult = anyhow::Result<(Vec<ModelRecord>, ActiveInfo)>;

async fn list_models_from_api(client: &ApiClient) -> ModelListResult {
    let providers = client.get("/api/models").await?;
    let active_resp = client.get("/api/models/active").await?;

    let active_provider = active_resp
        .get("provider_id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let default_model = active_resp
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    let active = if !active_provider.is_empty() && !default_model.is_empty() {
        Some((active_provider, default_model))
    } else {
        None
    };

    let mut records = Vec::new();
    if let Some(arr) = providers.as_array() {
        for p in arr {
            let pid = p.get("id").and_then(|v| v.as_str()).unwrap_or_default();
            for key in &["models", "extra_models"] {
                if let Some(models) = p.get(*key).and_then(|v| v.as_array()) {
                    for m in models {
                        let mid = m.get("id").and_then(|v| v.as_str()).unwrap_or_default();
                        if !mid.is_empty() {
                            records.push(ModelRecord {
                                provider_id: pid.to_string(),
                                model_id: mid.to_string(),
                            });
                        }
                    }
                }
            }
        }
    }

    Ok((records, active))
}

fn list_models_from_config() -> ModelListResult {
    let config_path = rua_config::get_config_path(&rua_config::dirs::rua_root());
    if !config_path.is_file() {
        anyhow::bail!(
            "Config file not found: {}. Is rua configured?",
            config_path.display()
        );
    }

    let content = std::fs::read_to_string(&config_path)?;
    let root: serde_json::Value = serde_json::from_str(&content)?;

    let mut records = Vec::new();
    if let Some(providers) = root.get("providers").and_then(|v| v.as_array()) {
        for p in providers {
            let pid = p.get("id").and_then(|v| v.as_str()).unwrap_or_default();
            for key in &["models", "extra_models"] {
                if let Some(models) = p.get(*key).and_then(|v| v.as_array()) {
                    for m in models {
                        let mid = m.get("id").and_then(|v| v.as_str()).unwrap_or_default();
                        if !mid.is_empty() {
                            records.push(ModelRecord {
                                provider_id: pid.to_string(),
                                model_id: mid.to_string(),
                            });
                        }
                    }
                }
            }
        }
    }

    let active = root.get("default_model").and_then(|am| {
        let pid = am.get("provider_id").and_then(|v| v.as_str())?;
        let mid = am.get("model").and_then(|v| v.as_str())?;
        Some((pid.to_string(), mid.to_string()))
    });

    Ok((records, active))
}

async fn list_models(client: &ApiClient) -> anyhow::Result<()> {
    let (records, active) = match list_models_from_api(client).await {
        Ok(result) => result,
        Err(_) => list_models_from_config()?,
    };

    if records.is_empty() {
        println!("No models found.");
        return Ok(());
    }

    let pid_width = records
        .iter()
        .map(|r| r.provider_id.len())
        .max()
        .unwrap_or(8)
        .max(8);
    let mid_width = records
        .iter()
        .map(|r| r.model_id.len())
        .max()
        .unwrap_or(5)
        .max(5);

    println!(
        "{:<pid_width$}  {:<mid_width$}  ACTIVE",
        "PROVIDER", "MODEL"
    );
    for r in &records {
        let is_active = active
            .as_ref()
            .map(|(ap, am)| ap == &r.provider_id && am == &r.model_id)
            .unwrap_or(false);
        let mark = if is_active { "*" } else { "" };
        println!(
            "{:<pid_width$}  {:<mid_width$}  {mark}",
            r.provider_id, r.model_id
        );
    }

    Ok(())
}

async fn set_model(client: &ApiClient, model_input: &str) -> anyhow::Result<()> {
    let target_provider = None;
    let target_model = model_input.to_string();

    let try_api = set_model_via_api(client, target_provider, &target_model).await;
    match try_api {
        Ok(()) => return Ok(()),
        Err(e) => {
            let is_connection_error = e
                .downcast_ref::<reqwest::Error>()
                .map(|re| re.is_connect() || re.is_timeout())
                .unwrap_or(false);
            if !is_connection_error {
                return Err(e);
            }
            set_model_via_config(target_provider, &target_model)?;
        }
    }

    Ok(())
}

async fn set_model_via_api(
    client: &ApiClient,
    target_provider: Option<&str>,
    target_model: &str,
) -> anyhow::Result<()> {
    let providers_resp = client.get("/api/models").await?;

    let mut found_provider = None;
    if let Some(arr) = providers_resp.as_array() {
        for p in arr {
            let pid = p.get("id").and_then(|v| v.as_str()).unwrap_or_default();
            if let Some(tp) = target_provider {
                if pid != tp {
                    continue;
                }
            }
            for key in &["models", "extra_models"] {
                if let Some(models) = p.get(*key).and_then(|v| v.as_array()) {
                    for m in models {
                        let mid = m.get("id").and_then(|v| v.as_str()).unwrap_or_default();
                        if mid == target_model {
                            found_provider = Some(pid.to_string());
                            break;
                        }
                    }
                }
                if found_provider.is_some() {
                    break;
                }
            }
            if found_provider.is_some() {
                break;
            }
        }
    }

    let provider_id = found_provider.ok_or_else(|| {
        if let Some(tp) = target_provider {
            anyhow::anyhow!("Model '{}' not found in provider '{}'", target_model, tp)
        } else {
            anyhow::anyhow!(
                "Model '{}' not found in any provider. Run `rua models list` to see available models.",
                target_model
            )
        }
    })?;

    let body = serde_json::json!({
        "provider_id": provider_id,
        "model": target_model,
    });
    client.put("/api/models/active", &body).await?;

    println!("Active model set to {provider_id}/{target_model}");
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

    println!("Custom provider '{resp_name}' ({resp_id}) created.");
    if resp_id != provider_id {
        println!("  requested id: {provider_id}");
    }
    if !base_url.is_empty() {
        println!("  base_url: {base_url}");
    }
    println!("  Run 'rua models add-model' to add models, then 'rua models config-key' to set the API key.");

    Ok(())
}

async fn remove_provider(client: &ApiClient, provider_id: &str) -> anyhow::Result<()> {
    let path = format!("/api/models/providers/{provider_id}");
    client.delete(&path).await?;
    println!("Provider '{provider_id}' deleted.");
    Ok(())
}

async fn add_model(
    client: &ApiClient,
    provider_id: &str,
    model_id: &str,
    model_name: &str,
) -> anyhow::Result<()> {
    let body = serde_json::json!({
        "id": model_id,
        "name": model_name,
    });
    let path = format!("/api/models/{provider_id}/models");

    match client.post(&path, &body).await {
        Ok(_) => {
            println!("Model '{model_name}' ({model_id}) added to '{provider_id}'.");
            Ok(())
        }
        Err(e) => {
            let is_connection_error = e
                .downcast_ref::<reqwest::Error>()
                .map(|re| re.is_connect() || re.is_timeout())
                .unwrap_or(false);
            if !is_connection_error {
                return Err(e);
            }
            add_model_via_config(provider_id, model_id, model_name)
        }
    }
}

fn add_model_via_config(provider_id: &str, model_id: &str, model_name: &str) -> anyhow::Result<()> {
    let config_path = rua_config::get_config_path(&rua_config::dirs::rua_root());
    if !config_path.is_file() {
        anyhow::bail!(
            "Config file not found: {}. Is rua configured?",
            config_path.display()
        );
    }

    let content = std::fs::read_to_string(&config_path)?;
    let mut root: serde_json::Value = serde_json::from_str(&content)?;

    let providers = root
        .get_mut("providers")
        .and_then(|v| v.as_array_mut())
        .ok_or_else(|| anyhow::anyhow!("No providers found in config"))?;

    let provider = providers
        .iter_mut()
        .find(|p| p.get("id").and_then(|v| v.as_str()) == Some(provider_id))
        .ok_or_else(|| anyhow::anyhow!("Provider '{provider_id}' not found"))?;

    for key in &["models", "extra_models"] {
        if let Some(models) = provider.get(*key).and_then(|v| v.as_array()) {
            if models
                .iter()
                .any(|m| m.get("id").and_then(|v| v.as_str()) == Some(model_id))
            {
                anyhow::bail!("Model '{model_id}' already exists in provider '{provider_id}'");
            }
        }
    }

    let new_model = serde_json::json!({ "id": model_id, "name": model_name });
    let extra_models = provider
        .get_mut("extra_models")
        .and_then(|v| v.as_array_mut());
    match extra_models {
        Some(arr) => arr.push(new_model),
        None => {
            provider["extra_models"] = serde_json::json!([new_model]);
        }
    }

    let output = serde_json::to_string_pretty(&root)?;
    std::fs::write(&config_path, output)?;

    println!("Model '{model_name}' ({model_id}) added to '{provider_id}'.");
    Ok(())
}

async fn remove_model(client: &ApiClient, provider_id: &str, model_id: &str) -> anyhow::Result<()> {
    let path = format!("/api/models/{provider_id}/models/delete");
    let body = serde_json::json!({ "model_id": model_id });

    match client.post(&path, &body).await {
        Ok(_) => {
            println!("Model '{model_id}' removed from '{provider_id}'.");
            Ok(())
        }
        Err(e) => {
            let is_connection_error = e
                .downcast_ref::<reqwest::Error>()
                .map(|re| re.is_connect() || re.is_timeout())
                .unwrap_or(false);
            if !is_connection_error {
                return Err(e);
            }
            remove_model_via_config(provider_id, model_id)
        }
    }
}

fn remove_model_via_config(provider_id: &str, model_id: &str) -> anyhow::Result<()> {
    let config_path = rua_config::get_config_path(&rua_config::dirs::rua_root());
    if !config_path.is_file() {
        anyhow::bail!(
            "Config file not found: {}. Is rua configured?",
            config_path.display()
        );
    }

    let content = std::fs::read_to_string(&config_path)?;
    let mut root: serde_json::Value = serde_json::from_str(&content)?;

    let providers = root
        .get_mut("providers")
        .and_then(|v| v.as_array_mut())
        .ok_or_else(|| anyhow::anyhow!("No providers found in config"))?;

    let provider = providers
        .iter_mut()
        .find(|p| p.get("id").and_then(|v| v.as_str()) == Some(provider_id))
        .ok_or_else(|| anyhow::anyhow!("Provider '{provider_id}' not found"))?;

    let mut removed = false;
    for key in &["models", "extra_models"] {
        if let Some(models) = provider.get_mut(*key).and_then(|v| v.as_array_mut()) {
            let before = models.len();
            models.retain(|m| m.get("id").and_then(|v| v.as_str()) != Some(model_id));
            if models.len() < before {
                removed = true;
            }
        }
    }

    if !removed {
        anyhow::bail!("Model '{model_id}' not found in provider '{provider_id}'");
    }

    let output = serde_json::to_string_pretty(&root)?;
    std::fs::write(&config_path, output)?;

    println!("Model '{model_id}' removed from '{provider_id}'.");
    Ok(())
}

fn set_model_via_config(target_provider: Option<&str>, target_model: &str) -> anyhow::Result<()> {
    let config_path = rua_config::get_config_path(&rua_config::dirs::rua_root());
    if !config_path.is_file() {
        anyhow::bail!(
            "Config file not found: {}. Is rua configured?",
            config_path.display()
        );
    }

    let content = std::fs::read_to_string(&config_path)?;
    let mut root: serde_json::Value = serde_json::from_str(&content)?;

    let mut found_provider = None;
    if let Some(providers) = root.get("providers").and_then(|v| v.as_array()) {
        for p in providers {
            let pid = p.get("id").and_then(|v| v.as_str()).unwrap_or_default();
            if let Some(tp) = target_provider {
                if pid != tp {
                    continue;
                }
            }
            for key in &["models", "extra_models"] {
                if let Some(models) = p.get(*key).and_then(|v| v.as_array()) {
                    for m in models {
                        let mid = m.get("id").and_then(|v| v.as_str()).unwrap_or_default();
                        if mid == target_model {
                            found_provider = Some(pid.to_string());
                            break;
                        }
                    }
                }
                if found_provider.is_some() {
                    break;
                }
            }
            if found_provider.is_some() {
                break;
            }
        }
    }

    let provider_id = found_provider.ok_or_else(|| {
        if let Some(tp) = target_provider {
            anyhow::anyhow!("Model '{}' not found in provider '{}'", target_model, tp)
        } else {
            anyhow::anyhow!(
                "Model '{}' not found in config. Run `rua models list` to see available models.",
                target_model
            )
        }
    })?;

    root["default_model"] = serde_json::json!({
        "provider_id": provider_id,
        "model": target_model,
    });

    let output = serde_json::to_string_pretty(&root)?;
    std::fs::write(&config_path, output)?;

    println!("Active model set to {provider_id}/{target_model}");
    Ok(())
}
