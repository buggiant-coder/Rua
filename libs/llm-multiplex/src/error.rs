use std::time::Duration;

#[derive(thiserror::Error, Debug)]
pub enum LlmError {
    #[error("auth failed for provider {provider}{status_suffix}: {message}", status_suffix = status.map(|value| format!(" (HTTP {value})")).unwrap_or_default())]
    Auth {
        provider: String,
        status: Option<u16>,
        message: String,
    },

    #[error("rate limited by provider {provider}")]
    RateLimit {
        provider: String,
        retry_after: Option<Duration>,
        message: String,
    },

    #[error("provider server error {status:?}: {message}")]
    Server {
        provider: String,
        status: Option<u16>,
        message: String,
    },

    #[error("request timeout after {timeout:?}")]
    Timeout { provider: String, timeout: Duration },

    #[error("model call retry deadline exceeded after {timeout:?}")]
    RetryDeadlineExceeded { provider: String, timeout: Duration },

    #[error("connection error: {message}")]
    Connection { provider: String, message: String },

    #[error("invalid provider response: {message}")]
    InvalidResponse { provider: String, message: String },

    #[error("model {model} not found in provider {provider}")]
    ModelNotFound { provider: String, model: String },

    #[error("request cancelled")]
    Cancelled,

    #[error("configuration error: {message}")]
    Config { message: String },

    #[error("unknown provider error: {message}")]
    Unknown { provider: String, message: String },
}

impl LlmError {
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            LlmError::RateLimit { .. }
                | LlmError::Server { .. }
                | LlmError::Connection { .. }
                | LlmError::Timeout { .. }
        )
    }

    pub fn provider(&self) -> Option<&str> {
        match self {
            LlmError::Auth { provider, .. }
            | LlmError::RateLimit { provider, .. }
            | LlmError::Server { provider, .. }
            | LlmError::Timeout { provider, .. }
            | LlmError::RetryDeadlineExceeded { provider, .. }
            | LlmError::Connection { provider, .. }
            | LlmError::InvalidResponse { provider, .. }
            | LlmError::ModelNotFound { provider, .. }
            | LlmError::Unknown { provider, .. } => Some(provider),
            LlmError::Cancelled | LlmError::Config { .. } => None,
        }
    }

    pub fn parse_retry_after_header(value: &str) -> Option<Duration> {
        let seconds = value.trim().parse::<f64>().ok()?;
        if !seconds.is_finite() || seconds < 0.0 {
            return None;
        }
        Some(Duration::from_secs_f64(seconds))
    }

    pub fn from_http_status(
        provider: &str,
        status: u16,
        body: &str,
        retry_after_header: Option<&str>,
    ) -> Self {
        match status {
            401 | 403 => LlmError::Auth {
                provider: provider.to_string(),
                status: Some(status),
                message: Self::safe_error_message(body),
            },
            404 => LlmError::ModelNotFound {
                provider: provider.to_string(),
                model: String::new(),
            },
            429 => {
                let retry_after = retry_after_header
                    .and_then(Self::parse_retry_after_header)
                    .or_else(|| {
                        serde_json::from_str::<serde_json::Value>(body)
                            .ok()
                            .and_then(|v| v.get("retry_after").and_then(|r| r.as_f64()))
                            .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
                            .map(Duration::from_secs_f64)
                    });
                LlmError::RateLimit {
                    provider: provider.to_string(),
                    retry_after,
                    message: Self::safe_error_message(body),
                }
            }
            408 => LlmError::Timeout {
                provider: provider.to_string(),
                timeout: Duration::from_secs(0),
            },
            529 => {
                let retry_after = retry_after_header.and_then(Self::parse_retry_after_header);
                LlmError::RateLimit {
                    provider: provider.to_string(),
                    retry_after,
                    message: Self::safe_error_message(body),
                }
            }
            s if (500..600).contains(&s) => LlmError::Server {
                provider: provider.to_string(),
                status: Some(status),
                message: Self::safe_error_message(body),
            },
            _ => LlmError::Unknown {
                provider: provider.to_string(),
                message: format!("HTTP {status}: {}", Self::safe_error_message(body)),
            },
        }
    }

    fn safe_error_message(body: &str) -> String {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(body) {
            if let Some(msg) = v
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
            {
                return Self::redact_secret_fragments(msg);
            }
            if let Some(msg) = v.get("message").and_then(|m| m.as_str()) {
                return Self::redact_secret_fragments(msg);
            }
            if let Some(msg) = v.get("detail").and_then(|m| m.as_str()) {
                return Self::redact_secret_fragments(msg);
            }
        }

        let truncated: String = body.chars().take(200).collect();
        Self::redact_secret_fragments(&truncated)
    }

    fn redact_secret_fragments(message: &str) -> String {
        let mut redact_next_token = false;
        let mut redacted = Vec::new();

        for token in message.split_whitespace() {
            if redact_next_token && is_secret_payload_token(token) {
                redacted.push(Self::replace_trimmed_token(token, "***"));
                redact_next_token = false;
                continue;
            }

            redact_next_token = token
                .trim_matches(|c: char| !c.is_ascii_alphanumeric())
                .eq_ignore_ascii_case("bearer");
            redacted.push(Self::redact_secret_token(token));
        }

        redacted.join(" ")
    }

    fn redact_secret_token(token: &str) -> String {
        let trimmed = trim_token_punctuation(token);

        if is_secret_like_token(trimmed) {
            Self::replace_trimmed_token(token, "***")
        } else {
            token.to_string()
        }
    }

    fn replace_trimmed_token(token: &str, replacement: &str) -> String {
        let trimmed = token.trim_matches(|c: char| {
            matches!(
                c,
                '"' | '\'' | '`' | ',' | ';' | ':' | ')' | '(' | ']' | '[' | '}' | '{' | '.'
            )
        });
        token.replace(trimmed, replacement)
    }
}

pub(crate) fn is_retryable_provider_error(code: Option<&str>, message: &str) -> bool {
    let code = code.unwrap_or_default().to_ascii_lowercase();
    if matches!(
        code.as_str(),
        "rate_limit_error"
            | "rate_limit_exceeded"
            | "server_error"
            | "overloaded_error"
            | "api_connection_error"
            | "timeout"
    ) {
        return true;
    }

    let message = message.to_ascii_lowercase();
    [
        "upstream connect error",
        "disconnect/reset before headers",
        "connection termination",
        "connection reset",
        "connection refused",
        "temporarily unavailable",
        "service unavailable",
        "gateway timeout",
    ]
    .iter()
    .any(|signal| message.contains(signal))
}

fn is_secret_like_token(token: &str) -> bool {
    let lower = token.to_ascii_lowercase();
    lower.starts_with("sk-")
        || lower.starts_with("sk_")
        || lower.starts_with("sk-ant-")
        || lower.starts_with("sk-or-")
        || lower.starts_with("sk-sp")
        || lower.starts_with("gsk_")
}

fn is_secret_payload_token(token: &str) -> bool {
    trim_token_punctuation(token)
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .count()
        >= 16
}

fn trim_token_punctuation(token: &str) -> &str {
    token.trim_matches(|c: char| {
        matches!(
            c,
            '"' | '\'' | '`' | ',' | ';' | ':' | ')' | '(' | ']' | '[' | '}' | '{' | '.'
        )
    })
}
