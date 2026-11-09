use anyhow::{Context, Result};
use reqwest::StatusCode;
use std::fmt;
use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

const DEFAULT_HOST: &str = "127.0.0.1";
const DEFAULT_PORT: u16 = 8765;
const CLI_ENDPOINT_CONFIG: &str = "cli-endpoint.env";
const DESKTOP_ENDPOINT_FILE: &str = "desktop-endpoint.json";
const DESKTOP_ENDPOINT_VERSION: u8 = 1;

pub struct ApiClient {
    base_url: String,
    agent_id: String,
    client: reqwest::Client,
}

#[derive(Debug)]
pub struct ApiHttpError {
    method: String,
    url: String,
    status: StatusCode,
    body: String,
}

impl ApiHttpError {
    pub fn status(&self) -> StatusCode {
        self.status
    }
}

impl fmt::Display for ApiHttpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&format_http_error(
            &self.method,
            &self.url,
            self.status,
            &self.body,
        ))
    }
}

impl std::error::Error for ApiHttpError {}

impl ApiClient {
    pub fn agent_id(&self) -> &str {
        &self.agent_id
    }
    pub fn new(base_url: &str, agent_id: &str) -> Self {
        let client = if should_bypass_proxy_for_base_url(base_url) {
            rua_http::build_client_or_default(reqwest::Client::builder().no_proxy(), "rua-cli")
        } else {
            rua_http::default_client_or_default("rua-cli")
        };

        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            agent_id: agent_id.to_string(),
            client,
        }
    }

    pub async fn get(&self, path: &str) -> Result<serde_json::Value> {
        let url = format!("{}{path}", self.base_url);
        let resp = self
            .client
            .get(&url)
            .header("X-Agent-Id", &self.agent_id)
            .send()
            .await
            .with_context(|| format!("GET {url}"))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(ApiHttpError::new("GET", url, status, body).into());
        }
        resp.json()
            .await
            .with_context(|| format!("parse GET {url}"))
    }

    pub async fn post(&self, path: &str, body: &serde_json::Value) -> Result<serde_json::Value> {
        let url = format!("{}{path}", self.base_url);
        let resp = self
            .client
            .post(&url)
            .header("X-Agent-Id", &self.agent_id)
            .json(body)
            .send()
            .await
            .with_context(|| format!("POST {url}"))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(ApiHttpError::new("POST", url, status, body).into());
        }
        resp.json()
            .await
            .with_context(|| format!("parse POST {url}"))
    }

    pub async fn put(&self, path: &str, body: &serde_json::Value) -> Result<serde_json::Value> {
        let url = format!("{}{path}", self.base_url);
        let resp = self
            .client
            .put(&url)
            .header("X-Agent-Id", &self.agent_id)
            .json(body)
            .send()
            .await
            .with_context(|| format!("PUT {url}"))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(ApiHttpError::new("PUT", url, status, body).into());
        }
        resp.json()
            .await
            .with_context(|| format!("parse PUT {url}"))
    }

    pub async fn patch(&self, path: &str, body: &serde_json::Value) -> Result<serde_json::Value> {
        let url = format!("{}{path}", self.base_url);
        let resp = self
            .client
            .patch(&url)
            .header("X-Agent-Id", &self.agent_id)
            .json(body)
            .send()
            .await
            .with_context(|| format!("PATCH {url}"))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(ApiHttpError::new("PATCH", url, status, body).into());
        }
        resp.json()
            .await
            .with_context(|| format!("parse PATCH {url}"))
    }

    pub async fn delete(&self, path: &str) -> Result<serde_json::Value> {
        let url = format!("{}{path}", self.base_url);
        let resp = self
            .client
            .delete(&url)
            .header("X-Agent-Id", &self.agent_id)
            .send()
            .await
            .with_context(|| format!("DELETE {url}"))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(ApiHttpError::new("DELETE", url, status, body).into());
        }
        let text = resp.text().await.unwrap_or_default();
        if text.is_empty() {
            Ok(serde_json::json!({"success": true}))
        } else {
            serde_json::from_str(&text).with_context(|| format!("parse DELETE {url}"))
        }
    }

    pub fn streaming_url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }

    pub async fn download_bytes(&self, path: &str) -> Result<bytes::Bytes> {
        let url = format!("{}{path}", self.base_url);
        let resp = self
            .client
            .get(&url)
            .header("X-Agent-Id", &self.agent_id)
            .send()
            .await
            .with_context(|| format!("GET {url}"))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(ApiHttpError::new("GET", url, status, body).into());
        }
        resp.bytes()
            .await
            .with_context(|| format!("read bytes GET {url}"))
    }

    pub async fn upload_bytes(&self, path: &str, data: bytes::Bytes) -> Result<serde_json::Value> {
        let url = format!("{}{path}", self.base_url);
        let resp = self
            .client
            .post(&url)
            .header("X-Agent-Id", &self.agent_id)
            .header("Content-Type", "application/octet-stream")
            .body(data)
            .send()
            .await
            .with_context(|| format!("POST {url}"))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(ApiHttpError::new("POST", url, status, body).into());
        }
        resp.json()
            .await
            .with_context(|| format!("parse POST {url}"))
    }

    pub fn inner(&self) -> &reqwest::Client {
        &self.client
    }
}

