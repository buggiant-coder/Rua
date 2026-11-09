use rua_command::daemon::handlers::parse_log_lines;

#[test]
fn parse_log_lines_defaults_when_args_are_empty() {
    assert_eq!(parse_log_lines(""), 100);
}

#[test]
fn parse_log_lines_accepts_explicit_count() {
    assert_eq!(parse_log_lines("50"), 50);
}

#[test]
fn parse_log_lines_clamps_large_counts() {
    assert_eq!(parse_log_lines("3000"), 2000);
}

#[test]
fn parse_log_lines_clamps_zero_to_one() {
    assert_eq!(parse_log_lines("0"), 1);
}

#[test]
fn parse_log_lines_defaults_when_no_token_is_numeric() {
    assert_eq!(parse_log_lines("abc"), 100);
}

#[test]
fn parse_log_lines_uses_first_numeric_token_inside_mixed_args() {
    assert_eq!(parse_log_lines("foo 30 bar"), 30);
}
