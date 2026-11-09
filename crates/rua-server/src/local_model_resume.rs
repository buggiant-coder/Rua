use std::sync::Arc;

use rua_local_model::{LocalModelManager, StartServerRequest};
use rua_provider::manager::ProviderManager;
use rua_provider::{ProviderCells, ProviderConfig};
use tracing::{info, warn};

pub const LOCAL_PROVIDER_ID: &str = "rua-local";

pub async fn resume_on_startup(
    local_manager: Arc<LocalModelManager>,
    _provider_manager: Arc<ProviderManager>,
    cells: Arc<ProviderCells>,
) {
    let config = match cells.cell(LOCAL_PROVIDER_ID).read().await {
        Ok(Some(c)) => c,
        Ok(None) => {
            info!("no rua-local provider configured; skip local model resume");
            return;
        }
        Err(e) => {
            info!(error = %e, "failed to read rua-local provider config; skip local model resume");
            return;
        }
    };

    let model_id = match config.extra_models.first().map(|m| m.id.clone()) {
        Some(id) => id,
        None => {
            info!("rua-local provider has no extra_models; skip local model resume");
            return;
        }
    };

    let req = StartServerRequest {
        model_id: Some(model_id.clone()),
        port: None,
    };
    match local_manager.start_server(req).await {
        Ok(status) => {
            let port = match status.port {
                Some(p) => p,
                None => {
                    warn!("local server started but reported no port; skip provider update");
                    return;
                }
            };
            let cell = cells.cell(LOCAL_PROVIDER_ID);
            let result = cell
                .update(move |maybe_existing| {
                    let mut cfg = maybe_existing.unwrap_or(config);
                    apply_base_url(&mut cfg, port);
                    cfg
                })
                .await;
            match result {
                Ok(_) => info!(
                    model_id,
                    port, "resumed local model and updated provider base_url"
                ),
                Err(e) => warn!(error = %e, "failed to update rua-local base_url after resume"),
            }
        }
        Err(e) => {
            warn!(error = %e, "failed to resume local model server");
        }
    }
}

pub async fn sync_local_provider_url(
    _provider_manager: Arc<ProviderManager>,
    cells: Arc<ProviderCells>,
    port: u16,
) {
    let cell = cells.cell(LOCAL_PROVIDER_ID);
    if let Ok(None) = cell.read().await {
        info!("no rua-local provider configured; skip base_url sync");
        return;
    }
    let result = cell
        .update(move |maybe_existing| {
            let mut cfg = match maybe_existing {
                Some(c) => c,
                None => {
                    return ProviderConfig {
                        id: LOCAL_PROVIDER_ID.to_string(),
                        name: String::new(),
                        base_url: String::new(),
                        api_key: String::new(),
                        api_keys: Vec::new(),
                        api_key_prefixes: Vec::new(),
                        headers: serde_json::Map::new(),
                        custom_headers: serde_json::Map::new(),
                        chat_model: None,
                        models: Vec::new(),
                        extra_models: Vec::new(),
                        timeout: None,
                        generate_kwargs: serde_json::Map::new(),
                        support_model_discovery: false,
                        support_connection_check: false,
                        is_local: true,
                        api_key_prefix: String::new(),
                        is_custom: false,
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
                    };
                }
            };
            apply_base_url(&mut cfg, port);
            cfg
        })
        .await;
    match result {
        Ok(_) => info!(port, "synced rua-local base_url after server start"),
        Err(e) => warn!(error = %e, "failed to sync rua-local base_url"),
    }
}

fn apply_base_url(config: &mut ProviderConfig, port: u16) {
    config.base_url = format!("http://127.0.0.1:{}/v1", port);
}
