//! LiveKit's Rust SDK handles RTC signaling, ICE, Opus and audio resampling.
use std::borrow::Cow;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use livekit::options::TrackPublishOptions;
use livekit::participant::ParticipantKind;
use livekit::prelude::*;
use livekit::webrtc::audio_frame::AudioFrame;
use livekit::webrtc::audio_source::{
    native::NativeAudioSource, AudioSourceOptions, RtcAudioSource,
};
use livekit::webrtc::audio_stream::native::NativeAudioStream;
use livekit_token::{AccessToken, VideoGrants};
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use super::realtime::Calls;
use super::types::SipOptions;
use super::voice::{self, AudioOutput, CallControl, VoiceCall};
use crate::core::channel::Process;
use rua_core::{Result, RuaError};

fn failure(error: impl std::fmt::Display) -> RuaError {
    RuaError::Runtime {
        message: format!("LiveKit SIP: {error}"),
    }
}

fn room_token(options: &SipOptions, admin: bool) -> Result<String> {
    AccessToken::with_api_key(&options.livekit_api_key, &options.livekit_api_secret)
        .with_identity("rua-sip-agent")
        .with_ttl(Duration::from_secs(3600))
        .with_grants(VideoGrants {
            room_join: !admin,
            room_admin: admin,
            room_create: admin,
            room: options.livekit_room_name.clone(),
            ..Default::default()
        })
        .to_jwt()
        .map_err(failure)
}

pub(super) async fn run(
    options: Arc<SipOptions>,
    process: Arc<dyn Process>,
    calls: Calls,
    cancel: CancellationToken,
) {
    let mut retry = 2;
    loop {
        let result = tokio::select! {
            _ = cancel.cancelled() => break,
            result = connect_room(&options) => result,
        };
        match result {
            Ok((room, events, source)) => {
                retry = 2;
                if let Err(error) = serve_room(
                    &room,
                    events,
                    source,
                    &options,
                    process.clone(),
                    calls.clone(),
                    cancel.clone(),
                )
                .await
                {
                    tracing::warn!(%error, "LiveKit call ended");
                }
                let _ = tokio::time::timeout(Duration::from_secs(10), room.close()).await;
                // Fixed-room routing follows QwenPaw: release the call room, then reconnect.
                if let Err(error) = delete_room(&options).await {
                    tracing::debug!(%error, "LiveKit room cleanup failed");
                }
            }
            Err(error) => tracing::warn!(%error, "LiveKit connection failed"),
        }
        if cancel.is_cancelled() {
            break;
        }
        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = tokio::time::sleep(Duration::from_secs(retry)) => {},
        }
        retry = (retry * 2).min(30);
    }
    for call in calls.lock().await.values() {
        call.cancel.cancel();
    }
    calls.lock().await.clear();
}

async fn connect_room(
    options: &SipOptions,
) -> Result<(Room, mpsc::UnboundedReceiver<RoomEvent>, NativeAudioSource)> {
    let token = room_token(options, false)?;
    let (room, events) = Room::connect(&options.livekit_url, &token, RoomOptions::default())
        .await
        .map_err(failure)?;
    let source = NativeAudioSource::new(
        AudioSourceOptions::default(),
        options.livekit_output_sample_rate,
        1,
        100,
    );
    let track =
        LocalAudioTrack::create_audio_track("rua-voice", RtcAudioSource::Native(source.clone()));
    if let Err(error) = room
        .local_participant()
        .publish_track(
            LocalTrack::Audio(track),
            TrackPublishOptions {
                source: TrackSource::Microphone,
                ..Default::default()
            },
        )
        .await
    {
        let _ = room.close().await;
        return Err(failure(error));
    }
    Ok((room, events, source))
}

struct LiveKitOutput(NativeAudioSource);

#[async_trait]
impl AudioOutput for LiveKitOutput {
    async fn write(&self, pcm: &[i16], sample_rate: u32) -> Result<()> {
        self.0
            .capture_frame(&AudioFrame {
                data: Cow::Borrowed(pcm),
                sample_rate,
                num_channels: 1,
                samples_per_channel: pcm.len() as u32,
            })
            .await
            .map_err(failure)
    }
    async fn clear(&self) {
        self.0.clear_buffer();
    }
}

