use rua_config::encrypted_io::EncryptedFieldIO;
use rua_mcp::secret_field_io::encrypt_client_value;
use serde_json::json;

const KEY: [u8; 32] = [9; 32];

#[test]
fn encrypted_mcp_envelopes_point_to_set_secret_cli() {
    let encrypted = encrypt_client_value(
        &json!({
            "name": "Demo",
            "transport": "streamable_http",
            "headers": {
                "Authorization": "Bearer original"
            }
        }),
        Some(&KEY),
        "shared mcp client demo",
    )
    .expect("encrypt");

    let envelope = &encrypted["headers"]["Authorization"];
    assert!(EncryptedFieldIO::is_encrypted_value(envelope));
    let hint = envelope["tool_hint"].as_str().expect("tool hint");
    assert!(hint.contains("rua mcp set-secret"));
    assert!(hint.contains("--path headers.Authorization"));
    assert!(hint.contains("--shared"));
    assert!(hint.contains("Do not edit encrypted envelopes by hand"));
}
