#[path = "all/text_matcher.rs"]
mod text_matcher;

#[test]
fn shared_text_matcher_handles_literal_case_insensitive_search() {
    let matcher = rua_text_search::TextMatcher::new(rua_text_search::TextSearchQuery::literal(
        "Project Alpha",
    ))
    .unwrap();

    assert!(matcher.is_match("project alpha launch notes"));
    assert!(!matcher.is_match("project beta launch notes"));
}

#[test]
fn shared_text_matcher_handles_regex_search() {
    let matcher =
        rua_text_search::TextMatcher::new(rua_text_search::TextSearchQuery::regex("turn-[0-9]+"))
            .unwrap();

    assert!(matcher.is_match("see turn-42 for details"));
    assert!(!matcher.is_match("see turn-x for details"));
}
