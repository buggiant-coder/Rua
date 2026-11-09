use std::sync::Arc;

use async_trait::async_trait;

use crate::error::{MemoryError, MemoryResult};

#[async_trait]
pub trait Embedder: Send + Sync {
    async fn embed(&self, texts: &[String]) -> MemoryResult<Vec<Vec<f32>>>;

    fn dimension(&self) -> usize;

    fn model_id(&self) -> &str;
}

pub struct NullEmbedder;

#[async_trait]
impl Embedder for NullEmbedder {
    async fn embed(&self, texts: &[String]) -> MemoryResult<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|_| vec![]).collect())
    }

    fn dimension(&self) -> usize {
        0
    }

    fn model_id(&self) -> &str {
        "null"
    }
}

pub struct MockEmbedder {
    dimension: usize,
}

impl MockEmbedder {
    pub fn new(dimension: usize) -> Self {
        Self { dimension }
    }
}

#[async_trait]
impl Embedder for MockEmbedder {
    async fn embed(&self, texts: &[String]) -> MemoryResult<Vec<Vec<f32>>> {
        Ok(texts
            .iter()
            .map(|text| {
                let mut vec = vec![0.0f32; self.dimension];
                for (i, byte) in text.bytes().enumerate() {
                    vec[i % self.dimension] += (byte as f32) / 255.0;
                }

                let norm: f32 = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
                if norm > 0.0 {
                    for v in &mut vec {
                        *v /= norm;
                    }
                }
                vec
            })
            .collect())
    }

    fn dimension(&self) -> usize {
        self.dimension
    }

    fn model_id(&self) -> &str {
        "mock"
    }
}

#[async_trait]
pub trait EmbeddingTransport: Send + Sync {
    async fn post_embedding(&self, body: &serde_json::Value) -> Result<serde_json::Value, String>;
}

pub struct LlmEmbedder {
    transport: Arc<dyn EmbeddingTransport>,
    model: String,
    index_identity: String,
    dim: usize,
}

impl LlmEmbedder {
    pub fn new(
        transport: Arc<dyn EmbeddingTransport>,
        model: impl Into<String>,
        dimension: usize,
    ) -> Self {
        let model = model.into();
        Self {
            transport,
            index_identity: model.clone(),
            model,
            dim: dimension,
        }
    }

    pub fn with_backend_identity(mut self, backend: &str) -> Self {
        self.index_identity = format!("{backend}:{}", self.model);
        self
    }
}

#[async_trait]
impl Embedder for LlmEmbedder {
    async fn embed(&self, texts: &[String]) -> MemoryResult<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(vec![]);
        }

        let body = serde_json::json!({
            "model": self.model,
            "input": texts,
        });

        let response = self
            .transport
            .post_embedding(&body)
            .await
            .map_err(MemoryError::Embedding)?;

        let data = response
            .get("data")
            .and_then(|d| d.as_array())
            .ok_or_else(|| {
                MemoryError::Embedding("missing 'data' array in embedding response".into())
            })?;

        let mut embeddings = Vec::with_capacity(texts.len());
        for (i, item) in data.iter().enumerate() {
            let arr = item
                .get("embedding")
                .and_then(|e| e.as_array())
                .ok_or_else(|| {
                    MemoryError::Embedding(format!("missing 'embedding' for item {i}"))
                })?;

            let vec: Vec<f32> = arr
                .iter()
                .filter_map(|v| v.as_f64().map(|f| f as f32))
                .collect();
            embeddings.push(vec);
        }

        Ok(embeddings)
    }

    fn dimension(&self) -> usize {
        self.dim
    }

    fn model_id(&self) -> &str {
        &self.index_identity
    }
}
