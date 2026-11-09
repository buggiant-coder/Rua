use rua_core::token_estimate::{estimate_serialized_tokens, DEFAULT_RESIZED_IMAGE_TOKEN_ESTIMATE};

#[test]
fn plain_text_estimate_honors_configured_divisor() {
    let input = serde_json::json!({ "text": "A".repeat(4_000) });

    let tokens = estimate_serialized_tokens(&input, 4.0);

    assert!(
        (1_000..=1_010).contains(&tokens),
        "4 bytes per token should remain the default text estimate"
    );
}

#[test]
fn base64_image_payload_size_does_not_dominate_estimate() {
    let short = serde_json::json!({
        "type": "image",
        "url": "data:image/png;base64,AAAA",
    });
    let large = serde_json::json!({
        "type": "image",
        "url": format!("data:image/png;base64,{}", "A".repeat(1_942_716)),
    });

    let short_tokens = estimate_serialized_tokens(&short, 4.0);
    let large_tokens = estimate_serialized_tokens(&large, 4.0);

    assert!(short_tokens >= DEFAULT_RESIZED_IMAGE_TOKEN_ESTIMATE);
    assert!(large_tokens >= DEFAULT_RESIZED_IMAGE_TOKEN_ESTIMATE);
    assert!(
        short_tokens.abs_diff(large_tokens) <= 2,
        "base64 payload size should not materially change image token cost"
    );
}

#[test]
fn legacy_tool_result_image_json_uses_image_token_cost() {
    let image_output = serde_json::json!({
        "type": "tool_result",
        "output": serde_json::json!([{
            "type": "image",
            "media_type": "image/png",
            "url": format!("data:image/png;base64,{}", "A".repeat(1_942_716)),
        }])
        .to_string(),
    });

    let tokens = estimate_serialized_tokens(&image_output, 4.0);

    assert!(
        (DEFAULT_RESIZED_IMAGE_TOKEN_ESTIMATE..=DEFAULT_RESIZED_IMAGE_TOKEN_ESTIMATE + 100)
            .contains(&tokens),
        "tool-result image estimate should stay near the provider-observed image cost"
    );
}
