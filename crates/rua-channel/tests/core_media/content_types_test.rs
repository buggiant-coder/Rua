use std::path::PathBuf;

use rua_channel::core::content::{
    render_parts_to_string, IncomingContentPart, OutgoingContentPart,
};

#[test]
fn incoming_audio_video_have_stable_snake_case_json() {
    let audio = IncomingContentPart::Audio {
        file_id: "aud-1".into(),
        path: PathBuf::from("/tmp/voice.ogg"),
        mime: Some("audio/ogg".into()),
        name: Some("voice.ogg".into()),
    };
    let video = IncomingContentPart::Video {
        file_id: "vid-1".into(),
        path: PathBuf::from("/tmp/clip.mp4"),
        mime: Some("video/mp4".into()),
        name: Some("clip.mp4".into()),
    };

    assert_eq!(
        serde_json::to_value(&audio).expect("audio json"),
        serde_json::json!({
            "type": "audio",
            "file_id": "aud-1",
            "path": "/tmp/voice.ogg",
            "mime": "audio/ogg",
            "name": "voice.ogg"
        })
    );
    assert_eq!(
        serde_json::to_value(&video).expect("video json"),
        serde_json::json!({
            "type": "video",
            "file_id": "vid-1",
            "path": "/tmp/clip.mp4",
            "mime": "video/mp4",
            "name": "clip.mp4"
        })
    );
}

#[test]
fn outgoing_audio_video_have_stable_snake_case_json() {
    let audio = OutgoingContentPart::Audio {
        url: Some("file:///tmp/voice.ogg".into()),
        path: None,
        name: Some("voice.ogg".into()),
    };
    let video = OutgoingContentPart::Video {
        url: None,
        path: Some(PathBuf::from("clips/clip.mp4")),
        name: Some("clip.mp4".into()),
    };

    assert_eq!(
        serde_json::to_value(&audio).expect("audio json"),
        serde_json::json!({
            "type": "audio",
            "url": "file:///tmp/voice.ogg",
            "path": null,
            "name": "voice.ogg"
        })
    );
    assert_eq!(
        serde_json::to_value(&video).expect("video json"),
        serde_json::json!({
            "type": "video",
            "url": null,
            "path": "clips/clip.mp4",
            "name": "clip.mp4"
        })
    );
}

#[test]
fn render_parts_to_string_has_audio_video_fallbacks() {
    let rendered = render_parts_to_string(&[
        OutgoingContentPart::Audio {
            url: None,
            path: None,
            name: Some("voice.ogg".into()),
        },
        OutgoingContentPart::Video {
            url: None,
            path: None,
            name: Some("clip.mp4".into()),
        },
        OutgoingContentPart::Audio {
            url: None,
            path: None,
            name: None,
        },
        OutgoingContentPart::Video {
            url: None,
            path: None,
            name: None,
        },
    ]);

    assert_eq!(
        rendered,
        "[Audio: voice.ogg]\n[Video: clip.mp4]\n[Audio]\n[Video]"
    );
}
