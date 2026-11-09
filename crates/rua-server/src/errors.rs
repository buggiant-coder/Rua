use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use rua_core::RuaError;

#[derive(Debug)]
pub enum ApiError {
    BadRequest(String),

    Unauthorized(String),

    Forbidden(String),

    NotFound(String),

    Conflict(String),

    TooLarge(String),

    UpgradeRequired(String),

    Validation(Vec<ValidationError>),

    Internal(String),

    NotImplemented(String),

    Unavailable(String),

    Core(RuaError),
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ValidationError {
    pub loc: Vec<String>,
    pub msg: String,
    #[serde(rename = "type")]
    pub error_type: String,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, body) = match self {
            Self::BadRequest(msg) => {
                tracing::debug!(%msg, "400 bad request");
                (StatusCode::BAD_REQUEST, detail_json(&msg))
            }
            Self::Unauthorized(msg) => {
                tracing::debug!(%msg, "401 unauthorized");
                (StatusCode::UNAUTHORIZED, detail_json(&msg))
            }
            Self::Forbidden(msg) => {
                tracing::debug!(%msg, "403 forbidden");
                (StatusCode::FORBIDDEN, detail_json(&msg))
            }
            Self::NotFound(msg) => {
                tracing::debug!(%msg, "404 not found");
                (StatusCode::NOT_FOUND, detail_json(&msg))
            }
            Self::Conflict(msg) => {
                tracing::debug!(%msg, "409 conflict");
                (StatusCode::CONFLICT, detail_json(&msg))
            }
            Self::TooLarge(msg) => {
                tracing::debug!(%msg, "413 payload too large");
                (StatusCode::PAYLOAD_TOO_LARGE, detail_json(&msg))
            }
            Self::UpgradeRequired(msg) => {
                tracing::debug!(%msg, "426 upgrade required");
                (StatusCode::UPGRADE_REQUIRED, detail_json(&msg))
            }
            Self::Validation(errors) => {
                tracing::debug!(error_count = errors.len(), "422 validation error");
                let detail: Vec<serde_json::Value> = errors
                    .iter()
                    .map(|e| serde_json::to_value(e).unwrap_or_default())
                    .collect();
                let body = serde_json::json!({ "detail": detail });
                (StatusCode::UNPROCESSABLE_ENTITY, body)
            }
            Self::Internal(msg) => {
                tracing::error!(error = %msg, "internal server error");
                (StatusCode::INTERNAL_SERVER_ERROR, detail_json(&msg))
            }
            Self::NotImplemented(msg) => {
                tracing::debug!(feature = %msg, "501 not implemented");
                (
                    StatusCode::NOT_IMPLEMENTED,
                    detail_json(&format!("Not implemented: {msg}")),
                )
            }
            Self::Unavailable(msg) => {
                tracing::warn!(%msg, "503 service unavailable");
                (StatusCode::SERVICE_UNAVAILABLE, detail_json(&msg))
            }
            Self::Core(err) => {
                let status = StatusCode::from_u16(err.status_code())
                    .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
                if status.is_server_error() {
                    tracing::error!(%err, status = err.status_code(), "core error");
                } else {
                    tracing::debug!(%err, status = err.status_code(), "core error");
                }
                let body = err.to_api_detail();
                (status, body)
            }
        };
        (status, Json(body)).into_response()
    }
}

impl From<RuaError> for ApiError {
    fn from(e: RuaError) -> Self {
        Self::Core(e)
    }
}

impl From<llm_multiplex::LlmError> for ApiError {
    fn from(e: llm_multiplex::LlmError) -> Self {
        let msg = e.to_string();
        match e {
            llm_multiplex::LlmError::Auth { .. } => Self::Unauthorized(msg),
            llm_multiplex::LlmError::ModelNotFound { .. } => Self::NotFound(msg),
            llm_multiplex::LlmError::RateLimit { .. }
            | llm_multiplex::LlmError::Server { .. }
            | llm_multiplex::LlmError::Timeout { .. }
            | llm_multiplex::LlmError::RetryDeadlineExceeded { .. }
            | llm_multiplex::LlmError::Connection { .. } => Self::Unavailable(msg),
            llm_multiplex::LlmError::Config { .. } => Self::BadRequest(msg),
            llm_multiplex::LlmError::InvalidResponse { .. }
            | llm_multiplex::LlmError::Cancelled
            | llm_multiplex::LlmError::Unknown { .. } => Self::Internal(msg),
        }
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadRequest(m) => write!(f, "bad request: {m}"),
            Self::Unauthorized(m) => write!(f, "unauthorized: {m}"),
            Self::Forbidden(m) => write!(f, "forbidden: {m}"),
            Self::NotFound(m) => write!(f, "not found: {m}"),
            Self::Conflict(m) => write!(f, "conflict: {m}"),
            Self::TooLarge(m) => write!(f, "too large: {m}"),
            Self::UpgradeRequired(m) => write!(f, "upgrade required: {m}"),
            Self::Validation(errs) => write!(f, "validation: {} errors", errs.len()),
            Self::Internal(m) => write!(f, "internal: {m}"),
            Self::NotImplemented(m) => write!(f, "not implemented: {m}"),
            Self::Unavailable(m) => write!(f, "unavailable: {m}"),
            Self::Core(e) => write!(f, "{e}"),
        }
    }
}

fn detail_json(message: &str) -> serde_json::Value {
    serde_json::json!({ "detail": message })
}

pub fn api_not_found() -> Response {
    ApiError::NotFound("Not found".into()).into_response()
}

pub fn api_method_not_allowed() -> Response {
    let body = detail_json("Method not allowed");
    (StatusCode::METHOD_NOT_ALLOWED, Json(body)).into_response()
}

pub fn api_not_implemented(feature: &str) -> Response {
    ApiError::NotImplemented(feature.into()).into_response()
}

pub async fn json_error_middleware(
    req: axum::http::Request<axum::body::Body>,
    next: axum::middleware::Next,
) -> Response {
    let path = req.uri().path().to_string();
    let method = req.method().to_string();

    let response = next.run(req).await;
    let status = response.status();

    if !status.is_client_error() && !status.is_server_error() {
        return response;
    }

    let content_type = response
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if content_type.contains("json") {
        return response;
    }

    let (parts, body) = response.into_parts();
    let bytes = axum::body::to_bytes(body, 64 * 1024)
        .await
        .unwrap_or_default();
    let message = String::from_utf8_lossy(&bytes);
    let message = if message.is_empty() {
        status.canonical_reason().unwrap_or("Error").to_string()
    } else {
        message.into_owned()
    };

    tracing::debug!(
        %method,
        %path,
        status = status.as_u16(),
        %message,
        "non-json error converted to json"
    );

    let json_body = detail_json(&message);
    let mut resp = (parts.status, Json(json_body)).into_response();
    for (key, value) in &parts.headers {
        if key != axum::http::header::CONTENT_TYPE && key != axum::http::header::CONTENT_LENGTH {
            resp.headers_mut().insert(key.clone(), value.clone());
        }
    }
    resp
}
