//! Shared live call turn handling for SIP/RTP and LiveKit.
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::core::channel::{AgentRequest, Process};
use crate::core::content::IncomingContentPart;
use rua_core::{Result, RuaError, RuntimeEvent};

use super::speech::{self, SpeechEvent};
use super::types::SipOptions;

/// PCM output is paced by the transport. Clearing it implements barge-in.
#[async_trait]
pub(super) trait AudioOutput: Send + Sync {
    async fn write(&self, pcm: &[i16], sample_rate: u32) -> Result<()>;
    async fn clear(&self);
}

#[derive(Clone)]
pub(super) struct CallControl {
    pub cancel: CancellationToken,
    pub outbound: mpsc::Sender<String>,
}

pub(super) struct VoiceCall {
    pub id: String,
    pub caller: String,
    pub callee: String,
    pub input: mpsc::Receiver<Vec<i16>>,
    pub output: Arc<dyn AudioOutput>,
    pub output_rate: u32,
    pub outbound: mpsc::Receiver<String>,
    pub cancel: CancellationToken,
}

pub(super) async fn run_call(
    call: VoiceCall,
    options: Arc<SipOptions>,
    process: Arc<dyn Process>,
) -> Result<()> {
    let VoiceCall {
        id,
        caller,
        callee,
        input,
        output,
        output_rate,
        mut outbound,
        cancel,
    } = call;
    let (events_tx, mut events) = mpsc::channel(32);
    let mut workers = JoinSet::new();
    let asr_cancel = cancel.child_token();
    let asr_options = options.clone();
    workers
        .spawn(async move { speech::recognize(asr_options, input, events_tx, asr_cancel).await });

    let mut turn = JoinSet::new();
    if !options.welcome_greeting.is_empty() {
        spawn_speech(
            &mut turn,
            options.clone(),
            output.clone(),
            output_rate,
            options.welcome_greeting.clone(),
            cancel.child_token(),
        );
    }
    let timeout = if options.call_timeout > 0.0 {
        Duration::from_secs_f64(options.call_timeout)
    } else {
        Duration::from_secs(365 * 24 * 3600)
    };
    let deadline = tokio::time::sleep(timeout);
    tokio::pin!(deadline);
    let result = loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => break Ok(()),
            _ = &mut deadline => break Ok(()),
            result = workers.join_next(), if !workers.is_empty() => {
                break match result {
                    Some(Ok(result)) => result,
                    Some(Err(error)) => Err(RuaError::Runtime { message: format!("SIP speech task failed: {error}") }),
                    None => Ok(()),
                };
            }
            result = turn.join_next(), if !turn.is_empty() => {
                if let Some(Ok(Err(error))) = result {
                    tracing::warn!(%error, call_id = %id, "SIP reply failed");
                }
            }
            Some(text) = outbound.recv() => {
                turn.abort_all();
                while turn.join_next().await.is_some() {}
                output.clear().await;
                spawn_speech(&mut turn, options.clone(), output.clone(), output_rate, text, cancel.child_token());
            }
            event = events.recv() => {
                match event {
                    Some(SpeechEvent::Started) => {
                        turn.abort_all();
                        while turn.join_next().await.is_some() {}
                        output.clear().await;
                    }
                    Some(SpeechEvent::Transcript(text)) => {
                        deadline.as_mut().reset(tokio::time::Instant::now() + timeout);
                        turn.abort_all();
                        while turn.join_next().await.is_some() {}
                        output.clear().await;
                        let req = AgentRequest {
                            agent_id: String::new(),
                            user_id: caller.clone(),
                            session_id: format!("sip_call:{id}"),
                            input: vec![IncomingContentPart::Text { text }],
                            channel_id: "sip".into(),
                            conversation_id: id.clone(),
                            message_id: Some(uuid::Uuid::new_v4().to_string()),
                            metadata: serde_json::json!({
                                "call_id": id, "caller": caller, "callee": callee,
                                "is_dm": true, "is_group": false,
                            }),
                        };
                        let process = process.clone();
                        let output = output.clone();
                        let options = options.clone();
                        let turn_cancel = cancel.child_token();
                        turn.spawn(async move {
                            tokio::select! {
                                _ = turn_cancel.cancelled() => Ok(()),
                                result = async {
                                    let mut stream = process.handle(req).await;
                                    let mut reply = String::new();
                                    while let Some(event) = stream.next().await {
                                        match event {
                                            RuntimeEvent::TextDelta { delta, .. } => reply.push_str(&delta),
                                            RuntimeEvent::Error { error } => {
                                                return Err(RuaError::Runtime { message: error.to_string() });
                                            }
                                            _ => {}
                                        }
                                    }
                                    if !reply.trim().is_empty() && reply.trim() != "NO_REPLY" {
                                        speech::speak(&options, &reply, output, output_rate, &turn_cancel).await?;
                                    }
                                    Ok(())
                                } => result,
                            }
                        });
                    }
                    None => break Ok(()),
                }
            }
        }
    };
    turn.abort_all();
    while turn.join_next().await.is_some() {}
    workers.abort_all();
    while workers.join_next().await.is_some() {}
    output.clear().await;
    result
}

fn spawn_speech(
    tasks: &mut JoinSet<Result<()>>,
    options: Arc<SipOptions>,
    output: Arc<dyn AudioOutput>,
    sample_rate: u32,
    text: String,
    cancel: CancellationToken,
) {
    tasks.spawn(async move { speech::speak(&options, &text, output, sample_rate, &cancel).await });
}
