//! DashScope's duplex ASR and CosyVoice PCM wire protocol.
use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use tokio_util::sync::CancellationToken;

use super::types::SipOptions;
use super::voice::AudioOutput;
use rua_core::{Result, RuaError};

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;
const TASK_TIMEOUT: Duration = Duration::from_secs(30);

pub(super) enum SpeechEvent {
    Started,
    Transcript(String),
}

fn failure(error: impl std::fmt::Display) -> RuaError {
    RuaError::Runtime {
        message: format!("DashScope speech: {error}"),
    }
}

async fn connect(options: &SipOptions) -> Result<Socket> {
    let mut request = options
        .speech_url()
        .into_client_request()
        .map_err(failure)?;
    let key = options.api_key();
    request.headers_mut().insert(
        "Authorization",
        format!("Bearer {key}").parse().map_err(failure)?,
    );
    let (socket, _) = tokio::time::timeout(TASK_TIMEOUT, tokio_tungstenite::connect_async(request))
        .await
        .map_err(failure)?
        .map_err(failure)?;
    Ok(socket)
}

async fn control(socket: &mut Socket, task: &str, action: &str, payload: Value) -> Result<()> {
    socket
        .send(Message::Text(
            json!({
                "header": { "action": action, "task_id": task, "streaming": "duplex" },
                "payload": payload,
            })
            .to_string()
            .into(),
        ))
        .await
        .map_err(failure)
}

async fn next_control(socket: &mut Socket, task: &str) -> Result<Value> {
    loop {
        match socket.next().await {
            Some(Ok(Message::Text(text))) => {
                let event: Value = serde_json::from_str(&text).map_err(failure)?;
                if event["header"]["task_id"]
                    .as_str()
                    .is_some_and(|id| id != task)
                {
                    continue;
                }
                if event["header"]["event"] == "task-failed" {
                    return Err(failure(
                        event["header"]["error_message"]
                            .as_str()
                            .unwrap_or("task failed"),
                    ));
                }
                return Ok(event);
            }
            Some(Ok(Message::Ping(data))) => {
                socket.send(Message::Pong(data)).await.map_err(failure)?
            }
            Some(Ok(Message::Close(_))) | None => return Err(failure("connection closed")),
            Some(Err(error)) => return Err(failure(error)),
            _ => {}
        }
    }
}

async fn wait_started(socket: &mut Socket, task: &str) -> Result<()> {
    tokio::time::timeout(TASK_TIMEOUT, async {
        loop {
            if next_control(socket, task).await?["header"]["event"] == "task-started" {
                return Ok(());
            }
        }
    })
    .await
    .map_err(failure)?
}

pub(super) async fn recognize(
    options: Arc<SipOptions>,
    mut input: mpsc::Receiver<Vec<i16>>,
    events: mpsc::Sender<SpeechEvent>,
    cancel: CancellationToken,
) -> Result<()> {
    let run = async {
        let mut socket = connect(&options).await?;
        let task = uuid::Uuid::new_v4().simple().to_string();
        control(
            &mut socket,
            &task,
            "run-task",
            json!({
                "task_group": "audio", "task": "asr", "function": "recognition",
                "model": "paraformer-realtime-v2",
                "parameters": {
                    "sample_rate": 16000, "format": "pcm", "language_hints": ["zh", "en"],
                    "disfluency_removal_enabled": false, "semantic_punctuation_enabled": false,
                    "max_sentence_silence": 800, "punctuation_prediction_enabled": true,
                    "heartbeat": false, "inverse_text_normalization_enabled": true,
                },
                "input": {},
            }),
        )
        .await?;
        wait_started(&mut socket, &task).await?;
        let mut speaking = false;
        loop {
            tokio::select! {
                audio = input.recv() => {
                    let Some(pcm) = audio else {
                        control(&mut socket, &task, "finish-task", json!({"input":{}})).await?;
                        break;
                    };
                    let data = pcm.into_iter().flat_map(i16::to_le_bytes).collect::<Vec<_>>();
                    socket.send(Message::Binary(data.into())).await.map_err(failure)?;
                }
                event = next_control(&mut socket, &task) => {
                    let event = event?;
                    if event["header"]["event"] == "task-finished" {
                        break;
                    }
                    if event["header"]["event"] == "result-generated" {
                        let sentence = &event["payload"]["output"]["sentence"];
                        let text = sentence["text"].as_str().unwrap_or_default().trim();
                        if !text.is_empty() {
                            if !speaking {
                                speaking = true;
                                let _ = events.send(SpeechEvent::Started).await;
                            }
                            if sentence["sentence_end"] == true {
                                speaking = false;
                                let _ = events.send(SpeechEvent::Transcript(text.to_owned())).await;
                            }
                        }
                    }
                }
            }
        }
        let _ = socket.close(None).await;
        Ok(())
    };
    tokio::select! { _ = cancel.cancelled() => Ok(()), result = run => result }
}