pub async fn require_server(base_url: &str) -> Result<()> {
    let base_url = base_url.trim_end_matches('/');
    let client = if should_bypass_proxy_for_base_url(base_url) {
        rua_http::build_client_or_default(reqwest::Client::builder().no_proxy(), "rua-cli")
    } else {
        rua_http::default_client_or_default("rua-cli")
    };
    let ready_url = format!("{base_url}/readyz");
    let ready = client
        .get(&ready_url)
        .timeout(std::time::Duration::from_secs(2))
        .send()
        .await;
    if ready
        .as_ref()
        .is_ok_and(|response| response.status().is_success())
    {
        return Ok(());
    }
    anyhow::bail!("Rua server is not running at {base_url}.\nStart it with: rua daemon start")
}

impl ApiHttpError {
    fn new(
        method: impl Into<String>,
        url: impl Into<String>,
        status: StatusCode,
        body: impl Into<String>,
    ) -> Self {
        Self {
            method: method.into(),
            url: url.into(),
            status,
            body: body.into(),
        }
    }
}

pub fn error_status(error: &anyhow::Error) -> Option<StatusCode> {
    error
        .downcast_ref::<ApiHttpError>()
        .map(ApiHttpError::status)
}

pub fn format_http_error(method: &str, url: &str, status: StatusCode, body: &str) -> String {
    let body = body.trim();
    if body.is_empty() {
        return format!("{method} {url} returned {status}");
    }

    if let Ok(value) = serde_json::from_str::<serde_json::Value>(body) {
        if let Some(detail) = value.get("detail") {
            let detail = match detail {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            return format!("{method} {url} returned {status}: {detail}");
        }
        if let Some(error) = value.get("error") {
            let error = match error {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            return format!("{method} {url} returned {status}: {error}");
        }
    }

    format!("{method} {url} returned {status}: {body}")
}

pub fn resolve_base_url(base_url: Option<&str>) -> String {
    let env_host = std::env::var("RUA_HOST").ok();
    let env_port = std::env::var("RUA_PORT").ok();
    let desktop_path = desktop_endpoint_path();
    let config_path = installed_endpoint_config_path();
    resolve_base_url_from_all_sources(
        base_url,
        env_host.as_deref(),
        env_port.as_deref(),
        desktop_path.as_deref(),
        config_path.as_deref(),
    )
}

#[doc(hidden)]
pub fn resolve_base_url_from_sources(
    base_url: Option<&str>,
    env_host: Option<&str>,
    env_port: Option<&str>,
    config_path: Option<&Path>,
) -> String {
    resolve_base_url_from_all_sources(base_url, env_host, env_port, None, config_path)
}

#[doc(hidden)]
pub fn resolve_base_url_from_all_sources(
    base_url: Option<&str>,
    env_host: Option<&str>,
    env_port: Option<&str>,
    desktop_path: Option<&Path>,
    config_path: Option<&Path>,
) -> String {
    if let Some(url) = base_url {
        return url.to_string();
    }

    let desktop = if non_empty(env_host).is_none() && non_empty(env_port).is_none() {
        read_ready_desktop_endpoint(desktop_path)
    } else {
        None
    };
    let installed = read_installed_endpoint(config_path);
    let host = non_empty(env_host)
        .map(str::to_string)
        .or_else(|| desktop.as_ref().map(|endpoint| endpoint.host.to_string()))
        .or(installed.host)
        .unwrap_or_else(|| DEFAULT_HOST.to_string());
    let port = non_empty(env_port)
        .map(str::to_string)
        .or_else(|| desktop.as_ref().map(|endpoint| endpoint.port.to_string()))
        .or(installed.port)
        .unwrap_or_else(|| DEFAULT_PORT.to_string());
    format!("http://{host}:{port}")
}

pub fn resolve_server_port(port: Option<u16>) -> u16 {
    let env_port = std::env::var("RUA_PORT").ok();
    let config_path = installed_endpoint_config_path();
    resolve_server_port_from_sources(port, env_port.as_deref(), config_path.as_deref())
}

#[doc(hidden)]
pub fn resolve_server_port_from_sources(
    port: Option<u16>,
    env_port: Option<&str>,
    config_path: Option<&Path>,
) -> u16 {
    port.or_else(|| non_empty(env_port).and_then(|value| value.parse().ok()))
        .or_else(|| {
            read_installed_endpoint(config_path)
                .port
                .and_then(|value| value.parse().ok())
        })
        .unwrap_or(DEFAULT_PORT)
}

#[derive(Debug, serde::Deserialize)]
struct DesktopEndpoint {
    version: u8,
    host: IpAddr,
    port: u16,
    pid: u32,
}

fn desktop_endpoint_path() -> Option<PathBuf> {
    Some(rua_config::dirs::resolve_rua_root(None).join(DESKTOP_ENDPOINT_FILE))
}

fn read_ready_desktop_endpoint(path: Option<&Path>) -> Option<DesktopEndpoint> {
    let path = path?;
    let content = std::fs::read(path).ok()?;
    let endpoint = serde_json::from_slice::<DesktopEndpoint>(&content).ok()?;
    if endpoint.version != DESKTOP_ENDPOINT_VERSION
        || !endpoint.host.is_loopback()
        || endpoint.port == 0
        || !rua_server::process_liveness::is_process_alive(endpoint.pid)
        || !desktop_endpoint_is_ready(&endpoint)
    {
        return None;
    }
    Some(endpoint)
}

fn desktop_endpoint_is_ready(endpoint: &DesktopEndpoint) -> bool {
    let address = SocketAddr::from((endpoint.host, endpoint.port));
    let Ok(mut stream) = TcpStream::connect_timeout(&address, Duration::from_millis(400)) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_millis(600)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(600)));
    let host = if endpoint.host.is_ipv6() {
        format!("[{}]:{}", endpoint.host, endpoint.port)
    } else {
        format!("{}:{}", endpoint.host, endpoint.port)
    };
    if write!(
        stream,
        "GET /readyz HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"
    )
    .and_then(|_| stream.flush())
    .is_err()
    {
        return false;
    }
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 1024];
    loop {
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => bytes.extend_from_slice(&buffer[..read]),
            Err(error)
                if error.kind() == std::io::ErrorKind::ConnectionReset && !bytes.is_empty() =>
            {
                break;
            }
            Err(_) => return false,
        }
    }
    let Ok(response) = String::from_utf8(bytes) else {
        return false;
    };
    let Some((head, body)) = response.split_once("\r\n\r\n") else {
        return false;
    };
    head.lines()
        .next()
        .is_some_and(|status| status.contains(" 200 "))
        && serde_json::from_str::<serde_json::Value>(body)
            .ok()
            .and_then(|value| value.get("ready").and_then(|ready| ready.as_bool()))
            == Some(true)
}

