use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::{stream, StreamExt};
use rua_channel::core::console::{
    submit_runtime_turn, ConsoleChatFacade, ConsoleChatRequest, ConsoleSubmitOutcome,
    SseOutputStream,
};
use rua_core::ChatAddress;
use tokio::sync::{mpsc, Notify};

struct Authority {
    admission_started: Notify,
    accept: Notify,
    cancelled: mpsc::UnboundedSender<(ChatAddress, String, String, Option<String>)>,
}

#[async_trait]
impl ConsoleChatFacade for Authority {
    async fn connect_scoped_chat_activity(
        &self,
        _: ChatAddress,
        _: &str,
        _: &str,
        _: Option<rua_core::ConversationActivityPosition>,
    ) -> SseOutputStream {
        Box::pin(
            stream::iter([Ok(serde_json::json!({
                "type":"stream_replay", "events":[
                    {"type":"turn_terminal","turn_id":"other","status":"completed"},
                    {"type":"external_runtime_event","turn_id":"scheduled","event":{
                        "type":"text_delta","delta":"scheduled output","message_id":"msg"}}
                ]
            })
            .to_string())])
            .chain(stream::pending()),
        )
    }

    async fn submit_scoped_chat(
        &self,
        _: ChatAddress,
        _: &str,
        request: ConsoleChatRequest,
    ) -> ConsoleSubmitOutcome {
        assert_eq!(request.extra["turn_id"], "scheduled");
        self.admission_started.notify_one();
        self.accept.notified().await;
        ConsoleSubmitOutcome::queued("chat", Some("queued-input".into()))
    }

    async fn cancel_submitted_turn(
        &self,
        address: ChatAddress,
        agent: &str,
        turn: &str,
        input: Option<&str>,
    ) {
        self.cancelled
            .send((address, agent.into(), turn.into(), input.map(str::to_owned)))
            .unwrap();
    }

    async fn stop_chat(&self, _: &str, _: &str) -> bool {
        panic!("external cancellation must target its own receipt");
    }
}

fn request() -> ConsoleChatRequest {
    ConsoleChatRequest {
        message: "scheduled input".into(),
        chat_id: Some("chat".into()),
        model: None,
        is_reconnect: false,
        attachments: vec![],
        input: None,
        messages: None,
        channel_kind: None,
        extra: serde_json::Map::from_iter([("turn_id".into(), "scheduled".into())]),
    }
}

#[tokio::test]
async fn disconnect_withdraws_only_its_turn_even_while_authority_is_accepting_it() {
    for disconnect_during_admission in [false, true] {
        let (cancelled, mut cancellations) = mpsc::unbounded_channel();
        let authority = Arc::new(Authority {
            admission_started: Notify::new(),
            accept: Notify::new(),
            cancelled,
        });
        let mut submission = Box::pin(submit_runtime_turn(
            authority.clone(),
            ChatAddress::new("chat"),
            "current-agent",
            request(),
        ));
        tokio::select! {
            _ = authority.admission_started.notified() => {},
            _ = &mut submission => panic!("admission must wait for an authority receipt"),
        }
        if disconnect_during_admission {
            drop(submission);
            authority.accept.notify_one();
        } else {
            authority.accept.notify_one();
            let mut ongoing = submission.await.unwrap();
            let event = ongoing.next().await.unwrap().unwrap();
            assert!(
                matches!(event, rua_core::RuntimeEvent::TextDelta { delta, .. } if delta == "scheduled output")
            );
            drop(ongoing);
        }
        let cancelled = tokio::time::timeout(Duration::from_secs(2), cancellations.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            cancelled,
            (
                ChatAddress::new("chat"),
                "current-agent".into(),
                "scheduled".into(),
                Some("queued-input".into())
            )
        );
    }
}
