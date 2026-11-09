use rua_agent::profile::{AgentProfile, ModelSlotRef};
use rua_agent::{DefaultModelConfig, DefaultProviderInfo, ModelSelectionSource, ModelSelector};

#[test]
fn shared_model_slot_resolution_matches_runtime_default_selection() {
    use rua_agent::runtime::model_slot_provider_model;
    use serde_json::json;
    for slot in [
        json!(null),
        json!({}),
        json!({"provider_id":"", "model":""}),
        json!({"model":"auto"}),
    ] {
        assert_eq!(model_slot_provider_model(&slot), None);
    }
    assert_eq!(
        model_slot_provider_model(
            &json!({"provider_id":" provider ","model":" vendor/model ","extra":true})
        ),
        Some(("provider".into(), "vendor/model".into()))
    );
}

#[test]
fn request_override_treats_slash_model_id_as_opaque() {
    let mut profile = AgentProfile::new("agent", "Agent");
    profile.model_slot = Some(ModelSlotRef {
        provider_id: "dashscope".to_string(),
        model: "qwen-plus".to_string(),
        extra: Default::default(),
    });

    let selected =
        ModelSelector::select(Some("kimi/kimi-k3"), &profile, None, None).expect("model selection");

    assert_eq!(selected.provider_id, "dashscope");
    assert_eq!(selected.model_id, "kimi/kimi-k3");
    assert_eq!(selected.source, ModelSelectionSource::RequestOverride);
}

#[test]
fn request_override_uses_active_provider_without_parsing_model_id() {
    let profile = AgentProfile::new("agent", "Agent");
    let active = DefaultModelConfig {
        provider_id: "openrouter".to_string(),
        model: "fallback-model".to_string(),
        extra: Default::default(),
    };

    let selected = ModelSelector::select(Some("vendor/model-name"), &profile, Some(&active), None)
        .expect("model selection");

    assert_eq!(selected.provider_id, "openrouter");
    assert_eq!(selected.model_id, "vendor/model-name");
    assert_eq!(selected.source, ModelSelectionSource::RequestOverride);
}

#[test]
fn request_override_uses_default_provider_without_parsing_model_id() {
    let profile = AgentProfile::new("agent", "Agent");
    let default_provider = DefaultProviderInfo {
        provider_id: "dashscope".to_string(),
        default_model: "qwen-plus".to_string(),
    };

    let selected = ModelSelector::select(
        Some("vendor/model-name"),
        &profile,
        None,
        Some(&default_provider),
    )
    .expect("model selection");

    assert_eq!(selected.provider_id, "dashscope");
    assert_eq!(selected.model_id, "vendor/model-name");
    assert_eq!(selected.source, ModelSelectionSource::RequestOverride);
}
