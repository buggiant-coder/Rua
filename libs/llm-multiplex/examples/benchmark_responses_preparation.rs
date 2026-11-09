//! Offline replay of Responses request preparation. Never polls the stream or sends HTTP.

use std::hint::black_box;
use std::path::PathBuf;
use std::time::Instant;

use agent_types::ChatRequest;
use llm_multiplex::{OpenAiCompatibleConfig, OpenAiResponsesProvider, Provider};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let call_dir = PathBuf::from(
        std::env::args()
            .nth(1)
            .ok_or("expected capture call directory")?,
    );
    let request: ChatRequest =
        serde_json::from_slice(&std::fs::read(call_dir.join("logical-request.json"))?)?;
    let provider = OpenAiResponsesProvider::new(OpenAiCompatibleConfig {
        provider_id: "dashscope".into(),
        provider_name: "DashScope offline benchmark".into(),
        base_url: "http://127.0.0.1:1".into(),
        api_key: None,
        api_keys: Vec::new(),
        headers: Default::default(),
        models: Vec::new(),
        timeout_secs: 1,
        generate_kwargs: Default::default(),
        support_model_discovery: false,
        support_connection_check: false,
        is_local: true,
    })?;
    let model =
        provider.create_chat_model(request.model.as_deref().ok_or("capture has no model")?)?;
    let mut samples = Vec::new();
    let mut wire_hash = String::new();
    let mut input_items = 0;
    for _ in 0..5 {
        let started = Instant::now();
        let body = black_box(provider.build_request_body(black_box(&request)));
        let body_ms = started.elapsed().as_secs_f64() * 1000.0;
        wire_hash = format!("{:x}", Sha256::digest(serde_json::to_vec(&body)?));
        input_items = body["input"].as_array().map_or(0, Vec::len);
        let owned_request = request.clone();
        let started = Instant::now();
        // Construction includes all synchronous provider work before the first poll.
        let stream = black_box(model.chat_stream(black_box(owned_request)));
        let stream_ms = started.elapsed().as_secs_f64() * 1000.0;
        drop(stream);
        samples.push(json!({"body_ms": body_ms, "stream_construction_ms": stream_ms}));
    }
    let expected_path = std::fs::read_dir(&call_dir)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("provider-request-")
        });
    let wire_matches_capture = expected_path
        .map(|path| -> Result<bool, Box<dyn std::error::Error>> {
            let artifact: Value = serde_json::from_slice(&std::fs::read(path)?)?;
            Ok(format!(
                "{:x}",
                Sha256::digest(serde_json::to_vec(&artifact["body"])?)
            ) == wire_hash)
        })
        .transpose()?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "offline": true,
            "model": request.model,
            "messages": request.messages.len(),
            "input_items": input_items,
            "wire_sha256": wire_hash,
            "wire_matches_capture": wire_matches_capture,
            "samples": samples,
        }))?
    );
    Ok(())
}
