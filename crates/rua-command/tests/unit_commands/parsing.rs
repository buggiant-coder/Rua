use rua_command::{is_command, parse_command};

#[test]
fn is_command_accepts_leading_whitespace() {
    assert!(is_command("/clear"));
    assert!(is_command(" \t\n/status"));
}

#[test]
fn is_command_rejects_plain_text_and_empty_input() {
    assert!(!is_command("hello /clear"));
    assert!(!is_command("   "));
    assert!(!is_command(""));
}

#[test]
fn is_command_rejects_absolute_paths_that_look_like_slash_commands() {
    assert!(!is_command("/usr/local/bin"));
    assert!(!is_command(" /tmp/project "));
}

#[test]
fn parse_command_trims_wrapper_whitespace_and_separates_args() {
    assert_eq!(
        parse_command(" \t/compact summarize only recent errors \n"),
        Some(("compact", "summarize only recent errors"))
    );
}

#[test]
fn parse_command_preserves_inner_argument_spacing() {
    assert_eq!(
        parse_command("/acp claude /tmp/project with spaces"),
        Some(("acp", "claude /tmp/project with spaces"))
    );
}

#[test]
fn parse_command_treats_command_without_args_as_empty_args() {
    assert_eq!(parse_command("/skills"), Some(("skills", "")));
}

#[test]
fn parse_command_returns_none_for_non_command_input() {
    assert_eq!(parse_command("hello"), None);
    assert_eq!(parse_command("before /status"), None);
}

#[test]
fn parse_command_returns_none_for_empty_command_token() {
    assert_eq!(parse_command("/"), None);
    assert_eq!(parse_command("/   "), None);
}

#[test]
fn parse_command_returns_none_for_path_like_slash_tokens() {
    assert_eq!(parse_command("/usr/local/bin"), None);
    assert_eq!(parse_command("/tmp/project with args"), None);
}

#[test]
fn parse_command_keeps_daemon_subcommand_in_args() {
    assert_eq!(
        parse_command("/daemon reload-config --all"),
        Some(("daemon", "reload-config --all"))
    );
}
