use aes_gcm::aead::{Aead, AeadCore, OsRng};
use aes_gcm::{Aes256Gcm, KeyInit};
use base64::engine::general_purpose::{STANDARD, URL_SAFE};
use base64::Engine;
use rua_server::qrcode_auth::QRCodeAuthHandler;
use rua_server::qrcode_handlers::dingtalk::DingtalkQRCodeAuthHandler;
use rua_server::qrcode_handlers::feishu::FeishuQRCodeAuthHandler;
use rua_server::qrcode_handlers::ilink_client::ILinkClient;
use rua_server::qrcode_handlers::qq::QqQRCodeAuthHandler;
use rua_server::qrcode_handlers::wecom::WecomQRCodeAuthHandler;
use rua_server::qrcode_handlers::weixin::WeixinQRCodeAuthHandler;
use wiremock::matchers::{body_json, body_string_contains, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn qq_poll_token_parts(token: &str) -> (String, String) {
    let raw = URL_SAFE.decode(token).expect("URL-safe poll token");
    let value: serde_json::Value = serde_json::from_slice(&raw).expect("JSON poll token");
    (
        value["task_id"].as_str().unwrap().to_string(),
        value["key"].as_str().unwrap().to_string(),
    )
}

fn qq_poll_token(task_id: &str, key: &str) -> String {
    URL_SAFE.encode(serde_json::json!({"task_id": task_id, "key": key}).to_string())
}

fn qq_test_key() -> String {
    STANDARD.encode([7u8; 32])
}

fn qq_encrypt_secret(secret: &str, key_base64: &str) -> String {
    let key = STANDARD.decode(key_base64).unwrap();
    let cipher = Aes256Gcm::new_from_slice(&key).unwrap();
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let ciphertext = cipher.encrypt(&nonce, secret.as_bytes()).unwrap();
    let mut raw = nonce.to_vec();
    raw.extend_from_slice(&ciphertext);
    STANDARD.encode(raw)
}

#[tokio::test]
async fn ilink_client_requests_wechat_bot_qrcode_and_status() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/ilink/bot/get_bot_qrcode"))
        .and(query_param("bot_type", "3"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "qrcode": "abc123",
            "qrcode_img_content": "data:image/png;base64,xxx"
        })))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/ilink/bot/get_qrcode_status"))
        .and(query_param("qrcode", "abc123"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "status": "confirmed",
            "bot_token": "tok-xyz",
            "baseurl": "https://custom.weixin.qq.com"
        })))
        .mount(&server)
        .await;

    let client = ILinkClient::new(Some(&server.uri()));
    let qr = client.get_bot_qrcode().await.unwrap();
    assert_eq!(qr.qrcode, "abc123");
    assert_eq!(qr.qrcode_img_content, "data:image/png;base64,xxx");

    let status = client.get_qrcode_status("abc123").await.unwrap();
    assert_eq!(status.status, "confirmed");
    assert_eq!(status.bot_token, "tok-xyz");
    assert_eq!(status.baseurl, "https://custom.weixin.qq.com");
}

#[tokio::test]
async fn wechat_handler_builds_scan_url_and_maps_credentials() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/ilink/bot/get_bot_qrcode"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "qrcode": "qr-token-abc",
            "qrcode_img_content": ""
        })))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/ilink/bot/get_qrcode_status"))
        .and(query_param("qrcode", "qr-token-abc"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "status": "confirmed",
            "bot_token": "bt-xyz",
            "baseurl": "https://custom.api.weixin.qq.com"
        })))
        .mount(&server)
        .await;

    let handler = WeixinQRCodeAuthHandler::with_ilink_base_url(server.uri());
    let qr = handler.fetch_qrcode("agent-1").await.unwrap();
    assert_eq!(
        qr.scan_url,
        "https://liteapp.weixin.qq.com/q/7GiQu1?qrcode=qr-token-abc&bot_type=3"
    );
    assert_eq!(qr.poll_token, "qr-token-abc");

    let status = handler
        .poll_status(&qr.poll_token, "agent-1")
        .await
        .unwrap();
    assert_eq!(status.status, "confirmed");
    assert_eq!(status.credentials["bot_token"], "bt-xyz");
    assert_eq!(
        status.credentials["base_url"],
        "https://custom.api.weixin.qq.com"
    );
}

