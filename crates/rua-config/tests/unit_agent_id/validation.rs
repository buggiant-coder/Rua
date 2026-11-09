use std::collections::HashSet;

use rua_config::{generate_short_agent_id, sanitize_agent_id, validate_agent_id};

#[test]
fn generated_short_agent_ids_are_twelve_character_alphanumeric_hex() {
    let agent_id = generate_short_agent_id();

    assert_eq!(agent_id.len(), 12);
    assert!(agent_id.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn generated_short_agent_ids_have_low_collision_rate_for_small_batches() {
    let ids: HashSet<String> = (0..100).map(|_| generate_short_agent_id()).collect();

    assert!(
        ids.len() >= 95,
        "expected at least 95 unique ids from 100 generated ids, got {}",
        ids.len()
    );
}

#[test]
fn sanitize_agent_id_strips_outer_whitespace_without_rewriting_case_or_symbols() {
    assert_eq!(sanitize_agent_id("  Browse-Agent  "), "Browse-Agent");
    assert_eq!(sanitize_agent_id("MY_BOT"), "MY_BOT");
    assert_eq!(sanitize_agent_id("   "), "");
}

#[test]
fn validate_agent_id_accepts_qwenpaw_compatible_ascii_patterns() {
    let existing = HashSet::new();
    for id in [
        "browse-agent",
        "my_bot_v2",
        "a1",
        "agent123",
        "Browse-Agent",
        "MY_BOT",
    ] {
        validate_agent_id(id, &existing).unwrap_or_else(|err| panic!("{id}: {err}"));
    }
}

#[test]
fn validate_agent_id_rejects_length_boundaries() {
    let existing = HashSet::new();

    let too_short = validate_agent_id("a", &existing).unwrap_err();
    assert!(too_short.contains("at least 2 characters"));

    let too_long_id = "a".repeat(65);
    let too_long = validate_agent_id(&too_long_id, &existing).unwrap_err();
    assert!(too_long.contains("at most 64 characters"));
}

#[test]
fn validate_agent_id_rejects_invalid_characters_and_edge_separators() {
    let existing = HashSet::new();

    for id in [
        "Browse Agent",
        "中文名",
        "special!chars",
        "-starts-dash",
        "_starts-under",
        "ends-dash-",
        "ends_under_",
    ] {
        let err = validate_agent_id(id, &existing).unwrap_err();
        assert!(
            err.contains("invalid characters"),
            "unexpected error for {id}: {err}"
        );
    }
}

#[test]
fn validate_agent_id_rejects_reserved_and_duplicate_ids() {
    let existing = HashSet::from(["browse-agent".to_string(), "my-bot".to_string()]);

    let reserved = validate_agent_id("default", &existing).unwrap_err();
    assert!(reserved.contains("reserved"));

    let duplicate = validate_agent_id("browse-agent", &existing).unwrap_err();
    assert!(duplicate.contains("already exists"));
}
