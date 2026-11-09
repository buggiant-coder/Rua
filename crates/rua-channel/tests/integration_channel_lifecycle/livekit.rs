use std::borrow::Cow;
use std::time::Duration;

use futures::StreamExt;
use livekit::options::TrackPublishOptions;
use livekit::prelude::*;
use livekit::webrtc::audio_frame::AudioFrame;
use livekit::webrtc::audio_source::{
    native::NativeAudioSource, AudioSourceOptions, RtcAudioSource,
};
use livekit::webrtc::audio_stream::native::NativeAudioStream;
use livekit_token::{AccessToken, VideoGrants};
use rua_channel::core::channel::Channel;
use rua_channel::sip::{SipChannel, SipConfig, SipOptions};
use serde_json::json;

use super::sip::{speech_server, transcript};
use super::support::{recv, reply_process};

/// Run with a local LiveKit dev server; no telephony or paid speech service.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires RUA_TEST_LIVEKIT_URL pointing to a local dev server (devkey/secret)"]
async fn livekit_rtc_audio_roundtrip_and_room_reconnect() {
    let url = std::env::var("RUA_TEST_LIVEKIT_URL").expect("local LiveKit URL");
    let room_name = format!("rua-test-{}", uuid::Uuid::new_v4());
    let (speech, speech_url, speech_server) = speech_server().await;
    let channel = SipChannel::new(SipConfig::default()).with_options(SipOptions {
        sip_mode: "livekit".into(),
        livekit_url: url.clone(),
        livekit_api_key: "devkey".into(),
        livekit_api_secret: "secret".into(),
        livekit_room_name: room_name.clone(),
        dashscope_api_key: "test-dashscope-key".into(),
        welcome_greeting: String::new(),
        call_timeout: 30.0,
        extra: std::collections::BTreeMap::from([("dashscope_ws_url".into(), json!(speech_url))]),
        ..Default::default()
    });
    let (process, mut requests) = reply_process("LiveKit audio response");
    channel.start(process).await.unwrap();

    for call in 0..2 {
        *speech.asr.lock().await = None;
        let prior_audio = speech.pcm.lock().await.len();
        let token = AccessToken::with_api_key("devkey", "secret")
            .with_identity(&format!("caller-{call}"))
            .with_grants(VideoGrants {
                room_join: true,
                room: room_name.clone(),
                ..Default::default()
            })
            .to_jwt()
            .unwrap();
        let (room, mut events) = Room::connect(&url, &token, RoomOptions::default())
            .await
            .unwrap();
        let source = NativeAudioSource::new(AudioSourceOptions::default(), 48000, 1, 100);
        let track =
            LocalAudioTrack::create_audio_track("caller", RtcAudioSource::Native(source.clone()));
        room.local_participant()
            .publish_track(
                LocalTrack::Audio(track),
                TrackPublishOptions {
                    source: TrackSource::Microphone,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let remote = tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                if let Some(RoomEvent::TrackSubscribed {
                    track: RemoteTrack::Audio(track),
                    participant,
                    ..
                }) = events.recv().await
                {
                    if participant.identity().as_str() == "rua-sip-agent" {
                        break track;
                    }
                }
            }
        })
        .await
        .unwrap();
        // Send real Opus/RTC audio; the channel feeds decoded 16 kHz PCM to mock ASR.
        for frame in 0..80 {
            let pcm = (0..960)
                .map(|i| {
                    let angle = (frame * 960 + i) as f64 * 440.0 * std::f64::consts::TAU / 48000.0;
                    (angle.sin() * 12000.0) as i16
                })
                .collect::<Vec<_>>();
            source
                .capture_frame(&AudioFrame {
                    data: Cow::Owned(pcm),
                    sample_rate: 48000,
                    num_channels: 1,
                    samples_per_channel: 960,
                })
                .await
                .unwrap();
        }
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let audio = speech.pcm.lock().await;
                if audio.len() > prior_audio
                    && audio.iter().skip(prior_audio).any(|p| {
                        p.chunks_exact(2)
                            .any(|s| i16::from_le_bytes([s[0], s[1]]).unsigned_abs() > 100)
                    })
                {
                    break;
                }
                drop(audio);
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        let mut output = NativeAudioStream::new(remote.rtc_track(), 24000, 1);
        transcript(&speech, &format!("hello call {call}"), true).await;
        let request = recv(&mut requests).await;
        assert_eq!(request.conversation_id, format!("caller-{call}"));
        tokio::time::timeout(Duration::from_secs(10), async {
            while let Some(frame) = output.next().await {
                if frame.data.iter().any(|s| s.unsigned_abs() > 100) {
                    return;
                }
            }
            panic!("LiveKit audio stream closed");
        })
        .await
        .unwrap();
        room.close().await.unwrap();
        tokio::time::timeout(Duration::from_secs(10), async {
            while channel.active_call_count().await != 0 {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        // The fixed room is deleted and the bot reconnects after a call.
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
    tokio::time::timeout(Duration::from_secs(15), channel.stop())
        .await
        .unwrap()
        .unwrap();
    speech_server.abort();
}
