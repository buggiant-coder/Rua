use async_trait::async_trait;
use futures::stream::{self, BoxStream};
use rua_channel::core::channel::{AgentRequest, Process};
use rua_core::RuntimeEvent;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};

pub struct ReplyProcess {
    received: mpsc::UnboundedSender<AgentRequest>,
    text: String,
}

#[async_trait]
impl Process for ReplyProcess {
    async fn handle(&self, request: AgentRequest) -> BoxStream<'static, RuntimeEvent> {
        self.received.send(request).unwrap();
        Box::pin(stream::iter(vec![
            RuntimeEvent::TextDelta {
                delta: self.text.clone(),
                message_id: None,
            },
            RuntimeEvent::End {
                finish_reason: None,
            },
        ]))
    }
}

pub fn reply_process(text: &str) -> (Arc<dyn Process>, mpsc::UnboundedReceiver<AgentRequest>) {
    let (tx, rx) = mpsc::unbounded_channel();
    (
        Arc::new(ReplyProcess {
            received: tx,
            text: text.into(),
        }),
        rx,
    )
}

pub async fn recv<T>(rx: &mut mpsc::UnboundedReceiver<T>) -> T {
    tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
        .await
        .unwrap()
        .unwrap()
}

pub async fn wait_for_len<T>(values: &Mutex<Vec<T>>, count: usize) {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if values.lock().await.len() >= count {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

#[cfg(any(feature = "slack", feature = "discord"))]
pub async fn http_proxy(
    upstream_addr: std::net::SocketAddr,
) -> (String, Arc<Mutex<Vec<String>>>, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let captured = requests.clone();
    let task = tokio::spawn(async move {
        let mut connections = tokio::task::JoinSet::new();
        loop {
            let (mut incoming, _) = listener.accept().await.unwrap();
            let captured = captured.clone();
            connections.spawn(async move {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut headers = Vec::new();
                while !headers.ends_with(b"\r\n\r\n") {
                    let Ok(byte) = incoming.read_u8().await else {
                        return;
                    };
                    headers.push(byte);
                    assert!(headers.len() < 16384);
                }
                captured
                    .lock()
                    .await
                    .push(String::from_utf8_lossy(&headers).to_string());
                let mut upstream = tokio::net::TcpStream::connect(upstream_addr).await.unwrap();
                upstream.write_all(&headers).await.unwrap();
                let _ = tokio::io::copy_bidirectional(&mut incoming, &mut upstream).await;
            });
        }
    });
    (url, requests, task)
}
