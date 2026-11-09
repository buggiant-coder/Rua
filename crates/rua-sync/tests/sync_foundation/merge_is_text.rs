use rua_sync::merge::is_text;

#[test]
fn utf8_no_nul_is_text() {
    assert!(is_text(b"hello world"));
    assert!(is_text("你好世界".as_bytes()));
    assert!(is_text(b"line1\nline2\n"));
}

#[test]
fn empty_is_text() {
    assert!(is_text(b""));
}

#[test]
fn nul_byte_is_binary() {
    assert!(!is_text(b"hello\x00world"));
    assert!(!is_text(b"\x00"));
}

#[test]
fn invalid_utf8_is_binary() {
    assert!(!is_text(&[0xFF, 0xFE, 0x41]));
    assert!(!is_text(&[0x80, 0x81]));
}

#[test]
fn nul_in_valid_utf8_is_binary() {
    assert!(!is_text(b"abc\x00def"));
}
