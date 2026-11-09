use std::time::Duration;

use llm_multiplex::LlmError;

#[test]
fn auth_errors_preserve_provider_status_and_are_not_retryable() {
    for status in [401, 403] {
        let err = LlmError::from_http_status(
            "dashscope",
            status,
            r#"{"error":{"message":"invalid api key"}}"#,
            None,
        );

        match &err {
            LlmError::Auth {
                provider,
                status: actual_status,
                message,
            } => {
                assert_eq!(provider, "dashscope");
                assert_eq!(*actual_status, Some(status));
                assert_eq!(message, "invalid api key");
            }
            _ => panic!("expected Auth for {status}, got {err:?}"),
        }
        assert!(!err.is_retryable());
        assert_eq!(
            err.to_string(),
            format!("auth failed for provider dashscope (HTTP {status}): invalid api key")
        );
    }
}

#[test]
fn rate_limit_uses_retry_after_header_before_body() {
    let err = LlmError::from_http_status(
        "openai-compatible",
        429,
        r#"{"retry_after":60,"error":{"message":"too many requests"}}"#,
        Some("5"),
    );

    match &err {
        LlmError::RateLimit {
            provider,
            retry_after,
            message,
        } => {
            assert_eq!(provider, "openai-compatible");
            assert_eq!(*retry_after, Some(Duration::from_secs(5)));
            assert_eq!(message, "too many requests");
        }
        _ => panic!("expected RateLimit, got {err:?}"),
    }
    assert!(err.is_retryable());
}

#[test]
fn rate_limit_accepts_decimal_retry_after_header() {
    let err = LlmError::from_http_status(
        "openai-compatible",
        429,
        r#"{"error":{"message":"too many requests"}}"#,
        Some("5.0"),
    );

    match &err {
        LlmError::RateLimit { retry_after, .. } => {
            assert_eq!(*retry_after, Some(Duration::from_secs(5)));
        }
        _ => panic!("expected RateLimit, got {err:?}"),
    }
}

#[test]
fn retry_after_header_rejects_invalid_and_http_date_values() {
    assert_eq!(LlmError::parse_retry_after_header("not-a-number"), None);
    assert_eq!(
        LlmError::parse_retry_after_header("Fri, 31 Dec 2025 23:59:59 GMT"),
        None
    );
}

