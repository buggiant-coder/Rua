use rua_mcp::{secret_mask, SharedMcpStore};
use serde_json::json;

#[test]
fn masks_cookie_headers_and_raw_extra_secrets() {
    let raw = json!({
        "name": "Demo MCP",
        "transport": "streamable_http",
        "url": "https://mcp.example.test",
        "headers": {
            "Cookie": "session=header-cookie",
            "Set-Cookie": "session=header-set-cookie",
            "User-Agent": "rua"
        },
        "oauth": {
            "client_id": "public-client",
            "access_token": "oauth-access-token",
            "refresh_token": "oauth-refresh-token",
            "client_secret": "oauth-client-secret"
        },
        "extra_token": "raw-extra-token",
        "nested": {
            "database_password": "db-password",
            "display_name": "Nested"
        }
    });

    let mut dto = SharedMcpStore::value_to_dto("demo", &raw).expect("parse MCP DTO");
    secret_mask::mask_client_secrets(&mut dto);

    let headers = match &dto.server.transport {
        mcp_manager::types::McpTransportConfig::StreamableHttp { headers, .. } => headers,
        other => panic!("unexpected transport: {other:?}"),
    };
    assert!(secret_mask::is_masked_value(&headers["Cookie"]));
    assert!(secret_mask::is_masked_value(&headers["Set-Cookie"]));
    assert_eq!(headers["User-Agent"], "rua");

    let oauth = dto.server.oauth.as_ref().expect("oauth");
    assert!(secret_mask::is_masked_value(&oauth.access_token));
    assert!(secret_mask::is_masked_value(&oauth.refresh_token));
    assert!(secret_mask::is_masked_value(
        oauth.extra["client_secret"]
            .as_str()
            .expect("client_secret")
    ));

    assert!(secret_mask::is_masked_value(
        dto.server.extra["extra_token"]
            .as_str()
            .expect("extra_token")
    ));
    assert!(secret_mask::is_masked_value(
        dto.server.extra["nested"]["database_password"]
            .as_str()
            .expect("database_password")
    ));
    assert_eq!(dto.server.extra["nested"]["display_name"], "Nested");
}

#[test]
fn restore_masked_values_preserves_raw_extra_secrets() {
    let existing = json!({
        "transport": "streamable_http",
        "url": "https://mcp.example.test",
        "headers": {
            "Cookie": "session=header-cookie",
            "User-Agent": "rua"
        },
        "oauth": {
            "access_token": "oauth-access-token",
            "refresh_token": "oauth-refresh-token",
            "client_secret": "oauth-client-secret"
        },
        "extra_token": "raw-extra-token",
        "nested": {
            "database_password": "db-password",
            "display_name": "Nested"
        }
    });
    let mut update: rua_mcp::store_dto::McpClientUpdate = serde_json::from_value(json!({
        "headers": {
            "Cookie": secret_mask::mask_env_value("session=header-cookie"),
            "User-Agent": "rua"
        },
        "oauth": {
            "access_token": secret_mask::mask_env_value("oauth-access-token"),
            "refresh_token": secret_mask::mask_env_value("oauth-refresh-token"),
            "client_secret": secret_mask::mask_env_value("oauth-client-secret")
        },
        "extra_token": secret_mask::mask_env_value("raw-extra-token"),
        "nested": {
            "database_password": secret_mask::mask_env_value("db-password"),
            "display_name": "Nested Renamed"
        }
    }))
    .expect("update DTO");

    secret_mask::restore_masked_values(&mut update, &existing);

    assert_eq!(
        update.headers.as_ref().unwrap()["Cookie"],
        "session=header-cookie"
    );
    let oauth = update.oauth.as_ref().expect("oauth");
    assert_eq!(oauth.access_token, "oauth-access-token");
    assert_eq!(oauth.refresh_token, "oauth-refresh-token");
    assert_eq!(oauth.extra["client_secret"], "oauth-client-secret");
    assert_eq!(update.extra["extra_token"], "raw-extra-token");
    assert_eq!(update.extra["nested"]["database_password"], "db-password");
    assert_eq!(update.extra["nested"]["display_name"], "Nested Renamed");
}
