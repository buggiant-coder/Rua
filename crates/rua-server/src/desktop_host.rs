use std::sync::Arc;

use axum::body::Body;
use axum::http::header::{
    ACCESS_CONTROL_ALLOW_HEADERS, ACCESS_CONTROL_ALLOW_METHODS, ACCESS_CONTROL_ALLOW_ORIGIN,
    ACCESS_CONTROL_REQUEST_HEADERS, ACCESS_CONTROL_REQUEST_METHOD, HOST, ORIGIN, VARY,
};
use axum::http::uri::Authority;
use axum::http::{HeaderMap, HeaderValue, Method, Request, StatusCode, Uri};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use subtle::ConstantTimeEq;

use crate::state::AppState;

pub const DESKTOP_SHUTDOWN_TOKEN_ENV: &str = "RUA_DESKTOP_SHUTDOWN_TOKEN";
pub const DESKTOP_SHUTDOWN_TOKEN_HEADER: &str = "x-rua-desktop-shutdown-token";
pub const DESKTOP_SHUTDOWN_PATH: &str = "/api/desktop/shutdown";

#[derive(Clone)]
pub struct DesktopHostConfig {
    shutdown_token: Arc<[u8]>,
}

impl DesktopHostConfig {
    pub fn from_shutdown_token(token: String) -> Option<Self> {
        (!token.is_empty()).then(|| Self {
            shutdown_token: Arc::from(token.into_bytes()),
        })
    }

    pub fn from_environment() -> Option<Self> {
        std::env::var(DESKTOP_SHUTDOWN_TOKEN_ENV)
            .ok()
            .and_then(Self::from_shutdown_token)
    }
}

pub fn router(state: AppState, config: Option<DesktopHostConfig>) -> Router<AppState> {
    let Some(config) = config else {
        return Router::new();
    };

    Router::new().route(
        DESKTOP_SHUTDOWN_PATH,
        post(move |headers: HeaderMap| desktop_shutdown(headers, state.clone(), config.clone())),
    )
}

pub async fn security_middleware(request: Request<Body>, next: Next) -> Response {
    let Some(host) = request.headers().get(HOST).and_then(header_text) else {
        return bad_request("Missing Host header");
    };
    if !allowed_loopback_host(host) {
        return bad_request("Invalid desktop Host");
    }

    let origin = request.headers().get(ORIGIN).cloned();
    let path = request.uri().path();
    if request.method() == Method::OPTIONS
        && request
            .headers()
            .contains_key(ACCESS_CONTROL_REQUEST_METHOD)
    {
        return preflight_response(request.headers(), host, path, origin.as_ref());
    }

    let cors_origin = match origin.as_ref() {
        None => None,
        Some(origin) if same_origin(origin, host) => Some(origin.clone()),
        Some(_) if public_cross_origin_read(request.method(), path) => {
            Some(HeaderValue::from_static("*"))
        }
        Some(_) => return forbidden_origin(),
    };

    let mut response = next.run(request).await;
    if let Some(origin) = cors_origin {
        apply_cors_origin(response.headers_mut(), origin);
    }
    response
}

