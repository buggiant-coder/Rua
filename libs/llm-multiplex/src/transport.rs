use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use futures::{FutureExt, Stream};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use tracing::{debug, warn};

use crate::error::LlmError;

pub(crate) mod timing;

fn map_request_error(provider: &str, timeout: Duration, error: reqwest::Error) -> LlmError {
    if error.is_timeout() {
        return LlmError::Timeout {
            provider: provider.to_string(),
            timeout,
        };
    }
    let transient_request_failure = error.is_request()
        && !error.is_builder()
        && !error.is_redirect()
        && std::error::Error::source(&error).is_some();
    if error.is_connect() || error.is_body() || transient_request_failure {
        return LlmError::Connection {
            provider: provider.to_string(),
            message: error.to_string(),
        };
    }
    LlmError::Unknown {
        provider: provider.to_string(),
        message: error.to_string(),
    }
}

#[derive(Debug, Clone)]
pub struct TransportConfig {
    pub base_url: String,

    pub api_key: Option<String>,

    pub headers: HeaderMap,

    pub timeout: Duration,

    pub provider_name: String,
}

impl TransportConfig {
    pub fn new(base_url: impl Into<String>, provider_name: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            api_key: None,
            headers: HeaderMap::new(),
            timeout: Duration::from_secs(60),
            provider_name: provider_name.into(),
        }
    }

    pub fn with_api_key(mut self, key: impl Into<String>) -> Self {
        self.api_key = Some(key.into());
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn with_header(mut self, name: &str, value: &str) -> Self {
        if let (Ok(n), Ok(v)) = (HeaderName::try_from(name), HeaderValue::try_from(value)) {
            self.headers.insert(n, v);
        }
        self
    }

    pub fn with_headers_from_map(
        mut self,
        headers: &serde_json::Map<String, serde_json::Value>,
    ) -> Self {
        for (key, value) in headers {
            if let Some(val_str) = value.as_str() {
                if let (Ok(n), Ok(v)) = (
                    HeaderName::try_from(key.as_str()),
                    HeaderValue::try_from(val_str),
                ) {
                    self.headers.insert(n, v);
                }
            }
        }
        self
    }
}