#[derive(Default)]
struct InstalledEndpoint {
    host: Option<String>,
    port: Option<String>,
}

fn installed_endpoint_config_path() -> Option<PathBuf> {
    let bin_dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    Some(
        bin_dir
            .join("..")
            .join("share")
            .join("rua")
            .join(CLI_ENDPOINT_CONFIG),
    )
}

fn read_installed_endpoint(config_path: Option<&Path>) -> InstalledEndpoint {
    let Some(path) = config_path else {
        return InstalledEndpoint::default();
    };
    let Ok(content) = std::fs::read_to_string(path) else {
        return InstalledEndpoint::default();
    };

    let mut endpoint = InstalledEndpoint::default();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let Some(value) = non_empty(Some(value)) else {
            continue;
        };
        match key.trim() {
            "RUA_HOST" => endpoint.host = Some(value.to_string()),
            "RUA_PORT" if value.parse::<u16>().is_ok_and(|port| port > 0) => {
                endpoint.port = Some(value.to_string());
            }
            _ => {}
        }
    }
    endpoint
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

pub fn should_bypass_proxy_for_base_url(base_url: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(base_url) else {
        return false;
    };
    let Some(host) = url.host_str() else {
        return false;
    };
    let normalized_host = host
        .trim_end_matches('.')
        .trim_start_matches('[')
        .trim_end_matches(']');
    if normalized_host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    normalized_host
        .parse::<std::net::IpAddr>()
        .is_ok_and(|ip| ip.is_loopback())
}