#[doc(hidden)]
pub fn allowed_loopback_host(value: &str) -> bool {
    let Ok(authority) = value.parse::<Authority>() else {
        return false;
    };
    let host = authority.host().trim_matches(['[', ']']);
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

#[doc(hidden)]
pub fn origin_matches_host(origin: &str, host: &str) -> bool {
    let Ok(uri) = origin.parse::<Uri>() else {
        return false;
    };
    if uri.scheme_str() != Some("http") {
        return false;
    }
    let Some(origin_authority) = uri.authority() else {
        return false;
    };
    let Ok(host_authority) = host.parse::<Authority>() else {
        return false;
    };
    authority_key(origin_authority) == authority_key(&host_authority)
}

async fn desktop_shutdown(
    headers: HeaderMap,
    state: AppState,
    config: DesktopHostConfig,
) -> Response {
    let provided = headers
        .get(DESKTOP_SHUTDOWN_TOKEN_HEADER)
        .map(|value| value.as_bytes())
        .unwrap_or_default();
    if !constant_time_token_matches(&config.shutdown_token, provided) {
        return not_found();
    }

    state.signal_shutdown();
    (
        StatusCode::ACCEPTED,
        Json(serde_json::json!({ "status": "shutting_down" })),
    )
        .into_response()
}

fn preflight_response(
    headers: &HeaderMap,
    host: &str,
    path: &str,
    origin: Option<&HeaderValue>,
) -> Response {
    let Some(origin) = origin else {
        return forbidden_origin();
    };
    let requested_method = headers
        .get(ACCESS_CONTROL_REQUEST_METHOD)
        .and_then(header_text)
        .and_then(|value| value.parse::<Method>().ok());
    let Some(requested_method) = requested_method else {
        return bad_request("Invalid preflight method");
    };

    let same_origin = same_origin(origin, host);
    if !same_origin && !public_cross_origin_read(&requested_method, path) {
        return forbidden_origin();
    }
    if !requested_headers_allowed(headers.get(ACCESS_CONTROL_REQUEST_HEADERS)) {
        return bad_request("Unsupported preflight headers");
    }

    let mut response = StatusCode::NO_CONTENT.into_response();
    let allowed_origin = if same_origin {
        origin.clone()
    } else {
        HeaderValue::from_static("*")
    };
    apply_cors_origin(response.headers_mut(), allowed_origin);
    response.headers_mut().insert(
        ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("GET, HEAD, POST, PUT, PATCH, DELETE, OPTIONS"),
    );
    response.headers_mut().insert(
        ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static(
            "accept, authorization, content-type, x-agent-id, x-api-token, x-request-id, x-rua-desktop-shutdown-token",
        ),
    );
    response
}

fn same_origin(origin: &HeaderValue, host: &str) -> bool {
    header_text(origin).is_some_and(|origin| origin_matches_host(origin, host))
}

fn public_cross_origin_read(method: &Method, path: &str) -> bool {
    matches!(*method, Method::GET | Method::HEAD)
        && matches!(path, "/healthz" | "/readyz" | "/api/version")
}

fn requested_headers_allowed(value: Option<&HeaderValue>) -> bool {
    const ALLOWED: &[&str] = &[
        "accept",
        "authorization",
        "content-type",
        "x-agent-id",
        "x-api-token",
        "x-request-id",
        DESKTOP_SHUTDOWN_TOKEN_HEADER,
    ];
    let Some(value) = value else {
        return true;
    };
    header_text(value).is_some_and(|value| {
        value.split(',').map(str::trim).all(|header| {
            ALLOWED
                .iter()
                .any(|allowed| header.eq_ignore_ascii_case(allowed))
        })
    })
}

fn authority_key(authority: &Authority) -> (String, u16) {
    (
        authority
            .host()
            .trim_matches(['[', ']'])
            .to_ascii_lowercase(),
        authority.port_u16().unwrap_or(80),
    )
}

fn apply_cors_origin(headers: &mut HeaderMap, origin: HeaderValue) {
    headers.insert(ACCESS_CONTROL_ALLOW_ORIGIN, origin);
    headers.insert(VARY, HeaderValue::from_static("Origin"));
}

fn header_text(value: &HeaderValue) -> Option<&str> {
    value.to_str().ok()
}

fn constant_time_token_matches(expected: &[u8], provided: &[u8]) -> bool {
    if expected.len() != provided.len() {
        let mut padded = vec![0u8; expected.len()];
        let copy_len = provided.len().min(expected.len());
        padded[..copy_len].copy_from_slice(&provided[..copy_len]);
        let _ = expected.ct_eq(&padded);
        return false;
    }
    bool::from(expected.ct_eq(provided))
}

fn not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({ "detail": "Not found" })),
    )
        .into_response()
}

fn bad_request(detail: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({ "detail": detail })),
    )
        .into_response()
}

fn forbidden_origin() -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(serde_json::json!({ "detail": "Forbidden origin" })),
    )
        .into_response()
}