#[tokio::test]
async fn wechat_handler_uses_absolute_qrcode_image_url() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/ilink/bot/get_bot_qrcode"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "qrcode": "qr-token-abc",
            "qrcode_img_content": "https://custom.weixin.qq.com/scan/xyz"
        })))
        .mount(&server)
        .await;

    let handler = WeixinQRCodeAuthHandler::with_ilink_base_url(server.uri());
    let result = handler.fetch_qrcode("agent-1").await.unwrap();
    assert_eq!(result.scan_url, "https://custom.weixin.qq.com/scan/xyz");
    assert_eq!(result.poll_token, "qr-token-abc");
}

#[tokio::test]
async fn wechat_handler_rejects_empty_qrcode_payload() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/ilink/bot/get_bot_qrcode"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "qrcode": "",
            "qrcode_img_content": ""
        })))
        .mount(&server)
        .await;

    let handler = WeixinQRCodeAuthHandler::with_ilink_base_url(server.uri());
    let err = handler.fetch_qrcode("agent-1").await.unwrap_err();
    assert!(format!("{err:?}").contains("empty QR code data"));
}

#[tokio::test]
async fn wecom_handler_parses_settings_html_and_poll_credentials() {
    let server = MockServer::start().await;
    let html = r#"<html><script>window.settings = {"scode":"sc-abc-123","auth_url":"https://work.weixin.qq.com/wework_admin/loginpage_wx?scode=sc-abc-123","nested":{"x":1}}</script></html>"#;

    Mock::given(method("GET"))
        .and(path("/ai/qc/gen"))
        .respond_with(ResponseTemplate::new(200).set_body_string(html))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/ai/qc/query_result"))
        .and(query_param("scode", "sc-abc-123"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": {
                "status": "authed",
                "bot_info": {
                    "botid": "bot-999",
                    "secret": "sec-xyz"
                }
            }
        })))
        .mount(&server)
        .await;

    let handler = WecomQRCodeAuthHandler::with_base_url(server.uri());
    let qr = handler.fetch_qrcode("agent-1").await.unwrap();
    assert_eq!(
        qr.scan_url,
        "https://work.weixin.qq.com/wework_admin/loginpage_wx?scode=sc-abc-123"
    );
    assert_eq!(qr.poll_token, "sc-abc-123");

    let status = handler
        .poll_status(&qr.poll_token, "agent-1")
        .await
        .unwrap();
    assert_eq!(status.status, "authed");
    assert_eq!(status.credentials["bot_id"], "bot-999");
    assert_eq!(status.credentials["secret"], "sec-xyz");
}

#[tokio::test]
async fn wecom_handler_rejects_missing_or_empty_settings() {
    let missing_settings = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/ai/qc/gen"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html>no settings here</html>"))
        .mount(&missing_settings)
        .await;

    let handler = WecomQRCodeAuthHandler::with_base_url(missing_settings.uri());
    let err = handler.fetch_qrcode("agent-1").await.unwrap_err();
    assert!(format!("{err:?}").contains("parse WeCom auth page settings"));

    let empty_scode = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/ai/qc/gen"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"<script>window.settings = {"scode":"","auth_url":"https://x"}</script>"#,
        ))
        .mount(&empty_scode)
        .await;

    let handler = WecomQRCodeAuthHandler::with_base_url(empty_scode.uri());
    let err = handler.fetch_qrcode("agent-1").await.unwrap_err();
    assert!(format!("{err:?}").contains("empty scode or auth_url"));
}

#[tokio::test]
async fn wecom_handler_defaults_empty_poll_status_to_waiting() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/ai/qc/query_result"))
        .and(query_param("scode", "sc-abc"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": {
                "status": "",
                "bot_info": {}
            }
        })))
        .mount(&server)
        .await;

    let handler = WecomQRCodeAuthHandler::with_base_url(server.uri());
    let result = handler.poll_status("sc-abc", "agent-1").await.unwrap();
    assert_eq!(result.status, "waiting");
    assert_eq!(result.credentials["bot_id"], "");
    assert_eq!(result.credentials["secret"], "");
}

