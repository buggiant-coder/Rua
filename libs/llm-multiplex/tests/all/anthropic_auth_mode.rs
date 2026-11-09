use llm_multiplex::providers::anthropic::{AnthropicConfig, AuthMode};

#[test]
fn api_key_mode_sets_x_api_key_header() {
    let config = AnthropicConfig {
        api_key: Some("sk-test-key".into()),
        auth_mode: AuthMode::ApiKey,
        ..Default::default()
    };
    let tc = config.build_transport_config();
    assert_eq!(
        tc.headers.get("x-api-key").map(|v| v.to_str().unwrap()),
        Some("sk-test-key")
    );
    assert!(tc.headers.get("authorization").is_none());
}

#[test]
fn auth_token_mode_sets_bearer_header() {
    let config = AnthropicConfig {
        api_key: Some("sk-test-key".into()),
        auth_mode: AuthMode::AuthToken,
        ..Default::default()
    };
    let tc = config.build_transport_config();
    assert_eq!(
        tc.headers.get("authorization").map(|v| v.to_str().unwrap()),
        Some("Bearer sk-test-key")
    );
    assert!(tc.headers.get("x-api-key").is_none());
}

#[test]
fn default_auth_mode_is_api_key() {
    let config = AnthropicConfig {
        api_key: Some("my-key".into()),
        ..Default::default()
    };
    let tc = config.build_transport_config();
    assert_eq!(
        tc.headers.get("x-api-key").map(|v| v.to_str().unwrap()),
        Some("my-key")
    );
    assert!(tc.headers.get("authorization").is_none());
}

#[test]
fn no_api_key_sets_no_auth_headers() {
    let config = AnthropicConfig {
        api_key: None,
        auth_mode: AuthMode::AuthToken,
        ..Default::default()
    };
    let tc = config.build_transport_config();
    assert!(tc.headers.get("x-api-key").is_none());
    assert!(tc.headers.get("authorization").is_none());
}
