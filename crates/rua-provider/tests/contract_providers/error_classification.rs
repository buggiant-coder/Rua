use std::time::Duration;

use llm_multiplex::LlmError;
use rua_provider::ErrorKind;

#[test]
fn only_explicit_input_context_errors_trigger_compaction() {
    use rua_provider::error::is_context_overflow;

    for message in [
        "invalid authentication token",
        "maximum requests exceeded",
        "output token limit exceeded",
        "too many tokens per minute",
        "context service unavailable",
        "field name is too long",
    ] {
        assert!(!is_context_overflow(message, None), "{message}");
    }
    for message in [
        "context_length_exceeded",
        "maximum context length is 1000000 tokens",
        "prompt is too long",
        "input_tokens_overflow",
    ] {
        assert!(is_context_overflow(message, None), "{message}");
    }
    assert!(is_context_overflow(
        "input rejected",
        Some("context_length_exceeded")
    ));
    assert!(!is_context_overflow(
        "maximum context length",
        Some("rate_limit_exceeded")
    ));
}

#[test]
fn llm_errors_classify_into_provider_retry_kinds() {
    let cases = [
        (
            LlmError::Auth {
                provider: "dashscope".into(),
                status: Some(401),
                message: "bad key".into(),
            },
            ErrorKind::AuthFailed,
            false,
        ),
        (
            LlmError::RateLimit {
                provider: "dashscope".into(),
                retry_after: Some(Duration::from_secs(5)),
                message: "too many".into(),
            },
            ErrorKind::RateLimited,
            true,
        ),
        (
            LlmError::Timeout {
                provider: "gemini".into(),
                timeout: Duration::from_secs(30),
            },
            ErrorKind::Timeout,
            true,
        ),
        (
            LlmError::Server {
                provider: "anthropic".into(),
                status: Some(503),
                message: "unavailable".into(),
            },
            ErrorKind::ModelUnavailable,
            true,
        ),
        (
            LlmError::ModelNotFound {
                provider: "dashscope".into(),
                model: "missing".into(),
            },
            ErrorKind::ModelUnavailable,
            true,
        ),
        (
            LlmError::Config {
                message: "missing provider config".into(),
            },
            ErrorKind::NonRetryable,
            false,
        ),
        (
            LlmError::Unknown {
                provider: "dashscope".into(),
                message: "HTTP 400: bad request".into(),
            },
            ErrorKind::NonRetryable,
            false,
        ),
        (
            LlmError::InvalidResponse {
                provider: "dashscope".into(),
                message: "malformed JSON".into(),
            },
            ErrorKind::NonRetryable,
            false,
        ),
        (LlmError::Cancelled, ErrorKind::NonRetryable, false),
    ];

    for (error, expected_kind, expected_retryable) in cases {
        let kind = ErrorKind::classify(&error);
        assert_eq!(kind, expected_kind, "unexpected kind for {error:?}");
        assert_eq!(
            kind.is_retryable(),
            expected_retryable,
            "unexpected retryability for {error:?}"
        );
    }
}

#[test]
fn context_length_errors_are_not_retried_with_same_provider() {
    let cases = [
        LlmError::InvalidResponse {
            provider: "dashscope".into(),
            message: "context_length_exceeded".into(),
        },
        LlmError::Unknown {
            provider: "dashscope".into(),
            message: "HTTP 400: context_length_exceeded".into(),
        },
    ];

    for error in cases {
        let kind = ErrorKind::classify(&error);
        assert_eq!(kind, ErrorKind::ContextTooLong);
        assert!(!kind.is_retryable());
    }
}