pub(super) async fn speak(
    options: &SipOptions,
    text: &str,
    output: Arc<dyn AudioOutput>,
    sample_rate: u32,
    cancel: &CancellationToken,
) -> Result<()> {
    let run = async {
        let mut socket = connect(options).await?;
        let task = uuid::Uuid::new_v4().simple().to_string();
        control(&mut socket, &task, "run-task", json!({
            "task_group": "audio", "task": "tts", "function": "SpeechSynthesizer",
            "model": "cosyvoice-v1",
            "parameters": {
                "text_type": "PlainText", "voice": if options.tts_voice.is_empty() { "longxiaochun" } else { &options.tts_voice },
                "format": "pcm", "sample_rate": sample_rate, "volume": 50,
                "rate": 1, "pitch": 1,
            }, "input": {},
        })).await?;
        wait_started(&mut socket, &task).await?;
        control(
            &mut socket,
            &task,
            "continue-task",
            json!({"input":{"text":text}}),
        )
        .await?;
        control(&mut socket, &task, "finish-task", json!({"input":{}})).await?;
        let mut pending_byte = None;
        loop {
            match tokio::time::timeout(TASK_TIMEOUT, socket.next())
                .await
                .map_err(failure)?
            {
                Some(Ok(Message::Binary(data))) => {
                    let mut bytes = Vec::with_capacity(data.len() + 1);
                    if let Some(byte) = pending_byte.take() {
                        bytes.push(byte);
                    }
                    bytes.extend_from_slice(&data);
                    if bytes.len() % 2 == 1 {
                        pending_byte = bytes.pop();
                    }
                    let samples = bytes
                        .chunks_exact(2)
                        .map(|p| i16::from_le_bytes([p[0], p[1]]))
                        .collect::<Vec<_>>();
                    for frame in samples.chunks((sample_rate / 50) as usize) {
                        output.write(frame, sample_rate).await?;
                    }
                }
                Some(Ok(Message::Text(text))) => {
                    let event: Value = serde_json::from_str(&text).map_err(failure)?;
                    match event["header"]["event"].as_str() {
                        Some("task-finished") => break,
                        Some("task-failed") => {
                            return Err(failure(
                                event["header"]["error_message"]
                                    .as_str()
                                    .unwrap_or("TTS failed"),
                            ))
                        }
                        _ => {}
                    }
                }
                Some(Ok(Message::Ping(data))) => {
                    socket.send(Message::Pong(data)).await.map_err(failure)?
                }
                Some(Err(error)) => return Err(failure(error)),
                None | Some(Ok(Message::Close(_))) => {
                    return Err(failure("TTS closed before completion"))
                }
                _ => {}
            }
        }
        let _ = socket.close(None).await;
        Ok(())
    };
    tokio::select! { _ = cancel.cancelled() => Ok(()), result = run => result }
}
