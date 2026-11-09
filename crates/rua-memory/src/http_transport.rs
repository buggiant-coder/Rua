use crate::EmbeddingTransport;
use async_trait::async_trait;

pub struct HttpEmbeddingTransport {
    client: reqwest::Client,
    url: String,
    api_key: Option<String>,
}

impl HttpEmbeddingTransport {
    pub fn new(base_url: &str, api_key: Option<String>) -> Self {
        let url = format!("{}/embeddings", base_url.trim_end_matches('/'));
        Self {
            client: rua_http::default_client_or_default("rua-memory"),
            url,
            api_key,
        }
    }
}

#[async_trait]
impl EmbeddingTransport for HttpEmbeddingTransport {
    async fn post_embedding(&self, body: &serde_json::Value) -> Result<serde_json::Value, String> {
        let mut req = self.client.post(&self.url).json(body);
        if let Some(key) = &self.api_key {
            req = req.bearer_auth(key);
        }
        let resp = req.send().await.map_err(|e| e.to_string())?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("embedding API returned {status}: {text}"));
        }
        resp.json::<serde_json::Value>()
            .await
            .map_err(|e| e.to_string())
    }
}
