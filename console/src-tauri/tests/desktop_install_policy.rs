use std::path::Path;

use rua_desktop::desktop_install::{
    legacy_command_matches, parse_endpoint_port, require_migration_confirmation,
};

#[test]
fn installed_endpoint_parser_accepts_only_a_nonzero_port() {
    assert_eq!(
        parse_endpoint_port("RUA_HOST=127.0.0.1\nRUA_PORT=2026\n"),
        Some(2026)
    );
    assert_eq!(parse_endpoint_port("RUA_PORT=0\n"), None);
    assert_eq!(parse_endpoint_port("RUA_PORT=not-a-port\n"), None);
}

#[test]
fn legacy_process_command_requires_server_start_and_the_same_rua_root() {
    let rua_root = Path::new("/Users/test/.rua");
    assert!(legacy_command_matches(
        &[
            "rua",
            "server",
            "start",
            "--port",
            "2026",
            "--rua-root",
            "/Users/test/.rua"
        ],
        rua_root
    ));
    assert!(!legacy_command_matches(
        &["rua", "server", "start", "--rua-root", "/Users/other/.rua"],
        rua_root
    ));
    assert!(!legacy_command_matches(
        &["rua", "doctor", "--rua-root", "/Users/test/.rua"],
        rua_root
    ));
}

#[test]
fn legacy_migration_requires_explicit_confirmation() {
    assert!(require_migration_confirmation(false).is_err());
    assert!(require_migration_confirmation(true).is_ok());
}
