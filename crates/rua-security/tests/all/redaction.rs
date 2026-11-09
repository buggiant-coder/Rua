use rua_security::{redact_path, redact_secrets, truncate_value};

#[test]
fn redact_secrets_masks_common_token_shapes() {
    let input = concat!(
        "openai=sk-abcdefghijklmnopqrstuvwxyz ",
        "api_key=\"abcd1234abcd1234\" ",
        "Authorization: Bearer abc.def_ghi-jklmnopqrstuvwxyz ",
        "aws=AKIAIOSFODNN7EXAMPLE ",
        "url=https://user:password@example.com/path ",
        "password=supersecret"
    );

    let redacted = redact_secrets(input);

    assert!(redacted.contains("sk-***"));
    assert!(redacted.contains("api_key=***"));
    assert!(redacted.contains("Bearer ***"));
    assert!(redacted.contains("AKIA***"));
    assert!(redacted.contains("https://user:***@example.com/path"));
    assert!(redacted.contains("password=***"));
    assert!(!redacted.contains("abcdefghijklmnopqrstuvwxyz"));
    assert!(!redacted.contains("abcd1234abcd1234"));
    assert!(!redacted.contains("AKIAIOSFODNN7EXAMPLE"));
    assert!(!redacted.contains("supersecret"));
}

#[test]
fn redact_secrets_leaves_non_secret_text_unchanged() {
    let input = "normal command output without credentials";

    assert_eq!(redact_secrets(input), input);
}

#[test]
fn redact_path_keeps_only_final_component() {
    assert_eq!(
        redact_path("/home/user/.ssh/id_rsa"),
        ".../<redacted>/id_rsa"
    );
    assert_eq!(redact_path("/var/log/"), ".../<redacted>/");
    assert_eq!(redact_path("relative.txt"), "relative.txt");
}

#[test]
fn truncate_value_uses_character_boundaries() {
    assert_eq!(truncate_value("abcdef", 3), "abc...");
    assert_eq!(truncate_value("你好世界", 2), "你好...");
    assert_eq!(truncate_value("short", 10), "short");
    assert_eq!(truncate_value("anything", 0), "...");
}