#[test]
fn rate_limit_falls_back_to_body_retry_after_when_header_is_absent() {
    let err = LlmError::from_http_status("openai-compatible", 429, r#"{"retry_after":60}"#, None);

    match &err {
        LlmError::RateLimit { retry_after, .. } => {
            assert_eq!(*retry_after, Some(Duration::from_secs(60)));
        }
        _ => panic!("expected RateLimit, got {err:?}"),
    }
}

#[test]
fn rate_limit_without_retry_after_keeps_retry_after_absent() {
    let err = LlmError::from_http_status(
        "openai-compatible",
        429,
        r#"{"error":{"message":"too many"}}"#,
        None,
    );

    match &err {
        LlmError::RateLimit { retry_after, .. } => {
            assert_eq!(*retry_after, None);
        }
        _ => panic!("expected RateLimit, got {err:?}"),
    }
}

#[test]
fn rate_limit_body_retry_after_rejects_negative_and_non_finite_values() {
    for body in [
        r#"{"retry_after":-1,"error":{"message":"too many"}}"#,
        r#"{"retry_after":1e309,"error":{"message":"too many"}}"#,
    ] {
        let err = LlmError::from_http_status("openai-compatible", 429, body, None);

        match &err {
            LlmError::RateLimit { retry_after, .. } => {
                assert_eq!(
                    *retry_after, None,
                    "body retry_after should be ignored: {body}"
                );
            }
            _ => panic!("expected RateLimit, got {err:?}"),
        }
    }
}

#[test]
fn anthropic_overload_status_is_retryable_rate_limit() {
    let err = LlmError::from_http_status(
        "anthropic",
        529,
        r#"{"error":{"message":"overloaded"}}"#,
        Some("30"),
    );

    match &err {
        LlmError::RateLimit {
            provider,
            retry_after,
            message,
        } => {
            assert_eq!(provider, "anthropic");
            assert_eq!(*retry_after, Some(Duration::from_secs(30)));
            assert_eq!(message, "overloaded");
        }
        _ => panic!("expected RateLimit for 529, got {err:?}"),
    }
    assert!(err.is_retryable());
}

#[test]
fn anthropic_overload_without_retry_after_keeps_retry_after_absent() {
    let err = LlmError::from_http_status(
        "anthropic",
        529,
        r#"{"error":{"message":"overloaded"}}"#,
        None,
    );

    match &err {
        LlmError::RateLimit { retry_after, .. } => {
            assert_eq!(*retry_after, None);
        }
        _ => panic!("expected RateLimit for 529, got {err:?}"),
    }
    assert!(err.is_retryable());
}

#[test]
fn server_errors_are_retryable_and_keep_status() {
    for status in [500, 502, 503, 504] {
        let err = LlmError::from_http_status(
            "gemini",
            status,
            r#"{"error":{"message":"provider unavailable"}}"#,
            None,
        );

        match &err {
            LlmError::Server {
                provider,
                status: actual_status,
                message,
            } => {
                assert_eq!(provider, "gemini");
                assert_eq!(*actual_status, Some(status));
                assert_eq!(message, "provider unavailable");
            }
            _ => panic!("expected Server for {status}, got {err:?}"),
        }
        assert!(err.is_retryable());
    }
}

#[test]
fn non_retryable_http_statuses_use_stable_error_kinds() {
    let not_found = LlmError::from_http_status(
        "dashscope",
        404,
        r#"{"error":{"message":"model not found"}}"#,
        None,
    );
    assert!(matches!(
        not_found,
        LlmError::ModelNotFound {
            ref provider,
            ref model
        } if provider == "dashscope" && model.is_empty()
    ));
    assert!(!not_found.is_retryable());

    let bad_request = LlmError::from_http_status(
        "dashscope",
        400,
        r#"{"error":{"message":"bad request"}}"#,
        None,
    );
    assert!(matches!(
        bad_request,
        LlmError::Unknown {
            ref provider,
            ref message
        } if provider == "dashscope" && message == "HTTP 400: bad request"
    ));
    assert!(!bad_request.is_retryable());
}

#[test]
fn request_timeout_is_retryable_timeout() {
    let err = LlmError::from_http_status("openai-compatible", 408, "", None);

    assert!(matches!(
        err,
        LlmError::Timeout {
            ref provider,
            timeout
        } if provider == "openai-compatible" && timeout == Duration::from_secs(0)
    ));
    assert!(err.is_retryable());
}

#[test]
fn provider_error_messages_redact_secrets_from_json_and_raw_body() {
    let cases = [
        r#"{"error":{"message":"invalid key sk-live-abcdefghijklmnopqrstuvwxyz123456"}}"#,
        r#"{"message":"Authorization: Bearer abcdefghijklmnopqrstuvwxyz123456"}"#,
        "upstream rejected api_key: sk-ant-abcdefghijklmnopqrstuvwxyz123456",
    ];

    for body in cases {
        let err = LlmError::from_http_status("openai-compatible", 500, body, None);
        let debug = format!("{err:?}");
        let display = err.to_string();

        assert!(!debug.contains("sk-live-abcdefghijklmnopqrstuvwxyz123456"));
        assert!(!debug.contains("sk-ant-abcdefghijklmnopqrstuvwxyz123456"));
        assert!(!debug.contains("abcdefghijklmnopqrstuvwxyz123456"));
        assert!(!display.contains("sk-live-abcdefghijklmnopqrstuvwxyz123456"));
        assert!(!display.contains("sk-ant-abcdefghijklmnopqrstuvwxyz123456"));
        assert!(!display.contains("abcdefghijklmnopqrstuvwxyz123456"));
    }
}

#[test]
fn provider_auth_errors_redact_short_prefixed_keys() {
    let err = LlmError::from_http_status(
        "openai-compatible",
        401,
        r#"{"error":{"message":"bad key sk-test-secret"}}"#,
        None,
    );
    assert_eq!(
        err.to_string(),
        "auth failed for provider openai-compatible (HTTP 401): bad key ***"
    );
    assert!(!format!("{err:?}").contains("sk-test-secret"));
}
