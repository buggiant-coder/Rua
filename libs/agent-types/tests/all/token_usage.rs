use agent_types::TokenUsage;

#[test]
fn total_prefers_provider_reported_total_when_present() {
    let usage = TokenUsage {
        prompt_tokens: 100,
        completion_tokens: 50,
        total_tokens: Some(175),
        ..Default::default()
    };

    assert_eq!(usage.total(), 175);
}

#[test]
fn total_falls_back_to_prompt_plus_completion() {
    let usage = TokenUsage::new(100, 50);

    assert_eq!(usage.total(), 150);
}

#[test]
fn accumulate_sums_provider_specific_optional_counts_and_recomputes_total() {
    let mut usage = TokenUsage {
        prompt_tokens: 100,
        completion_tokens: 50,
        reasoning_tokens: Some(10),
        cached_tokens: Some(20),
        cache_write_tokens: Some(30),
        total_tokens: Some(999),
    };
    let other = TokenUsage {
        prompt_tokens: 200,
        completion_tokens: 60,
        reasoning_tokens: Some(5),
        cached_tokens: Some(7),
        cache_write_tokens: Some(11),
        total_tokens: Some(888),
    };

    usage.accumulate(&other);

    assert_eq!(usage.prompt_tokens, 300);
    assert_eq!(usage.completion_tokens, 110);
    assert_eq!(usage.reasoning_tokens, Some(15));
    assert_eq!(usage.cached_tokens, Some(27));
    assert_eq!(usage.cache_write_tokens, Some(41));
    assert_eq!(usage.total_tokens, None);
    assert_eq!(usage.total(), 410);
}

#[test]
fn accumulate_initializes_missing_optional_counts() {
    let mut usage = TokenUsage::new(100, 50);
    let other = TokenUsage {
        reasoning_tokens: Some(3),
        cached_tokens: Some(5),
        cache_write_tokens: Some(7),
        ..Default::default()
    };

    usage.accumulate(&other);

    assert_eq!(usage.reasoning_tokens, Some(3));
    assert_eq!(usage.cached_tokens, Some(5));
    assert_eq!(usage.cache_write_tokens, Some(7));
}

#[test]
fn add_uses_accumulate_semantics() {
    let left = TokenUsage::new(10, 5);
    let right = TokenUsage {
        prompt_tokens: 20,
        completion_tokens: 7,
        reasoning_tokens: Some(2),
        ..Default::default()
    };

    let total = left + right;

    assert_eq!(total.prompt_tokens, 30);
    assert_eq!(total.completion_tokens, 12);
    assert_eq!(total.reasoning_tokens, Some(2));
}

#[test]
fn is_empty_treats_any_reported_count_as_non_empty() {
    assert!(TokenUsage::default().is_empty());
    assert!(!TokenUsage::new(1, 0).is_empty());
    assert!(!TokenUsage::new(0, 1).is_empty());
    assert!(!TokenUsage {
        reasoning_tokens: Some(1),
        ..Default::default()
    }
    .is_empty());
    assert!(!TokenUsage {
        cached_tokens: Some(1),
        ..Default::default()
    }
    .is_empty());
    assert!(!TokenUsage {
        cache_write_tokens: Some(1),
        ..Default::default()
    }
    .is_empty());
}

#[test]
fn serde_preserves_optional_provider_specific_counts() {
    let usage = TokenUsage {
        prompt_tokens: 10,
        completion_tokens: 5,
        reasoning_tokens: Some(2),
        cached_tokens: Some(3),
        cache_write_tokens: Some(7),
        total_tokens: Some(20),
    };

    let json = serde_json::to_string(&usage).unwrap();
    let parsed: TokenUsage = serde_json::from_str(&json).unwrap();

    assert_eq!(parsed, usage);
}

#[test]
fn deserialize_missing_optional_counts_defaults_to_none() {
    let json = r#"{"prompt_tokens":10,"completion_tokens":5}"#;

    let usage: TokenUsage = serde_json::from_str(json).unwrap();

    assert_eq!(usage.reasoning_tokens, None);
    assert_eq!(usage.cached_tokens, None);
    assert_eq!(usage.cache_write_tokens, None);
    assert_eq!(usage.total_tokens, None);
}
