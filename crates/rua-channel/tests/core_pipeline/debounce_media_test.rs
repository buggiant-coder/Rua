use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::stream;
use futures::stream::BoxStream;
use tokio::sync::Mutex;

use rua_channel::core::channel::Process;
use rua_channel::core::content::{
    AgentChannelRequest, ChannelTarget, IncomingContentPart, OutgoingMessage,
};
use rua_channel::core::debouncer::{content_allows_immediate, BufferDebouncer};
use rua_channel::core::pipeline;
use rua_channel::core::renderer::{MessageRenderer, RenderStyle};
use rua_core::RuntimeEvent;

#[test]
fn empty_input_returns_false() {
    assert!(!content_allows_immediate(&[]));
}

#[test]
fn pure_text_returns_false() {
    let parts = vec![IncomingContentPart::Text {
        text: "hello".into(),
    }];
    assert!(!content_allows_immediate(&parts));
}

#[test]
fn whitespace_only_text_returns_true() {
    let parts = vec![IncomingContentPart::Text { text: "   ".into() }];
    assert!(content_allows_immediate(&parts));
}

#[test]
fn pure_image_returns_true() {
    let parts = vec![IncomingContentPart::Image {
        file_id: "img-1".into(),
        path: PathBuf::from("/tmp/photo.jpg"),
        mime: Some("image/jpeg".into()),
    }];
    assert!(content_allows_immediate(&parts));
}

#[test]
fn pure_file_returns_true() {
    let parts = vec![IncomingContentPart::File {
        file_id: "file-1".into(),
        path: PathBuf::from("/tmp/doc.pdf"),
        mime: Some("application/pdf".into()),
        name: Some("doc.pdf".into()),
    }];
    assert!(content_allows_immediate(&parts));
}

#[test]
fn pure_url_returns_true() {
    let parts = vec![IncomingContentPart::Url {
        url: "https://example.com".into(),
        title: None,
    }];
    assert!(content_allows_immediate(&parts));
}

#[test]
fn pure_audio_returns_true() {
    let parts = vec![IncomingContentPart::Audio {
        file_id: "aud-1".into(),
        path: PathBuf::from("/tmp/voice.ogg"),
        mime: Some("audio/ogg".into()),
        name: Some("voice.ogg".into()),
    }];
    assert!(content_allows_immediate(&parts));
}

#[test]
fn pure_video_returns_true() {
    let parts = vec![IncomingContentPart::Video {
        file_id: "vid-1".into(),
        path: PathBuf::from("/tmp/clip.mp4"),
        mime: Some("video/mp4".into()),
        name: Some("clip.mp4".into()),
    }];
    assert!(content_allows_immediate(&parts));
}

#[test]
fn image_with_text_returns_false() {
    let parts = vec![
        IncomingContentPart::Image {
            file_id: "img-1".into(),
            path: PathBuf::from("/tmp/photo.jpg"),
            mime: None,
        },
        IncomingContentPart::Text {
            text: "look at this".into(),
        },
    ];
    assert!(!content_allows_immediate(&parts));
}

#[test]
fn multiple_media_no_text_returns_true() {
    let parts = vec![
        IncomingContentPart::Image {
            file_id: "img-1".into(),
            path: PathBuf::from("/tmp/a.jpg"),
            mime: None,
        },
        IncomingContentPart::File {
            file_id: "file-1".into(),
            path: PathBuf::from("/tmp/b.pdf"),
            mime: None,
            name: None,
        },
        IncomingContentPart::Url {
            url: "https://example.com".into(),
            title: Some("Example".into()),
        },
    ];
    assert!(content_allows_immediate(&parts));
}

struct EchoProcess;

#[async_trait]
impl Process for EchoProcess {
    async fn handle(&self, _req: AgentChannelRequest) -> BoxStream<'static, RuntimeEvent> {
        Box::pin(stream::iter(vec![
            RuntimeEvent::TextDelta {
                delta: "ack".into(),
                message_id: None,
            },
            RuntimeEvent::End {
                finish_reason: Some("stop".into()),
            },
        ]))
    }
}

fn make_request(input: Vec<IncomingContentPart>) -> AgentChannelRequest {
    AgentChannelRequest {
        agent_id: "agent-1".into(),
        user_id: "user-1".into(),
        session_id: "session-1".into(),
        input,
        channel_id: "test".into(),
        conversation_id: "conv-1".into(),
        message_id: None,
        metadata: serde_json::json!({}),
    }
}

type SentMessages = Arc<Mutex<Vec<(ChannelTarget, OutgoingMessage)>>>;

fn make_send_fn() -> (pipeline::SendFn, SentMessages) {
    let sent: Arc<Mutex<Vec<(ChannelTarget, OutgoingMessage)>>> = Arc::new(Mutex::new(Vec::new()));
    let sent_clone = sent.clone();
    let send_fn: pipeline::SendFn = Arc::new(move |to, msg| {
        let store = sent_clone.clone();
        Box::pin(async move {
            store.lock().await.push((to, msg));
            Ok(())
        })
    });
    (send_fn, sent)
}

#[tokio::test]
async fn pure_image_bypasses_debounce_window() {
    let process = Arc::new(EchoProcess);
    let debouncer = BufferDebouncer::new(Duration::from_secs(10));
    let renderer = Arc::new(MessageRenderer::new(RenderStyle::Markdown));
    let (send_fn, sent) = make_send_fn();

    let req = make_request(vec![IncomingContentPart::Image {
        file_id: "img-1".into(),
        path: PathBuf::from("/tmp/photo.jpg"),
        mime: Some("image/jpeg".into()),
    }]);

    let start = tokio::time::Instant::now();
    pipeline::run_inbound(
        process,
        req,
        Some((debouncer, "conv-1".into())),
        renderer,
        send_fn,
    )
    .await
    .unwrap();
    let elapsed = start.elapsed();

    assert!(
        elapsed < Duration::from_secs(2),
        "pure image should bypass debounce, but took {:?}",
        elapsed
    );
    let messages = sent.lock().await;
    assert_eq!(messages.len(), 1, "should have sent a reply");
}

#[tokio::test]
async fn pure_text_goes_through_debounce_window() {
    let process = Arc::new(EchoProcess);
    let debouncer = BufferDebouncer::new(Duration::from_millis(200));
    let renderer = Arc::new(MessageRenderer::new(RenderStyle::Markdown));
    let (send_fn, sent) = make_send_fn();

    let req = make_request(vec![IncomingContentPart::Text {
        text: "hello".into(),
    }]);

    let start = tokio::time::Instant::now();
    pipeline::run_inbound(
        process,
        req,
        Some((debouncer, "conv-1".into())),
        renderer,
        send_fn,
    )
    .await
    .unwrap();
    let elapsed = start.elapsed();

    assert!(
        elapsed >= Duration::from_millis(180),
        "pure text should wait for debounce window, but finished in {:?}",
        elapsed
    );
    let messages = sent.lock().await;
    assert_eq!(messages.len(), 1, "should have sent a reply after window");
}
