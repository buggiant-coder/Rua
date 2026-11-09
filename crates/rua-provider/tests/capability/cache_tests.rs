use rua_provider::capability_cache;

#[test]
fn learn_and_get_needs_reasoning_content() {
    let key = capability_cache::cache_key("test-provider-learn", "test-model-learn");
    assert!(!capability_cache::needs_reasoning_content(&key));
    assert!(capability_cache::get_capabilities(&key).is_none());

    capability_cache::learn_needs_reasoning_content(&key);

    assert!(capability_cache::needs_reasoning_content(&key));
    let caps = capability_cache::get_capabilities(&key).unwrap();
    assert!(caps.needs_reasoning_content);
    assert!(!caps.rejects_media);
}

#[test]
fn learn_and_get_rejects_media() {
    let key = capability_cache::cache_key("test-provider-media", "test-model-media");
    assert!(!capability_cache::rejects_media(&key));
    assert!(capability_cache::get_capabilities(&key).is_none());

    capability_cache::learn_rejects_media(&key);

    assert!(capability_cache::rejects_media(&key));
    let caps = capability_cache::get_capabilities(&key).unwrap();
    assert!(caps.rejects_media);
    assert!(!caps.needs_reasoning_content);
}

#[test]
fn rejects_media_and_reasoning_independent_flags() {
    let key = capability_cache::cache_key("test-two-flags", "model-y");
    capability_cache::learn_rejects_media(&key);
    capability_cache::learn_needs_reasoning_content(&key);

    let caps = capability_cache::get_capabilities(&key).unwrap();
    assert!(caps.rejects_media);
    assert!(caps.needs_reasoning_content);
}

#[test]
fn learn_rejects_media_idempotent() {
    let key = capability_cache::cache_key("test-media-idem", "model-z");
    capability_cache::learn_rejects_media(&key);
    capability_cache::learn_rejects_media(&key);

    assert!(capability_cache::rejects_media(&key));
}

#[test]
fn cache_key_format() {
    let key = capability_cache::cache_key("dashscope", "qwen-plus");
    assert_eq!(key, "dashscope:qwen-plus");
}

#[test]
fn different_models_independent() {
    let key_a = capability_cache::cache_key("test-indep", "model-a");
    let key_b = capability_cache::cache_key("test-indep", "model-b");

    capability_cache::learn_needs_reasoning_content(&key_a);

    assert!(capability_cache::needs_reasoning_content(&key_a));
    assert!(!capability_cache::needs_reasoning_content(&key_b));
}

#[test]
fn learn_idempotent() {
    let key = capability_cache::cache_key("test-idempotent", "model-x");
    capability_cache::learn_needs_reasoning_content(&key);
    capability_cache::learn_needs_reasoning_content(&key);

    assert!(capability_cache::needs_reasoning_content(&key));
}