async fn serve_room(
    room: &Room,
    mut events: mpsc::UnboundedReceiver<RoomEvent>,
    source: NativeAudioSource,
    options: &Arc<SipOptions>,
    process: Arc<dyn Process>,
    calls: Calls,
    cancel: CancellationToken,
) -> Result<()> {
    let call_cancel = cancel.child_token();
    let mut tasks = JoinSet::new();
    let mut caller_id: Option<String> = None;
    let mut audio_tx: Option<mpsc::Sender<Vec<i16>>> = None;
    let output: Arc<dyn AudioOutput> = Arc::new(LiveKitOutput(source));
    let mut subscribed = std::collections::HashSet::new();
    let mut incoming = room.remote_participants().into_values().collect::<Vec<_>>();
    loop {
        let event = if let Some(participant) = incoming.pop() {
            Some(RoomEvent::ParticipantConnected(participant))
        } else {
            tokio::select! {
                _ = cancel.cancelled() => break,
                result = tasks.join_next(), if !tasks.is_empty() => {
                    if let Some(Ok(Err(error))) = result { tracing::warn!(%error, "LiveKit call task failed"); }
                    break;
                }
                event = events.recv() => event,
            }
        };
        match event {
            Some(
                RoomEvent::ParticipantConnected(participant)
                | RoomEvent::ParticipantActive(participant),
            ) => {
                if caller_id.is_some()
                    || !matches!(
                        participant.kind(),
                        ParticipantKind::Sip | ParticipantKind::Standard
                    )
                {
                    continue;
                }
                let id = participant.identity().to_string();
                let caller = participant
                    .attributes()
                    .get("sip.phoneNumber")
                    .cloned()
                    .unwrap_or_else(|| id.clone());
                let (input_tx, input_rx) = mpsc::channel(150);
                let (out_tx, out_rx) = mpsc::channel(16);
                calls.lock().await.insert(
                    id.clone(),
                    CallControl {
                        cancel: call_cancel.clone(),
                        outbound: out_tx,
                    },
                );
                let call = VoiceCall {
                    id: id.clone(),
                    caller,
                    callee: options.livekit_room_name.clone(),
                    input: input_rx,
                    output: output.clone(),
                    output_rate: options.livekit_output_sample_rate,
                    outbound: out_rx,
                    cancel: call_cancel.clone(),
                };
                tasks.spawn(voice::run_call(call, options.clone(), process.clone()));
                caller_id = Some(id);
                audio_tx = Some(input_tx);
                // Tracks may already be subscribed when reconnecting to an occupied room.
                for publication in participant.track_publications().into_values() {
                    if let Some(RemoteTrack::Audio(track)) = publication.track() {
                        if !subscribed.insert(track.sid().to_string()) {
                            continue;
                        }
                        spawn_audio(
                            &mut tasks,
                            track,
                            audio_tx.as_ref().expect("audio sender").clone(),
                            call_cancel.child_token(),
                        );
                    }
                }
            }
            Some(RoomEvent::TrackSubscribed {
                track: RemoteTrack::Audio(track),
                participant,
                ..
            }) if caller_id.as_deref() == Some(participant.identity().as_str()) => {
                if let Some(input) = &audio_tx {
                    if !subscribed.insert(track.sid().to_string()) {
                        continue;
                    }
                    spawn_audio(&mut tasks, track, input.clone(), call_cancel.child_token());
                }
            }
            Some(RoomEvent::ParticipantDisconnected(participant))
                if caller_id.as_deref() == Some(participant.identity().as_str()) =>
            {
                break
            }
            Some(RoomEvent::Disconnected { .. }) | None => break,
            _ => {}
        }
    }
    call_cancel.cancel();
    while tasks.join_next().await.is_some() {}
    output.clear().await;
    if let Some(id) = caller_id {
        calls.lock().await.remove(&id);
    }
    Ok(())
}

fn spawn_audio(
    tasks: &mut JoinSet<Result<()>>,
    track: RemoteAudioTrack,
    input: mpsc::Sender<Vec<i16>>,
    cancel: CancellationToken,
) {
    tasks.spawn(async move {
        // SDK resamples all RTC formats to the ASR's mono 16 kHz PCM.
        let mut audio = NativeAudioStream::new(track.rtc_track(), 16000, 1);
        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                frame = audio.next() => {
                    let Some(frame) = frame else { break; };
                    let _ = input.try_send(frame.data.into_owned());
                }
            }
        }
        Ok(())
    });
}

async fn delete_room(options: &SipOptions) -> Result<()> {
    let base = options
        .livekit_url
        .replacen("wss://", "https://", 1)
        .replacen("ws://", "http://", 1);
    crate::http_client::default_http_client()
        .post(format!(
            "{}/twirp/livekit.RoomService/DeleteRoom",
            base.trim_end_matches('/')
        ))
        .bearer_auth(room_token(options, true)?)
        .json(&serde_json::json!({"room":options.livekit_room_name}))
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(failure)?
        .error_for_status()
        .map_err(failure)?;
    Ok(())
}
