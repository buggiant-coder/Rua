use rua_voice::twilio::{
    append_token, build_multipart_body, is_terminal_status, render_connect_twiml,
    render_error_twiml, render_reject_twiml,
};

#[test]
fn render_connect_twiml_includes_both_language_attributes_when_configured() {
    let twiml = render_connect_twiml("/ws?token=abc", Some("zh-CN"), Some("zh-CN"));

    assert!(twiml.starts_with(r#"<?xml version="1.0" encoding="UTF-8"?>"#));
    assert!(twiml.contains("<ConversationRelay "));
    assert!(twiml.contains(r#"dtmfDetection="true""#));
    assert!(twiml.contains(r#"interruptible="true""#));
    assert!(twiml.contains(r#"ttsLanguage="zh-CN""#));
    assert!(twiml.contains(r#"speechLanguage="zh-CN""#));
}

#[test]
fn render_connect_twiml_omits_language_attributes_when_unset() {
    let twiml = render_connect_twiml("/ws?token=abc", None, None);

    assert!(twiml.contains(r#"url="/ws?token=abc""#));
    assert!(!twiml.contains("ttsLanguage"));
    assert!(!twiml.contains("speechLanguage"));
}

#[test]
fn render_connect_twiml_allows_speech_language_without_tts_language() {
    let twiml = render_connect_twiml("/ws?token=abc", None, Some("en-US"));

    assert!(!twiml.contains("ttsLanguage"));
    assert!(twiml.contains(r#"speechLanguage="en-US""#));
}

#[test]
fn reject_and_error_twiml_are_valid_xml_responses_for_twilio_fallbacks() {
    let reject = render_reject_twiml();
    assert!(reject.starts_with(r#"<?xml version="1.0" encoding="UTF-8"?>"#));
    assert!(reject.contains("Voice channel is not available."));
    assert!(reject.contains(r#"<Reject reason="rejected" />"#));

    let error = render_error_twiml();
    assert!(error.starts_with(r#"<?xml version="1.0" encoding="UTF-8"?>"#));
    assert!(error.contains("An error occurred. Please try again later."));
    assert!(error.contains("<Hangup />"));
}

#[test]
fn terminal_status_matches_twilio_call_lifecycle_end_states() {
    for status in ["completed", "failed", "busy", "no-answer", "canceled"] {
        assert!(is_terminal_status(status), "{status} should be terminal");
    }

    for status in ["queued", "ringing", "in-progress", "", "COMPLETED"] {
        assert!(
            !is_terminal_status(status),
            "{status:?} should not be terminal"
        );
    }
}

#[test]
fn append_token_uses_query_separator_that_matches_existing_url() {
    assert_eq!(
        append_token("wss://voice.example/ws", "abc"),
        "wss://voice.example/ws?token=abc"
    );
    assert_eq!(
        append_token("wss://voice.example/ws?call=CA123", "abc"),
        "wss://voice.example/ws?call=CA123&token=abc"
    );
}

#[test]
fn multipart_body_contains_model_file_part_mime_and_closing_boundary() {
    let (boundary, body) = build_multipart_body(b"audio-bytes", "clip.wav", "whisper-1");
    let body = String::from_utf8(body).expect("multipart body is utf8 around test bytes");

    assert!(boundary.starts_with("----RuaBoundary"));
    assert!(body.contains(&format!("--{boundary}\r\n")));
    assert!(body.contains("Content-Disposition: form-data; name=\"model\""));
    assert!(body.contains("whisper-1"));
    assert!(body.contains("Content-Disposition: form-data; name=\"file\"; filename=\"clip.wav\""));
    assert!(body.contains("Content-Type: audio/"));
    assert!(body.contains("audio-bytes"));
    assert!(body.ends_with(&format!("--{boundary}--\r\n")));
}

#[test]
fn twilio_signature_matches_reference_vector_and_rejects_tampering() {
    let params = std::collections::BTreeMap::from([
        ("CallSid".into(), "CA1234567890ABCDE".into()),
        ("Caller".into(), "+14158675310".into()),
        ("Digits".into(), "1234".into()),
        ("From".into(), "+14158675310".into()),
        ("To".into(), "+18005551212".into()),
    ]);
    assert!(rua_voice::twilio::validate_signature(
        "12345",
        "https://mycompany.com/myapp.php?foo=1&bar=2",
        &params,
        "GvWf1cFY/Q7PnoempGyD5oXAezc=",
    ));
    assert!(!rua_voice::twilio::validate_signature(
        "wrong",
        "https://mycompany.com/myapp.php?foo=1&bar=2",
        &params,
        "GvWf1cFY/Q7PnoempGyD5oXAezc=",
    ));
}

#[test]
fn twiml_escapes_url_query_and_language_attributes() {
    let xml = rua_voice::twilio::render_connect_twiml(
        "wss://example.test/ws?a=1&token=x",
        Some("en\"US"),
        None,
    );
    assert!(xml.contains("a=1&amp;token=x"));
    assert!(xml.contains("en&quot;US"));
    assert_eq!(
        rua_voice::twilio::public_ws_url("https://example.test/prefix/").unwrap(),
        "wss://example.test/prefix/voice/ws"
    );
}
