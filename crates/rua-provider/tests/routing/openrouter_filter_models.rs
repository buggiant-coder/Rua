use std::collections::HashMap;

use rua_provider::openrouter_extended::{filter_models, ExtendedModel, FilterCriteria};

fn make_model(id: &str, provider: &str, prompt_price: &str, input_mods: &[&str]) -> ExtendedModel {
    let mut pricing = HashMap::new();
    pricing.insert("prompt".to_string(), prompt_price.to_string());
    ExtendedModel {
        id: id.to_string(),
        name: id.rsplit('/').next().unwrap_or(id).to_string(),
        provider: provider.to_string(),
        input_modalities: input_mods.iter().map(|s| s.to_string()).collect(),
        output_modalities: vec!["text".to_string()],
        pricing,
        context_window: None,
    }
}

fn sample_models() -> Vec<ExtendedModel> {
    vec![
        make_model("openai/gpt-4o", "openai", "0.0025", &["text", "image"]),
        make_model(
            "anthropic/claude-3.5-sonnet",
            "anthropic",
            "0.003",
            &["text", "image"],
        ),
        make_model(
            "google/gemini-2.5-flash",
            "google",
            "0.00015",
            &["text", "image", "audio"],
        ),
        make_model("meta/llama-3", "meta", "0.0001", &["text"]),
    ]
}

#[test]
fn filter_by_max_prompt_price() {
    let models = sample_models();
    let criteria = FilterCriteria {
        max_prompt_price: Some(0.001),
        ..Default::default()
    };
    let filtered = filter_models(&models, &criteria);
    assert_eq!(filtered.len(), 2);
    assert!(filtered.iter().all(|m| {
        let p: f64 = m.pricing.get("prompt").unwrap().parse().unwrap();
        p <= 0.001
    }));
}

#[test]
fn filter_by_provider() {
    let models = sample_models();
    let criteria = FilterCriteria {
        providers: Some(vec!["openai".to_string(), "google".to_string()]),
        ..Default::default()
    };
    let filtered = filter_models(&models, &criteria);
    assert_eq!(filtered.len(), 2);
    assert!(filtered.iter().any(|m| m.provider == "openai"));
    assert!(filtered.iter().any(|m| m.provider == "google"));
}

#[test]
fn filter_by_input_modality() {
    let models = sample_models();
    let criteria = FilterCriteria {
        input_modalities: Some(vec!["audio".to_string()]),
        ..Default::default()
    };
    let filtered = filter_models(&models, &criteria);
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].id, "google/gemini-2.5-flash");
}

#[test]
fn filter_combined_criteria() {
    let models = sample_models();
    let criteria = FilterCriteria {
        input_modalities: Some(vec!["image".to_string()]),
        max_prompt_price: Some(0.001),
        ..Default::default()
    };
    let filtered = filter_models(&models, &criteria);
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].id, "google/gemini-2.5-flash");
}

#[test]
fn filter_no_criteria_returns_all() {
    let models = sample_models();
    let criteria = FilterCriteria::default();
    let filtered = filter_models(&models, &criteria);
    assert_eq!(filtered.len(), models.len());
}

#[test]
fn filter_provider_case_insensitive() {
    let models = sample_models();
    let criteria = FilterCriteria {
        providers: Some(vec!["OpenAI".to_string()]),
        ..Default::default()
    };
    let filtered = filter_models(&models, &criteria);
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].provider, "openai");
}
