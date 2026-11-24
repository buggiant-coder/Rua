use std::fmt;

use llm_multiplex::LlmError;

pub fn is_context_overflow(message: &str, code: Option<&str>) -> bool {
    const CODES: &[&str] = &[
        "context_length_exceeded",
        "prompt_too_long",
        "input_tokens_overflow",
    ];
    // Provider-native codes that do not by themselves mean "too long", but are
    // the generic bucket those providers put input-length rejections in.
    const GENERIC_REJECTION_CODES: &[&str] = &[
        "400",
        "invalid_request_error",
        "bad_request",
        "invalidparameter",
        "invalid_parameter",
        "invalid_parameter_value",
    ];
    // An input-length rejection without a dedicated code still names the limit
    // in its message, so these phrases are checked even for a generic code.
    const LENGTH_PHRASES: &[&str] = &[
        "maximum context length",
        "exceeds the context window",
        "exceeds context window",
        "model's maximum context",
        "prompt is too long",
        "prompt too long",
        "context overflow",
        "range of input length",
        "input length should be",
        "input is too long",
        "too many input tokens",
    ];
    if let Some(code) = code {
        if CODES.iter().any(|known| code.eq_ignore_ascii_case(known)) {
            return true;
        }
        // A specific non-context code takes precedence over message heuristics.
        // Generic rejection codes (including DashScope's `InvalidParameter`)
        // still fall through so their message can be inspected below.
        if !GENERIC_REJECTION_CODES
            .iter()
            .any(|generic| code.eq_ignore_ascii_case(generic))
        {
            return false;
        }
    }
    let message = message.to_lowercase();
    CODES.iter().any(|code| message.contains(code))
        || LENGTH_PHRASES.iter().any(|phrase| message.contains(phrase))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorKind {
    RateLimited,

    Timeout,

    ModelUnavailable,

    AuthFailed,

    ContextTooLong,

    NonRetryable,
}

impl ErrorKind {
    pub fn classify(error: &LlmError) -> Self {
        match error {
            LlmError::RateLimit { .. } => ErrorKind::RateLimited,
            LlmError::Timeout { .. } => ErrorKind::Timeout,
            LlmError::Auth { .. } => ErrorKind::AuthFailed,
            LlmError::Server { .. }
            | LlmError::Connection { .. }
            | LlmError::ModelNotFound { .. } => ErrorKind::ModelUnavailable,
            LlmError::InvalidResponse { message, .. } | LlmError::Unknown { message, .. }
                if is_context_overflow(message, None) =>
            {
                ErrorKind::ContextTooLong
            }
            LlmError::InvalidResponse { .. }
            | LlmError::Unknown { .. }
            | LlmError::RetryDeadlineExceeded { .. }
            | LlmError::Cancelled
            | LlmError::Config { .. } => ErrorKind::NonRetryable,
        }
    }

    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            ErrorKind::RateLimited | ErrorKind::Timeout | ErrorKind::ModelUnavailable
        )
    }
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ErrorKind::RateLimited => write!(f, "rate_limited"),
            ErrorKind::Timeout => write!(f, "timeout"),
            ErrorKind::ModelUnavailable => write!(f, "model_unavailable"),
            ErrorKind::AuthFailed => write!(f, "auth_failed"),
            ErrorKind::ContextTooLong => write!(f, "context_too_long"),
            ErrorKind::NonRetryable => write!(f, "non_retryable"),
        }
    }
}
