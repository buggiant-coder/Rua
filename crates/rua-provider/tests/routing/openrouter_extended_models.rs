use rua_provider::openrouter_extended::parse_models_payload;

fn sample_payload() -> &'static [u8] {
    br#"{
  "data": [
    {
      "id": "openai/gpt-4o",
      "name": "GPT-4o",
      "context_length": 128000,
      "architecture": {
        "input_modalities": ["text", "image"],
        "output_modalities": ["text"]
      },
      "pricing": {
        "prompt": "0.0025",
        "completion": "0.01",
        "image": "0.003613",
        "request": "0"
      }
    },
    {
      "id": "anthropic/claude-3.5-sonnet",
      "name": "Claude 3.5 Sonnet",
      "context_length": 200000,
      "architecture": {
        "input_modalities": ["text", "image"],
        "output_modalities": ["text"]
      },
      "pricing": {
        "prompt": "0.003",
        "completion": "0.015",
        "image": "0.0048",
        "request": "0"
      }
    },
    {
      "id": "google/gemini-2.5-flash",
      "name": "Gemini 2.5 Flash",
      "context_length": 1048576,
      "architecture": {
        "input_modalities": ["text", "image", "audio"],
        "output_modalities": ["text"]
      },
      "pricing": {
        "prompt": "0.00015",
        "completion": "0.0006",
        "image": "0",
        "request": "0"
      }
    },
    {
      "id": "",
      "name": "Empty ID model"
    },
    {
      "id": "openai/gpt-4o",
      "name": "GPT-4o (duplicate)",
      "context_length": 128000
    }
  ]
}"#
}

#[test]
fn parse_models_extracts_pricing_and_modalities() {
    let models = parse_models_payload(sample_payload()).unwrap();

    assert_eq!(models.len(), 3);

    let gpt4o = models.iter().find(|m| m.id == "openai/gpt-4o").unwrap();
    assert_eq!(gpt4o.name, "gpt-4o");
    assert_eq!(gpt4o.provider, "openai");
    assert_eq!(gpt4o.pricing.get("prompt").unwrap(), "0.0025");
    assert_eq!(gpt4o.pricing.get("completion").unwrap(), "0.01");
    assert!(gpt4o.input_modalities.contains(&"text".to_string()));
    assert!(gpt4o.input_modalities.contains(&"image".to_string()));
    assert_eq!(gpt4o.context_window, Some(128000));
}

#[test]
fn parse_models_handles_audio_modality() {
    let models = parse_models_payload(sample_payload()).unwrap();
    let gemini = models
        .iter()
        .find(|m| m.id == "google/gemini-2.5-flash")
        .unwrap();
    assert!(gemini.input_modalities.contains(&"audio".to_string()));
    assert_eq!(gemini.provider, "google");
}

#[test]
fn parse_models_deduplicates() {
    let models = parse_models_payload(sample_payload()).unwrap();
    let gpt4o_count = models.iter().filter(|m| m.id == "openai/gpt-4o").count();
    assert_eq!(gpt4o_count, 1);
}

#[test]
fn parse_models_skips_empty_id() {
    let models = parse_models_payload(sample_payload()).unwrap();
    assert!(models.iter().all(|m| !m.id.is_empty()));
}

#[test]
fn parse_models_no_architecture() {
    let payload = br#"{"data":[{"id":"test/model","name":"Test"}]}"#;
    let models = parse_models_payload(payload).unwrap();
    assert_eq!(models.len(), 1);
    assert!(models[0].input_modalities.is_empty());
    assert!(models[0].output_modalities.is_empty());
    assert!(models[0].pricing.is_empty());
}

#[test]
fn parse_models_numeric_pricing_values() {
    let payload = br#"{"data":[{
        "id": "meta/llama-3",
        "name": "Llama 3",
        "pricing": {"prompt": 0.001, "completion": 0.002}
    }]}"#;
    let models = parse_models_payload(payload).unwrap();
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].pricing.get("prompt").unwrap(), "0.001");
}

#[test]
fn parse_models_null_pricing_value() {
    let payload = br#"{"data":[{
        "id": "test/null-price",
        "pricing": {"prompt": null, "completion": "0.01"}
    }]}"#;
    let models = parse_models_payload(payload).unwrap();
    assert_eq!(models[0].pricing.get("prompt").unwrap(), "0");
    assert_eq!(models[0].pricing.get("completion").unwrap(), "0.01");
}