pub fn validate_url(url: &str) -> Result<(), LlmError> {
    let lower = url.to_ascii_lowercase();
    if !lower.starts_with("https://") && !lower.starts_with("http://") {
        return Err(LlmError::Config {
            message: format!("base_url must use http:// or https:// scheme, got: {url}"),
        });
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct HttpTransport {
    client: reqwest::Client,
    config: TransportConfig,
    connections: Arc<timing::ConnectionRegistry>,
}

impl HttpTransport {
    pub fn new(config: TransportConfig) -> Result<Self, LlmError> {
        validate_url(&config.base_url)?;
        let mut default_headers = config.headers.clone();
        default_headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        if let Some(ref key) = config.api_key {
            let auth_value = format!("Bearer {key}");
            default_headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&auth_value).map_err(|_| LlmError::Config {
                    message: "invalid API key format".into(),
                })?,
            );
        }

        let _ = &config.timeout;
        let connections = Arc::new(timing::ConnectionRegistry::default());
        let builder = reqwest::Client::builder()
            .default_headers(default_headers)
            .connect_timeout(Duration::from_secs(5))
            .read_timeout(Duration::from_secs(600))
            .connector_layer(timing::ConnectionTimingLayer(connections.clone()))
            .dns_resolver(Arc::new(timing::TimedResolver));
        let client = rua_http::build_client(builder).map_err(|e| LlmError::Config {
            message: format!("failed to build HTTP client: {e}"),
        })?;

        Ok(Self {
            client,
            config,
            connections,
        })
    }

    pub async fn post(&self, path: &str, body: &serde_json::Value) -> Result<String, LlmError> {
        let url = format!("{}{}", self.config.base_url, path);
        debug!(provider = %self.config.provider_name, path, "POST request");

        let response = self
            .client
            .post(&url)
            .json(body)
            .send()
            .await
            .map_err(|error| {
                map_request_error(&self.config.provider_name, self.config.timeout, error)
            })?;

        let status = response.status().as_u16();
        let retry_after_header = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_owned());
        let body_text = response
            .text()
            .await
            .map_err(|e| LlmError::InvalidResponse {
                provider: self.config.provider_name.clone(),
                message: format!("failed to read response body: {e}"),
            })?;

        if status >= 400 {
            return Err(LlmError::from_http_status(
                &self.config.provider_name,
                status,
                &body_text,
                retry_after_header.as_deref(),
            ));
        }

        Ok(body_text)
    }

    pub async fn post_stream(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<impl Stream<Item = Result<Bytes, LlmError>> + Send, LlmError> {
        self.post_stream_traced(
            path,
            body,
            &mut crate::debug_capture::CaptureTrace::default(),
        )
        .await
    }

    pub(crate) async fn post_stream_traced(
        &self,
        path: &str,
        body: &serde_json::Value,
        trace: &mut crate::debug_capture::CaptureTrace,
    ) -> Result<impl Stream<Item = Result<Bytes, LlmError>> + Send, LlmError> {
        let url = format!("{}{}", self.config.base_url, path);
        let request = self.client.post(&url).json(body).build().map_err(|error| {
            map_request_error(&self.config.provider_name, self.config.timeout, error)
        })?;
        self.send_stream_request_traced(path, request, trace).await
    }

    /// Explicit replay seam. Send already-serialized bytes through the same client,
    /// preserving JSON order/whitespace, the connection pool, and body replayability.
    pub async fn post_stream_bytes_traced(
        &self,
        path: &str,
        body: Bytes,
        trace: &mut crate::debug_capture::CaptureTrace,
    ) -> Result<impl Stream<Item = Result<Bytes, LlmError>> + Send, LlmError> {
        let url = format!("{}{}", self.config.base_url, path);
        let request = self.client.post(&url).body(body).build().map_err(|error| {
            map_request_error(&self.config.provider_name, self.config.timeout, error)
        })?;
        trace.set("request_body_pre_serialized", true);
        self.send_stream_request_traced(path, request, trace).await
    }

    async fn send_stream_request_traced(
        &self,
        path: &str,
        request: reqwest::Request,
        trace: &mut crate::debug_capture::CaptureTrace,
    ) -> Result<impl Stream<Item = Result<Bytes, LlmError>> + Send, LlmError> {
        let body_bytes = request
            .body()
            .and_then(reqwest::Body::as_bytes)
            .map_or(0, <[u8]>::len);
        trace.set("request_body_bytes", body_bytes);
        trace.mark("request_serialized");
        let send_start = std::time::Instant::now();
        debug!(
            provider = %self.config.provider_name,
            path,
            body_bytes,
            "POST stream request: sending"
        );

        trace.checkpoint("http_send_started");
        let response = self.execute_traced(request, trace).await.map_err(|error| {
            map_request_error(&self.config.provider_name, self.config.timeout, error)
        })?;

        let status = response.status().as_u16();
        trace.response_headers(status, response.headers());
        let headers_received_ms = send_start.elapsed().as_millis();
        debug!(
            provider = %self.config.provider_name,
            path,
            status,
            elapsed_ms = headers_received_ms as u64,
            "POST stream request: response headers received"
        );
        if status >= 400 {
            let retry_after_header = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_owned());
            let body_text = response.text().await.unwrap_or_default();
            return Err(LlmError::from_http_status(
                &self.config.provider_name,
                status,
                &body_text,
                retry_after_header.as_deref(),
            ));
        }

        let provider_name = self.config.provider_name.clone();
        let stream = response.bytes_stream();
        let bytes_received = Arc::new(AtomicU64::new(0));
        let chunks_received = Arc::new(AtomicU64::new(0));

        Ok(futures::StreamExt::map(stream, move |result| {
            result
                .inspect(|bytes| {
                    bytes_received.fetch_add(bytes.len() as u64, Ordering::Relaxed);
                    chunks_received.fetch_add(1, Ordering::Relaxed);
                })
                .map_err(|e| {
                    let bytes = bytes_received.load(Ordering::Relaxed);
                    let chunks = chunks_received.load(Ordering::Relaxed);
                    warn!(
                        provider = %provider_name,
                        bytes_received = bytes,
                        chunks_received = chunks,
                        is_timeout = e.is_timeout(),
                        is_connect = e.is_connect(),
                        is_body = e.is_body(),
                        is_decode = e.is_decode(),
                        error = %e,
                        source = ?std::error::Error::source(&e),
                        "LLM stream read error: provider closed or sent malformed chunk after some bytes"
                    );
                    LlmError::Connection {
                        provider: provider_name.clone(),
                        message: format!(
                            "stream read error: {e} (after {bytes} bytes / {chunks} chunks)"
                        ),
                    }
                })
        }))
    }

    async fn execute_traced(
        &self,
        request: reqwest::Request,
        trace: &mut crate::debug_capture::CaptureTrace,
    ) -> Result<reqwest::Response, reqwest::Error> {
        let Some(timing) = trace.observe_http(self.connections.clone()) else {
            return self.client.execute(request).await;
        };
        let response = timing::ACTIVE_HTTP_TIMING.scope(timing.clone(), async {
            let response = self.client.execute(request);
            tokio::pin!(response);
            let period = Duration::from_millis(250);
            let wait_started = tokio::time::Instant::now();
            let mut ticker = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    result = &mut response => {
                        // The response and timer can become ready during the same
                        // polling stall. Do not lose that stall when response wins.
                        if let Some(scheduled) = ticker.tick().now_or_never() {
                            timing.scheduler_tick(tokio::time::Instant::now().saturating_duration_since(scheduled));
                        }
                        break result;
                    }
                    _ = timing.changed.notified() => {
                        for (name, elapsed_ms) in timing.checkpoints() {
                            trace.checkpoint_at(name, elapsed_ms);
                        }
                    }
                    scheduled = ticker.tick() => {
                        timing.scheduler_tick(tokio::time::Instant::now().saturating_duration_since(scheduled));
                        // Bounded live snapshots, not a per-tick/per-token log.
                        for (seconds, name) in [(1, "http_wait_1s"), (10, "http_wait_10s"), (30, "http_wait_30s")] {
                            if wait_started.elapsed().as_secs() >= seconds {
                                trace.checkpoint(name);
                            }
                        }
                    }
                }
            }
        }).await;
        for (name, elapsed_ms) in timing.checkpoints() {
            trace.checkpoint_at(name, elapsed_ms);
        }
        if let Ok(response) = &response {
            timing.response(response);
        }
        response
    }

    pub async fn get(&self, path: &str) -> Result<String, LlmError> {
        let url = format!("{}{}", self.config.base_url, path);
        debug!(provider = %self.config.provider_name, path, "GET request");

        let response = self.client.get(&url).send().await.map_err(|error| {
            map_request_error(&self.config.provider_name, self.config.timeout, error)
        })?;

        let status = response.status().as_u16();
        let retry_after_header = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_owned());
        let body_text = response
            .text()
            .await
            .map_err(|e| LlmError::InvalidResponse {
                provider: self.config.provider_name.clone(),
                message: format!("failed to read response body: {e}"),
            })?;

        if status >= 400 {
            return Err(LlmError::from_http_status(
                &self.config.provider_name,
                status,
                &body_text,
                retry_after_header.as_deref(),
            ));
        }

        Ok(body_text)
    }

    pub fn provider_name(&self) -> &str {
        &self.config.provider_name
    }

    pub fn base_url(&self) -> &str {
        &self.config.base_url
    }
}

