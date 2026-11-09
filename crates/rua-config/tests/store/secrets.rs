use rua_config::secrets::{is_masked_value, mask_env_value, redact_api_key};

#[test]
fn redact_api_key_empty() {
    assert_eq!(redact_api_key(""), "");
}

#[test]
fn redact_api_key_short_uses_sentinel() {
    assert_eq!(redact_api_key("abcdefgh"), "***");
    assert_eq!(redact_api_key("a"), "***");
}

#[test]
fn redact_api_key_long_keeps_first_and_last_four() {
    assert_eq!(
        redact_api_key("sk-1234567890abcdef"),
        "sk-1...cdef".to_string()
    );
    assert_eq!(redact_api_key("abcd1234efgh"), "abcd...efgh");
}

#[test]
fn redact_api_key_handles_unicode_chars() {
    assert_eq!(redact_api_key("αβγδε12345"), "αβγδ...2345");
}

#[test]
fn mask_env_value_empty_passthrough() {
    assert_eq!(mask_env_value(""), "");
}

#[test]
fn mask_env_value_short_fully_masked() {
    assert_eq!(mask_env_value("abc"), "***");
    assert_eq!(mask_env_value("abcdefgh"), "********");
}

#[test]
fn mask_env_value_long_no_dash_keeps_two_prefix() {
    assert_eq!(mask_env_value("abc123456789xyz"), "ab*********9xyz");
}

#[test]
fn mask_env_value_long_with_dash_at_index_two_keeps_three_prefix() {
    assert_eq!(mask_env_value("my-api-key-value"), "my-*********alue");
}

#[test]
fn mask_env_value_enforces_minimum_four_stars() {
    let masked = mask_env_value("abc456789");
    assert_eq!(masked, "ab****6789");
    assert!(masked.contains("****"));
}

#[test]
fn mask_env_value_python_parity_sk_proj_example() {
    let masked = mask_env_value("sk-proj-1234567890abcdefghij1234");
    assert_eq!(masked, "sk-*************************1234");
}

#[test]
fn is_masked_value_detects_four_stars_substring() {
    assert!(is_masked_value("ab****cdef"));
    assert!(is_masked_value(&mask_env_value("abc123456789xyz")));
}

#[test]
fn is_masked_value_rejects_unmasked_strings() {
    assert!(!is_masked_value("plain"));
    assert!(!is_masked_value(""));
    assert!(!is_masked_value("***"));
    assert!(!is_masked_value("a*b*c*d*"));
}

#[test]
fn is_masked_value_round_trip_with_mask_env_value() {
    for input in ["abcdefghi", "abc123456789xyz", "my-api-key-value"] {
        let masked = mask_env_value(input);
        assert!(
            is_masked_value(&masked),
            "expected masked round-trip for {input:?} -> {masked:?}"
        );
    }
}
