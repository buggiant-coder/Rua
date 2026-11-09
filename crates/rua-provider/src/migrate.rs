use std::path::Path;

use serde_json::Value;
use tracing::{info, warn};

use crate::config_loader::{DefaultModel, ProviderConfig, ProviderModelEntry};

pub fn migrate_legacy_providers(secret_dir: &Path) -> Result<(), MigrateError> {
    let legacy_path = secret_dir.join("providers.json");
    if !legacy_path.is_file() {
        return Ok(());
    }

    info!(path = %legacy_path.display(), "detected legacy providers.json, starting migration");

    let ts = chrono::Utc::now().format("%Y%m%d%H%M%S");
    let backup_path = secret_dir.join(format!("providers.json.bak.{ts}"));
    std::fs::copy(&legacy_path, &backup_path).map_err(|e| MigrateError {
        message: format!("failed to backup legacy providers.json: {e}"),
    })?;
    info!(backup = %backup_path.display(), "created backup");

    let raw = std::fs::read_to_string(&legacy_path).map_err(|e| MigrateError {
        message: format!("failed to read legacy providers.json: {e}"),
    })?;
    let legacy: Value = serde_json::from_str(&raw).map_err(|e| {
        restore_backup(&backup_path, &legacy_path);
        MigrateError {
            message: format!("invalid JSON in legacy providers.json: {e}"),
        }
    })?;

    let result = do_migrate(secret_dir, &legacy);
    if let Err(e) = &result {
        warn!(error = %e.message, "migration failed, restoring backup");
        restore_backup(&backup_path, &legacy_path);
        return Err(MigrateError {
            message: format!("migration failed (backup restored): {}", e.message),
        });
    }

    if let Err(e) = std::fs::remove_file(&legacy_path) {
        warn!(error = %e, "failed to remove legacy providers.json after migration");
    }

    info!("legacy providers.json migration complete");
    Ok(())
}

fn do_migrate(secret_dir: &Path, legacy: &Value) -> Result<(), MigrateError> {
    let providers_dir = secret_dir.join("providers").join("providers");
    std::fs::create_dir_all(&providers_dir).map_err(|e| MigrateError {
        message: format!("failed to create providers directory: {e}"),
    })?;

    if let Some(builtins) = legacy.get("providers").and_then(|v| v.as_object()) {
        for (provider_id, config) in builtins {
            let pc = parse_legacy_provider(provider_id, config, false);
            write_provider_file(&providers_dir, &pc)?;
        }
    }

    if let Some(customs) = legacy.get("custom_providers").and_then(|v| v.as_object()) {
        for (provider_id, data) in customs {
            let pc = parse_legacy_provider(provider_id, data, true);
            write_provider_file(&providers_dir, &pc)?;
        }
    }

    if let Some(default_llm) = legacy.get("default_llm") {
        if default_llm.is_object() && !default_llm.as_object().unwrap().is_empty() {
            migrate_default_model(secret_dir, default_llm)?;
        }
    }

    Ok(())
}

fn parse_legacy_provider(id: &str, data: &Value, is_custom: bool) -> ProviderConfig {
    let obj = data.as_object();
    let s = |key: &str| -> String {
        obj.and_then(|o| o.get(key))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };

    let models_key = if is_custom { "models" } else { "extra_models" };

    let extra_models: Vec<ProviderModelEntry> = obj
        .and_then(|o| o.get(models_key))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| serde_json::from_value::<ProviderModelEntry>(m.clone()).ok())
                .collect()
        })
        .unwrap_or_default();

    let chat_model = obj
        .and_then(|o| o.get("chat_model"))
        .and_then(|v| v.as_str())
        .map(String::from);

    ProviderConfig {
        id: id.to_string(),
        name: if s("name").is_empty() {
            id.to_string()
        } else {
            s("name")
        },
        base_url: s("base_url"),
        api_key: s("api_key"),
        api_keys: Vec::new(),
        api_key_prefixes: Vec::new(),
        headers: serde_json::Map::new(),
        custom_headers: serde_json::Map::new(),
        chat_model,
        models: Vec::new(),
        extra_models,
        timeout: obj.and_then(|o| o.get("timeout")).and_then(|v| v.as_u64()),
        generate_kwargs: serde_json::Map::new(),
        support_model_discovery: false,
        support_connection_check: false,
        is_local: false,
        api_key_prefix: String::new(),
        is_custom,
        is_plugin: false,
        freeze_url: false,
        require_api_key: false,
        provider_group: String::new(),
        provider_group_name: String::new(),
        provider_variant: String::new(),
        base_url_options: Vec::new(),
        thinking_param_style: None,
        reasoning_effort_options: Vec::new(),
        thinking_budget_range: None,
        extra: serde_json::Map::new(),
    }
}

fn write_provider_file(providers_dir: &Path, config: &ProviderConfig) -> Result<(), MigrateError> {
    let path = providers_dir.join(format!("{}.json", config.id));
    let value = serde_json::to_value(config).map_err(|e| MigrateError {
        message: format!("failed to serialize provider '{}': {e}", config.id),
    })?;
    rua_config::atomic_write_json(&path, &value).map_err(|e| MigrateError {
        message: format!(
            "failed to write provider '{}' to {}: {e}",
            config.id,
            path.display()
        ),
    })?;
    info!(provider_id = %config.id, path = %path.display(), "migrated provider");
    Ok(())
}

fn migrate_default_model(secret_dir: &Path, default_llm: &Value) -> Result<(), MigrateError> {
    let active: DefaultModel =
        serde_json::from_value(default_llm.clone()).map_err(|e| MigrateError {
            message: format!("failed to parse default_llm: {e}"),
        })?;

    let path = secret_dir.join("providers").join("default_model.json");
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| MigrateError {
        message: format!("failed to create providers directory: {e}"),
    })?;

    let value = serde_json::to_value(&active).map_err(|e| MigrateError {
        message: format!("failed to serialize active model: {e}"),
    })?;
    rua_config::atomic_write_json(&path, &value).map_err(|e| MigrateError {
        message: format!("failed to write default_model.json: {e}"),
    })?;

    info!(
        provider_id = %active.provider_id,
        model = %active.model,
        "migrated active model"
    );
    Ok(())
}

fn restore_backup(backup: &Path, target: &Path) {
    if let Err(e) = std::fs::copy(backup, target) {
        warn!(
            error = %e,
            backup = %backup.display(),
            target = %target.display(),
            "CRITICAL: failed to restore backup"
        );
    }
}

#[derive(Debug)]
pub struct MigrateError {
    pub message: String,
}

impl std::fmt::Display for MigrateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for MigrateError {}