#[tokio::test]
async fn dingtalk_handler_exchanges_nonce_for_qrcode_and_maps_poll_status() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/app/registration/init"))
        .and(body_json(serde_json::json!({"source": "RUA"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "errcode": 0,
            "errmsg": "ok",
            "nonce": "nonce-abc-123"
        })))
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/app/registration/begin"))
        .and(body_json(serde_json::json!({"nonce": "nonce-abc-123"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "errcode": 0,
            "errmsg": "ok",
            "device_code": "dev-code-xyz",
            "verification_uri_complete": "https://login.dingtalk.com/scan?code=xyz"
        })))
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/app/registration/poll"))
        .and(body_json(
            serde_json::json!({"device_code": "dev-code-xyz"}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "status": "SUCCESS",
            "client_id": "cid-123",
            "client_secret": "csec-456"
        })))
        .mount(&server)
        .await;

    let handler = DingtalkQRCodeAuthHandler::with_api_base_url(server.uri());
    let qr = handler.fetch_qrcode("agent-1").await.unwrap();
    assert_eq!(qr.scan_url, "https://login.dingtalk.com/scan?code=xyz");
    assert_eq!(qr.poll_token, "dev-code-xyz");

    let status = handler
        .poll_status(&qr.poll_token, "agent-1")
        .await
        .unwrap();
    assert_eq!(status.status, "success");
    assert_eq!(status.credentials["client_id"], "cid-123");
    assert_eq!(status.credentials["client_secret"], "csec-456");
}

#[tokio::test]
async fn dingtalk_handler_maps_waiting_expired_fail_and_init_error() {
    for (remote_status, expected_status, expected_reason) in [
        ("WAITING", "waiting", None),
        ("EXPIRED", "expired", None),
        ("FAIL", "fail", Some("user_denied")),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/app/registration/poll"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "status": remote_status,
                "fail_reason": expected_reason.unwrap_or_default()
            })))
            .mount(&server)
            .await;

        let handler = DingtalkQRCodeAuthHandler::with_api_base_url(server.uri());
        let result = handler
            .poll_status("dev-code-xyz", "agent-1")
            .await
            .unwrap();
        assert_eq!(result.status, expected_status);
        if let Some(reason) = expected_reason {
            assert_eq!(result.credentials["fail_reason"], reason);
        }
    }

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/app/registration/init"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "errcode": 40001,
            "errmsg": "invalid appkey"
        })))
        .mount(&server)
        .await;

    let handler = DingtalkQRCodeAuthHandler::with_api_base_url(server.uri());
    let err = handler.fetch_qrcode("agent-1").await.unwrap_err();
    assert!(format!("{err:?}").contains("invalid appkey"));
}

#[tokio::test]
async fn feishu_handler_runs_device_flow_and_maps_success_credentials() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/oauth/v1/app/registration"))
        .and(body_string_contains("action=init"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "supported_auth_methods": ["client_secret"]
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/oauth/v1/app/registration"))
        .and(body_string_contains("action=begin"))
        .and(body_string_contains("archetype=PersonalAgent"))
        .and(body_string_contains("auth_method=client_secret"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "device_code": "dev-code-123",
            "verification_uri_complete": "https://accounts.feishu.cn/scan?x=1"
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/oauth/v1/app/registration"))
        .and(body_string_contains("action=poll"))
        .and(body_string_contains("device_code=dev-code-123"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "client_id": "cli_a",
            "client_secret": "secret_b",
            "user_info": {
                "open_id": "ou_123",
                "tenant_brand": "feishu"
            }
        })))
        .mount(&server)
        .await;

    let handler = FeishuQRCodeAuthHandler::with_accounts_base(server.uri());
    let qr = handler.fetch_qrcode("agent-1").await.unwrap();
    assert_eq!(qr.poll_token, "dev-code-123");
    assert_eq!(
        qr.scan_url,
        "https://accounts.feishu.cn/scan?x=1&source=RUA"
    );

    let status = handler
        .poll_status("dev-code-123", "agent-1")
        .await
        .unwrap();
    assert_eq!(status.status, "success");
    assert_eq!(status.credentials["app_id"], "cli_a");
    assert_eq!(status.credentials["app_secret"], "secret_b");
    assert_eq!(status.credentials["open_id"], "ou_123");
    assert_eq!(status.credentials["tenant_brand"], "feishu");
}

#[tokio::test]
async fn feishu_handler_uses_lark_domain_and_maps_poll_errors() {
    let feishu_server = MockServer::start().await;
    let lark_server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/oauth/v1/app/registration"))
        .and(body_string_contains("action=init"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "supported_auth_methods": ["client_secret"]
        })))
        .mount(&lark_server)
        .await;
    Mock::given(method("POST"))
        .and(path("/oauth/v1/app/registration"))
        .and(body_string_contains("action=begin"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "device_code": "lark-dev-code",
            "verification_uri_complete": "https://accounts.larksuite.com/scan"
        })))
        .mount(&lark_server)
        .await;
    Mock::given(method("POST"))
        .and(path("/oauth/v1/app/registration"))
        .and(body_string_contains("action=poll"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "error": "access_denied"
        })))
        .mount(&lark_server)
        .await;

    let handler =
        FeishuQRCodeAuthHandler::with_domain_bases(feishu_server.uri(), lark_server.uri());
    let params = rua_server::qrcode_auth::QRCodeAuthParams {
        domain: Some("lark".into()),
    };
    let qr = handler
        .fetch_qrcode_with_params("agent-1", &params)
        .await
        .unwrap();
    assert_eq!(qr.poll_token, "lark-dev-code");

    let status = handler
        .poll_status_with_params("lark-dev-code", "agent-1", &params)
        .await
        .unwrap();
    assert_eq!(status.status, "fail");
    assert_eq!(
        status.credentials["fail_reason"],
        "User denied authorization"
    );
}

