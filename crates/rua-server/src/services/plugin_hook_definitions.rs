use serde::Deserialize;

use crate::services::product_hooks::{
    CreatePluginHookDefinition, HookHandlerView, ProductHookService,
};

#[derive(Debug, Deserialize)]
struct PluginHookDefinitionDeclaration {
    logical_id: String,
    display_name: String,
    event: String,
    #[serde(default)]
    capabilities: Vec<String>,
    handler: HookHandlerView,
}

pub async fn register_from_record(
    hooks: &ProductHookService,
    record: &rua_plugin::PluginRecord,
) -> Result<(), String> {
    let declarations = record
        .extra
        .get("meta")
        .and_then(|meta| meta.get("hook_definitions"))
        .cloned()
        .unwrap_or_else(|| serde_json::Value::Array(Vec::new()));
    let declarations = serde_json::from_value::<Vec<PluginHookDefinitionDeclaration>>(declarations)
        .map_err(|error| format!("invalid plugin hook_definitions: {error}"))?;
    let definitions = declarations
        .into_iter()
        .map(|declaration| {
            let available = declaration.handler.available && record.enabled && record.installed;
            CreatePluginHookDefinition {
                logical_id: declaration.logical_id,
                display_name: declaration.display_name,
                event: declaration.event,
                capabilities: declaration.capabilities,
                handler: HookHandlerView {
                    available: true,
                    ..declaration.handler
                },
                available,
            }
        })
        .collect();
    hooks
        .register_plugin_definitions(&record.id, definitions)
        .await?;
    Ok(())
}
