use std::sync::Arc;
use std::time::Duration;

use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::sync::{watch, Mutex, RwLock};
use tokio::task::JoinHandle;

use crate::http_client::default_http_client;

const TOKEN_API_URL: &str = "https://api.dingtalk.com/v1.0/oauth2/accessToken";
const REFRESH_MARGIN: Duration = Duration::from_secs(5 * 60);
const INITIAL_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(300);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone)]
pub struct TokenState {
    pub token: String,
    pub expires_at: tokio::time::Instant,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TokenApiResponse {
    access_token: String,
    expire_in: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TokenApiRequest {
    app_key: String,
    app_secret: String,
}

pub struct AccessTokenManager {
    app_key: String,
    app_secret: String,
    state: Arc<RwLock<Option<TokenState>>>,
    client: Client,
    token_url: String,
    stop_tx: watch::Sender<bool>,
    task_handle: Mutex<Option<JoinHandle<()>>>,
}

impl AccessTokenManager {
    pub fn new(app_key: String, app_secret: String) -> Self {
        let (stop_tx, _) = watch::channel(false);
        Self {
            app_key,
            app_secret,
            state: Arc::new(RwLock::new(None)),
            client: default_http_client(),
            token_url: TOKEN_API_URL.to_string(),
            stop_tx,
            task_handle: Mutex::new(None),
        }
    }

    #[doc(hidden)]
    pub fn with_token_url(mut self, url: String) -> Self {
        self.token_url = url;
        self
    }

    pub async fn start(&self) {
        let mut handle = self.task_handle.lock().await;
        if handle.is_some() {
            tracing::warn!("token refresh task already running");
            return;
        }

        self.stop_tx.send_replace(false);
        let state = self.state.clone();
        let client = self.client.clone();
        let token_url = self.token_url.clone();
        let app_key = self.app_key.clone();
        let app_secret = self.app_secret.clone();
        let mut stop_rx = self.stop_tx.subscribe();

        *handle = Some(tokio::spawn(async move {
            refresh_loop(state, client, token_url, app_key, app_secret, &mut stop_rx).await;
        }));

        tracing::info!("dingtalk token refresh task started");
    }

    pub async fn stop(&self) {
        let _ = self.stop_tx.send(true);
        let mut handle = self.task_handle.lock().await;
        if let Some(h) = handle.take() {
            let _ = h.await;
        }
        tracing::info!("dingtalk token refresh task stopped");
    }

    pub async fn current_token(&self) -> rua_core::Result<String> {
        {
            let state = self.state.read().await;
            if let Some(ref ts) = *state {
                if tokio::time::Instant::now() < ts.expires_at {
                    return Ok(ts.token.clone());
                }
            }
        }

        let mut state = self.state.write().await;
        if let Some(ref ts) = *state {
            if tokio::time::Instant::now() < ts.expires_at {
                return Ok(ts.token.clone());
            }
        }

        let (token, expires_in) = fetch_token(
            &self.client,
            &self.token_url,
            &self.app_key,
            &self.app_secret,
        )
        .await?;
        let expires_at = tokio::time::Instant::now() + expires_in;
        *state = Some(TokenState {
            token: token.clone(),
            expires_at,
        });
        Ok(token)
    }
}

async fn fetch_token(
    client: &Client,
    token_url: &str,
    app_key: &str,
    app_secret: &str,
) -> rua_core::Result<(String, Duration)> {
    let body = TokenApiRequest {
        app_key: app_key.to_string(),
        app_secret: app_secret.to_string(),
    };

    let resp = client
        .post(token_url)
        .json(&body)
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|e| rua_core::RuaError::Runtime {
            message: format!("dingtalk token fetch failed: {e}"),
        })?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(rua_core::RuaError::Runtime {
            message: format!("dingtalk token fetch HTTP {status}: {text}"),
        });
    }

    let parsed: TokenApiResponse = resp.json().await.map_err(|e| rua_core::RuaError::Runtime {
        message: format!("dingtalk token response parse error: {e}"),
    })?;

    Ok((parsed.access_token, Duration::from_secs(parsed.expire_in)))
}

async fn refresh_loop(
    state: Arc<RwLock<Option<TokenState>>>,
    client: Client,
    token_url: String,
    app_key: String,
    app_secret: String,
    stop_rx: &mut watch::Receiver<bool>,
) {
    let mut backoff = INITIAL_BACKOFF;

    loop {
        if *stop_rx.borrow() {
            return;
        }

        match fetch_token(&client, &token_url, &app_key, &app_secret).await {
            Ok((token, expires_in)) => {
                let expires_at = tokio::time::Instant::now() + expires_in;
                *state.write().await = Some(TokenState { token, expires_at });
                backoff = INITIAL_BACKOFF;
                tracing::info!(
                    expires_in_secs = expires_in.as_secs(),
                    "dingtalk access_token refreshed"
                );

                let sleep_dur = expires_in.saturating_sub(REFRESH_MARGIN);
                tokio::select! {
                    _ = tokio::time::sleep(sleep_dur) => continue,
                    _ = stop_rx.changed() => return,
                }
            }
            Err(e) => {
                tracing::error!(
                    error = %e,
                    backoff_secs = backoff.as_secs(),
                    "dingtalk access_token refresh failed, retrying"
                );
                tokio::select! {
                    _ = tokio::time::sleep(backoff) => {
                        backoff = (backoff * 2).min(MAX_BACKOFF);
                    },
                    _ = stop_rx.changed() => return,
                }
            }
        }
    }
}
