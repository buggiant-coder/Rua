use std::collections::HashMap;

use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtendedModel {
    pub id: String,
    pub name: String,
    pub provider: String,
    pub input_modalities: Vec<String>,
    pub output_modalities: Vec<String>,
    pub pricing: HashMap<String, String>,
    pub context_window: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpstreamProvider {
    pub name: String,
}

#[derive(Debug, Clone, Default)]
pub struct FilterCriteria {
    pub providers: Option<Vec<String>>,
    pub input_modalities: Option<Vec<String>>,
    pub output_modalities: Option<Vec<String>>,
    pub max_prompt_price: Option<f64>,
}

#[derive(Deserialize)]
struct ModelsResponse {
    data: Vec<RawModel>,
}

#[derive(Deserialize)]
struct RawModel {
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    context_length: Option<u64>,
    #[serde(default)]
    architecture: Option<RawArchitecture>,
    #[serde(default)]
    pricing: Option<HashMap<String, Option<serde_json::Value>>>,
}

#[derive(Deserialize)]
struct RawArchitecture {
    #[serde(default)]
    input_modalities: Option<Vec<String>>,
    #[serde(default)]
    output_modalities: Option<Vec<String>>,
}

fn extract_provider(model_id: &str) -> &str {
    model_id
        .split('/')
        .next()
        .filter(|_| model_id.contains('/'))
        .unwrap_or("")
}

fn extract_model_name(model_id: &str) -> &str {
    if model_id.contains('/') {
        model_id.rsplit('/').next().unwrap_or(model_id)
    } else {
        model_id
    }
}

fn normalize_pricing(raw: &HashMap<String, Option<serde_json::Value>>) -> HashMap<String, String> {
    raw.iter()
        .map(|(k, v)| {
            let s = match v {
                Some(serde_json::Value::String(s)) => s.clone(),
                Some(serde_json::Value::Number(n)) => n.to_string(),
                Some(v) => v.to_string(),
                None => "0".to_string(),
            };
            (k.clone(), s)
        })
        .collect()
}

fn raw_to_extended(raw: RawModel) -> Option<ExtendedModel> {
    let id = raw.id.trim().to_string();
    if id.is_empty() {
        return None;
    }

    let provider = extract_provider(&id).to_string();
    let mut name = extract_model_name(&id).to_string();

    if !id.contains('/') {
        if let Some(attr_name) = raw.name.as_deref() {
            let trimmed = attr_name.trim();
            if !trimmed.is_empty() {
                name = trimmed.to_string();
            }
        }
    }

    let (input_modalities, output_modalities) = match &raw.architecture {
        Some(arch) => (
            arch.input_modalities.clone().unwrap_or_default(),
            arch.output_modalities.clone().unwrap_or_default(),
        ),
        None => (vec![], vec![]),
    };

    let pricing = raw
        .pricing
        .as_ref()
        .map(normalize_pricing)
        .unwrap_or_default();

    Some(ExtendedModel {
        id,
        name,
        provider,
        input_modalities,
        output_modalities,
        pricing,
        context_window: raw.context_length,
    })
}

pub async fn fetch_extended_models(
    client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
) -> Result<Vec<ExtendedModel>, reqwest::Error> {
    let url = format!("{}/models", base_url.trim_end_matches('/'));

    let mut headers = HeaderMap::new();
    if !api_key.is_empty() {
        if let Ok(v) = HeaderValue::from_str(&format!("Bearer {api_key}")) {
            headers.insert(AUTHORIZATION, v);
        }
    }

    let resp = client
        .get(&url)
        .headers(headers)
        .send()
        .await?
        .error_for_status()?;

    let body: ModelsResponse = resp.json().await?;

    let mut seen = HashMap::new();
    for raw in body.data {
        let trimmed_id = raw.id.trim().to_string();
        if !trimmed_id.is_empty() && !seen.contains_key(&trimmed_id) {
            if let Some(model) = raw_to_extended(raw) {
                seen.insert(model.id.clone(), model);
            }
        }
    }

    Ok(seen.into_values().collect())
}

pub fn filter_models(models: &[ExtendedModel], criteria: &FilterCriteria) -> Vec<ExtendedModel> {
    models
        .iter()
        .filter(|m| {
            if let Some(providers) = &criteria.providers {
                let lower = m.provider.to_lowercase();
                if !providers.iter().any(|p| p.to_lowercase() == lower) {
                    return false;
                }
            }
            if let Some(input_mods) = &criteria.input_modalities {
                if !input_mods.iter().any(|im| m.input_modalities.contains(im)) {
                    return false;
                }
            }
            if let Some(output_mods) = &criteria.output_modalities {
                if !output_mods
                    .iter()
                    .any(|om| m.output_modalities.contains(om))
                {
                    return false;
                }
            }
            if let Some(max_price) = criteria.max_prompt_price {
                if let Some(prompt_str) = m.pricing.get("prompt") {
                    if let Ok(price) = prompt_str.parse::<f64>() {
                        if price > max_price {
                            return false;
                        }
                    }
                }
            }
            true
        })
        .cloned()
        .collect()
}

pub async fn get_available_providers(
    client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
) -> Result<Vec<UpstreamProvider>, reqwest::Error> {
    let models = fetch_extended_models(client, base_url, api_key).await?;
    let mut providers: Vec<String> = models
        .iter()
        .map(|m| m.provider.clone())
        .filter(|p| !p.is_empty())
        .collect();
    providers.sort();
    providers.dedup();
    Ok(providers
        .into_iter()
        .map(|name| UpstreamProvider { name })
        .collect())
}

pub fn parse_models_payload(json: &[u8]) -> Result<Vec<ExtendedModel>, serde_json::Error> {
    let resp: ModelsResponse = serde_json::from_slice(json)?;
    let mut seen = HashMap::new();
    for raw in resp.data {
        let trimmed_id = raw.id.trim().to_string();
        if !trimmed_id.is_empty() && !seen.contains_key(&trimmed_id) {
            if let Some(model) = raw_to_extended(raw) {
                seen.insert(model.id.clone(), model);
            }
        }
    }
    Ok(seen.into_values().collect())
}
