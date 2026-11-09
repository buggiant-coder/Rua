use std::fmt;

use reqwest::{Client, ClientBuilder};

pub mod diagnostics;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpClientError {
    Build { message: String },
    TlsConfig { message: String },
}

impl fmt::Display for HttpClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Build { message } => write!(f, "HTTP client build failed: {message}"),
            Self::TlsConfig { message } => write!(f, "HTTP client TLS config failed: {message}"),
        }
    }
}

impl std::error::Error for HttpClientError {}

pub fn build_client(builder: ClientBuilder) -> Result<Client, HttpClientError> {
    configure_tls_backend(builder)?
        .build()
        .map_err(|err| HttpClientError::Build {
            message: err.to_string(),
        })
}

pub fn default_client() -> Result<Client, HttpClientError> {
    build_client(Client::builder())
}

pub fn build_client_or_default(builder: ClientBuilder, log_context: &'static str) -> Client {
    build_client(builder).unwrap_or_else(|err| {
        tracing::warn!(error = %err, context = log_context, "failed to build HTTP client; using reqwest default client");
        Client::new()
    })
}

pub fn default_client_or_default(log_context: &'static str) -> Client {
    build_client_or_default(Client::builder(), log_context)
}

pub fn build_blocking_client(
    builder: reqwest::blocking::ClientBuilder,
) -> Result<reqwest::blocking::Client, HttpClientError> {
    configure_blocking_tls_backend(builder)?
        .build()
        .map_err(|err| HttpClientError::Build {
            message: err.to_string(),
        })
}

pub fn build_blocking_client_or_default(
    builder: reqwest::blocking::ClientBuilder,
    log_context: &'static str,
) -> reqwest::blocking::Client {
    build_blocking_client(builder).unwrap_or_else(|err| {
        tracing::warn!(error = %err, context = log_context, "failed to build blocking HTTP client; using reqwest default client");
        reqwest::blocking::Client::new()
    })
}

pub fn configure_tls_backend(builder: ClientBuilder) -> Result<ClientBuilder, HttpClientError> {
    configure_tls_backend_impl(builder)
}

#[cfg(not(target_os = "android"))]
fn configure_tls_backend_impl(builder: ClientBuilder) -> Result<ClientBuilder, HttpClientError> {
    Ok(builder)
}

#[cfg(not(target_os = "android"))]
fn configure_blocking_tls_backend(
    builder: reqwest::blocking::ClientBuilder,
) -> Result<reqwest::blocking::ClientBuilder, HttpClientError> {
    Ok(builder)
}

#[cfg(target_os = "android")]
fn configure_tls_backend_impl(builder: ClientBuilder) -> Result<ClientBuilder, HttpClientError> {
    Ok(builder.use_preconfigured_tls(android_webpki_tls_config()?))
}

#[cfg(target_os = "android")]
fn configure_blocking_tls_backend(
    builder: reqwest::blocking::ClientBuilder,
) -> Result<reqwest::blocking::ClientBuilder, HttpClientError> {
    Ok(builder.use_preconfigured_tls(android_webpki_tls_config()?))
}

#[cfg(target_os = "android")]
fn android_webpki_tls_config() -> Result<rustls::ClientConfig, HttpClientError> {
    let root_store =
        rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let crypto_provider = std::sync::Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(crypto_provider)
        .with_safe_default_protocol_versions()
        .map_err(|err| HttpClientError::TlsConfig {
            message: format!("failed to configure Android TLS protocol versions: {err}"),
        })?
        .with_root_certificates(root_store)
        .with_no_client_auth();
    Ok(config)
}
