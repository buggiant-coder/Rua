use rua_text_search::{regex_pattern, TextMatcher, TextSearchQuery};

#[test]
fn literal_search_is_case_insensitive_by_default() {
    let matcher = TextMatcher::new(TextSearchQuery::literal("hello.world")).unwrap();

    assert!(matcher.is_match("HELLO.WORLD"));
    assert!(!matcher.is_match("helloXworld"));
}

#[test]
fn regex_pattern_escapes_literals_for_tool_adapters() {
    assert_eq!(
        regex_pattern(&TextSearchQuery {
            pattern: "hello.world".to_string(),
            is_regex: false,
            case_sensitive: true,
        }),
        "hello\\.world"
    );
}

#[test]
fn regex_search_uses_native_regex_matcher() {
    let matcher = TextMatcher::new(TextSearchQuery::regex("hello\\s+world")).unwrap();

    assert!(matcher.is_match("hello   world"));
    assert!(!matcher.is_match("helloworld"));
}

#[test]
fn case_sensitive_search_can_be_requested() {
    let matcher = TextMatcher::new(TextSearchQuery {
        pattern: "Needle".to_string(),
        is_regex: false,
        case_sensitive: true,
    })
    .unwrap();

    assert!(matcher.is_match("Needle"));
    assert!(!matcher.is_match("needle"));
}