#[tokio::test]
async fn feishu_handler_rejects_unsupported_auth_methods() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/oauth/v1/app/registration"))
        .and(body_string_contains("action=init"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "supported_auth_methods": ["oauth"]
        })))
        .mount(&server)
        .await;

    let handler = FeishuQRCodeAuthHandler::with_accounts_base(server.uri());
    let err = handler.fetch_qrcode("agent-1").await.unwrap_err();
    assert!(format!("{err:?}").contains("unsupported auth methods"));
}

#[tokio::test]
async fn qq_handler_returns_scan_url_and_stateless_poll_token() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/lite/create_bind_task"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "retcode": 0,
            "data": {"task_id": "task-abc"}
        })))
        .mount(&server)
        .await;

    let handler = QqQRCodeAuthHandler::with_portal_base(server.uri());
    let result = handler.fetch_qrcode("agent-1").await.unwrap();

    assert_eq!(
        result.scan_url,
        format!(
            "{}/qqbot/openclaw/connect.html?task_id=task-abc&_wv=2&source=RUA",
            server.uri()
        )
    );
    let (task_id, key) = qq_poll_token_parts(&result.poll_token);
    assert_eq!(task_id, "task-abc");
    assert!(!key.is_empty());
}

#[tokio::test]
async fn qq_handler_decrypts_success_credentials() {
    let server = MockServer::start().await;
    let key = qq_test_key();
    let token = qq_poll_token("task-abc", &key);
    let encrypted = qq_encrypt_secret("client-secret", &key);

    Mock::given(method("POST"))
        .and(path("/lite/poll_bind_result"))
        .and(body_json(serde_json::json!({"task_id": "task-abc"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "retcode": 0,
            "data": {
                "status": 2,
                "bot_appid": 123456,
                "bot_encrypt_secret": encrypted,
                "user_openid": "openid-1"
            }
        })))
        .mount(&server)
        .await;

    let handler = QqQRCodeAuthHandler::with_portal_base(server.uri());
    let result = handler.poll_status(&token, "agent-1").await.unwrap();

    assert_eq!(result.status, "success");
    assert_eq!(result.credentials["app_id"], "123456");
    assert_eq!(result.credentials["client_secret"], "client-secret");
    assert_eq!(result.credentials["user_openid"], "openid-1");
}

#[tokio::test]
async fn qq_handler_maps_waiting_expired_and_failure() {
    for (remote_retcode, remote_status, expected_status, expected_reason) in [
        (0, 1, "waiting", None),
        (0, 3, "expired", None),
        (400, 0, "fail", Some("bad task")),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/lite/poll_bind_result"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "retcode": remote_retcode,
                "msg": expected_reason.unwrap_or_default(),
                "data": {"status": remote_status}
            })))
            .mount(&server)
            .await;

        let handler = QqQRCodeAuthHandler::with_portal_base(server.uri());
        let result = handler
            .poll_status(&qq_poll_token("task-abc", &qq_test_key()), "agent-1")
            .await
            .unwrap();
        assert_eq!(result.status, expected_status);
        if let Some(reason) = expected_reason {
            assert_eq!(result.credentials["fail_reason"], reason);
        }
    }
}