pub fn parse_sse_lines(buffer: &str) -> (Vec<String>, String) {
    let mut data_events = Vec::new();
    let mut event_start = 0;
    let mut cursor = 0;

    while let Some((line_end, next_line)) = next_sse_line(buffer, cursor) {
        if line_end == cursor {
            if let Some(data) = parse_sse_event(&buffer[event_start..cursor]) {
                data_events.push(data);
            }
            event_start = next_line;
        }
        cursor = next_line;
    }

    (data_events, buffer[event_start..].to_string())
}

fn next_sse_line(buffer: &str, start: usize) -> Option<(usize, usize)> {
    let bytes = buffer.as_bytes();
    for index in start..bytes.len() {
        match bytes[index] {
            b'\n' => return Some((index, index + 1)),
            b'\r' if index + 1 < bytes.len() => {
                let next = if bytes[index + 1] == b'\n' {
                    index + 2
                } else {
                    index + 1
                };
                return Some((index, next));
            }
            b'\r' => return None,
            _ => {}
        }
    }
    None
}

fn parse_sse_event(event: &str) -> Option<String> {
    let data = event
        .split(['\n', '\r'])
        .filter_map(|line| {
            line.strip_prefix("data:")
                .map(|value| value.strip_prefix(' ').unwrap_or(value))
        })
        .collect::<Vec<_>>();

    (!data.is_empty()).then(|| data.join("\n"))
}

pub fn parse_anthropic_sse(buffer: &str) -> (Vec<(String, String)>, String) {
    let mut events = Vec::new();
    let mut current_event = String::new();
    let mut current_data = String::new();
    let mut remaining = String::new();

    let lines: Vec<&str> = buffer.split('\n').collect();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];
        if let Some(event) = line.strip_prefix("event: ") {
            current_event = event.trim().to_string();
        } else if let Some(data) = line.strip_prefix("data: ") {
            current_data = data.to_string();
        } else if line.is_empty() && !current_event.is_empty() {
            events.push((current_event.clone(), current_data.clone()));
            current_event.clear();
            current_data.clear();
        }
        i += 1;
    }

    if !current_event.is_empty() || !current_data.is_empty() {
        if !current_event.is_empty() {
            remaining.push_str(&format!("event: {current_event}\n"));
        }
        if !current_data.is_empty() {
            remaining.push_str(&format!("data: {current_data}\n"));
        }
    }

    (events, remaining)
}
