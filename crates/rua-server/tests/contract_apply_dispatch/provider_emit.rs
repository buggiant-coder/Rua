fn function_body<'a>(src: &'a str, name: &str) -> &'a str {
    src.split(name)
        .nth(1)
        .and_then(|tail| tail.split("\nasync fn ").next())
        .expect("function should exist")
}

#[test]
fn provider_auto_persist_paths_publish_provider_committed_facts() {
    let src = include_str!("../../src/endpoints/models.rs");

    for (name, body) in [
        ("test_model", function_body(src, "async fn test_model")),
        (
            "probe_multimodal_inner",
            function_body(src, "async fn probe_multimodal_inner"),
        ),
    ] {
        assert!(
            body.contains("publish_provider_config_committed")
                && !body.contains("sync_ops::emit_provider_v4_upsert"),
            "{name} must publish Provider Committed Portable Fact through the Replication Publication Seam"
        );
    }
}

#[test]
fn provider_oauth_completion_publishes_provider_committed_fact() {
    let src = include_str!("../../src/endpoints/provider_oauth.rs");
    let persist_provider_key = function_body(src, "async fn persist_provider_key");

    assert!(
        persist_provider_key.contains(".provider_cells")
            && persist_provider_key.contains("publish_sync_fact")
            && persist_provider_key.contains("ProviderConfigCommitted")
            && !persist_provider_key.contains("sync_ops::emit_provider_v4_upsert"),
        "provider OAuth completion must publish Provider Committed Portable Fact after persisting the OAuth API key"
    );
}

#[test]
fn provider_write_paths_use_publication_seam() {
    let src = include_str!("../../src/endpoints/models.rs");

    for (name, body) in [
        (
            "add_custom_provider",
            function_body(src, "async fn add_custom_provider"),
        ),
        (
            "apply_provider_update",
            function_body(src, "async fn apply_provider_update"),
        ),
        (
            "delete_provider",
            function_body(src, "async fn delete_provider"),
        ),
        ("add_model", function_body(src, "async fn add_model")),
        (
            "delete_model_inner",
            function_body(src, "async fn delete_model_inner"),
        ),
        (
            "update_model_config_inner",
            function_body(src, "async fn update_model_config_inner"),
        ),
    ] {
        let publishes_provider_fact = body.contains("ProviderConfigCommitted")
            || body.contains("ProviderConfigDeleted")
            || body.contains("publish_provider_config_committed")
            || body.contains("publish_provider_config_deleted");
        assert!(
            publishes_provider_fact,
            "{name} must publish a Provider fact"
        );
        assert!(
            !body.contains("sync_ops::emit_provider_v4_upsert")
                && !body.contains("sync_ops::emit_provider_v4_delete"),
            "{name} must not use legacy Provider emit helpers"
        );
    }
}
