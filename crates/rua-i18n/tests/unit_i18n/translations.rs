use rua_i18n::{normalize_lang, t};

#[test]
fn translations_return_supported_language_text() {
    let cases = [
        ("zh", "历史已清除"),
        ("en", "History Cleared"),
        ("ja", "履歴"),
        ("ru", "История"),
    ];

    for (lang, expected) in cases {
        let text = t("command.clear.body", lang);
        assert!(
            text.contains(expected),
            "{lang} translation should contain {expected:?}, got {text:?}"
        );
    }
}

#[test]
fn translation_unknown_language_falls_back_to_zh_and_unknown_key_returns_key() {
    let text = t("command.clear.body", "xx");
    assert!(
        text.contains("历史已清除"),
        "unknown lang should fall back to zh"
    );

    assert_eq!(t("nonexistent.key", "en"), "nonexistent.key");
}

#[test]
fn normalize_lang_maps_locale_variants_to_rust_supported_languages() {
    for (raw, normalized) in [
        ("zh-CN", "zh"),
        ("zh_CN", "zh"),
        ("en-US", "en"),
        ("en", "en"),
        ("ja", "ja"),
        ("ru-RU", "ru"),
        ("unknown", "zh"),
        ("", "zh"),
    ] {
        assert_eq!(normalize_lang(raw), normalized, "{raw:?}");
    }
}
