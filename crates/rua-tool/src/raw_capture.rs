use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};

#[derive(Debug)]
pub struct RawCaptureBuffer {
    max_bytes: usize,
    head_budget: usize,
    tail_budget: usize,
    head: Vec<u8>,
    tail: VecDeque<u8>,
    total_bytes: usize,
}

#[derive(Debug, Default)]
pub struct CapturedRawStream {
    pub retained: Vec<u8>,
    pub path: Option<PathBuf>,
    pub total_bytes: usize,
    pub omitted_bytes: usize,
}

#[async_trait::async_trait]
pub trait RawCaptureObserver: Send + Sync {
    async fn appended(&self, source: &str, bytes: &[u8]);
    async fn sealed(&self, source: &str);
}

pub async fn capture_stream<R>(mut reader: R, max_bytes: usize, path: PathBuf) -> CapturedRawStream
where
    R: AsyncRead + Unpin,
{
    capture_stream_with_observer(&mut reader, max_bytes, path, None, "").await
}

pub async fn capture_stream_with_observer<R>(
    mut reader: R,
    max_bytes: usize,
    path: PathBuf,
    observer: Option<Arc<dyn RawCaptureObserver>>,
    source: &'static str,
) -> CapturedRawStream
where
    R: AsyncRead + Unpin,
{
    let mut file = if let Some(parent) = path.parent() {
        if tokio::fs::create_dir_all(parent).await.is_ok() {
            tokio::fs::File::create(&path).await.ok()
        } else {
            None
        }
    } else {
        None
    };
    let mut capture = RawCaptureBuffer::new(max_bytes);
    let mut chunk = vec![0_u8; 8192];
    loop {
        let count = match reader.read(&mut chunk).await {
            Ok(0) => break,
            Err(error) => {
                tracing::warn!(%error, "raw output stream read failed; capture is incomplete");
                file = None;
                break;
            }
            Ok(count) => count,
        };
        if let Some(writer) = file.as_mut() {
            let saved = match writer.write_all(&chunk[..count]).await {
                Ok(()) => writer.flush().await,
                Err(error) => Err(error),
            };
            if let Err(error) = saved {
                tracing::warn!(%error, "raw output capture write failed; archive is unavailable");
                file = None;
            }
        }
        capture.push(&chunk[..count]);
        if let Some(observer) = observer.as_ref() {
            observer.appended(source, &chunk[..count]).await;
        }
    }
    if let Some(writer) = file.as_mut() {
        if let Err(error) = writer.sync_all().await {
            tracing::warn!(%error, "raw output capture sync failed; archive is unavailable");
            file = None;
        }
    }
    if let Some(observer) = observer.as_ref() {
        observer.sealed(source).await;
    }
    CapturedRawStream {
        retained: capture.to_bytes_with_marker(),
        path: file.map(|_| path),
        total_bytes: capture.total_bytes(),
        omitted_bytes: capture.omitted_bytes(),
    }
}

impl RawCaptureBuffer {
    pub fn new(max_bytes: usize) -> Self {
        let head_budget = max_bytes / 2;
        Self {
            max_bytes,
            head_budget,
            tail_budget: max_bytes.saturating_sub(head_budget),
            head: Vec::new(),
            tail: VecDeque::new(),
            total_bytes: 0,
        }
    }

    pub fn push(&mut self, chunk: &[u8]) {
        self.total_bytes = self.total_bytes.saturating_add(chunk.len());
        if chunk.is_empty() || self.max_bytes == 0 {
            return;
        }
        let head_remaining = self.head_budget.saturating_sub(self.head.len());
        let head_len = head_remaining.min(chunk.len());
        self.head.extend_from_slice(&chunk[..head_len]);
        let tail_chunk = &chunk[head_len..];
        if self.tail_budget == 0 || tail_chunk.is_empty() {
            return;
        }
        if tail_chunk.len() >= self.tail_budget {
            self.tail.clear();
            self.tail
                .extend(&tail_chunk[tail_chunk.len() - self.tail_budget..]);
            return;
        }
        self.tail.extend(tail_chunk);
        let excess = self.tail.len().saturating_sub(self.tail_budget);
        if excess > 0 {
            self.tail.drain(..excess);
        }
    }

    pub fn retained_bytes(&self) -> usize {
        self.head.len().saturating_add(self.tail.len())
    }

    pub fn total_bytes(&self) -> usize {
        self.total_bytes
    }

    pub fn omitted_bytes(&self) -> usize {
        self.total_bytes.saturating_sub(self.retained_bytes())
    }

    pub fn to_bytes_with_marker(&self) -> Vec<u8> {
        let omitted = self.omitted_bytes();
        if omitted == 0 {
            let mut bytes = self.head.clone();
            bytes.extend(self.tail.iter().copied());
            return bytes;
        }
        let marker = format!("\n... [{omitted} raw bytes omitted] ...\n");
        let mut bytes = Vec::with_capacity(self.retained_bytes().saturating_add(marker.len()));
        bytes.extend_from_slice(&self.head);
        bytes.extend_from_slice(marker.as_bytes());
        bytes.extend(self.tail.iter().copied());
        bytes
    }
}
