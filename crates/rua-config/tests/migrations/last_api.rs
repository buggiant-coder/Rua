use rua_config::{read_last_api, write_last_api};

#[test]
fn last_api_roundtrips_as_nested_host_port_object() {
    let dir = tempfile::tempdir().unwrap();

    assert!(read_last_api(dir.path()).is_err());

    rua_config::save_config(
        &rua_config::RootConfig::default(),
        &dir.path().join("config.json"),
    )
    .unwrap();
    write_last_api(dir.path(), "127.0.0.1", 9876).unwrap();

    assert_eq!(
        read_last_api(dir.path()).unwrap(),
        Some(("127.0.0.1".to_string(), 9876))
    );

    let raw = std::fs::read_to_string(dir.path().join("config.json")).unwrap();
    let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(value["last_api"]["host"], "127.0.0.1");
    assert_eq!(value["last_api"]["port"], 9876);
}

#[test]
fn empty_or_incomplete_last_api_is_not_reused() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("config.json"),
        r#"{"last_api": {"host": "", "port": 8765}}"#,
    )
    .unwrap();
    assert!(read_last_api(dir.path()).is_err());

    std::fs::write(
        dir.path().join("config.json"),
        r#"{"last_api": {"host": "127.0.0.1"}}"#,
    )
    .unwrap();
    assert!(read_last_api(dir.path()).is_err());
}
