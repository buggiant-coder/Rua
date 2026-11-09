pub fn render_connect_twiml(
    ws_url_with_token: &str,
    tts_language: Option<&str>,
    speech_language: Option<&str>,
) -> String {
    let mut attrs = format!(
        r#"url="{}" dtmfDetection="true" interruptible="true""#,
        xml_escape(ws_url_with_token)
    );
    if let Some(lang) = tts_language {
        attrs.push_str(&format!(r#" ttsLanguage="{}""#, xml_escape(lang)));
    }
    if let Some(lang) = speech_language {
        attrs.push_str(&format!(r#" speechLanguage="{}""#, xml_escape(lang)));
    }
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<Response>
  <Connect>
    <ConversationRelay {attrs} />
  </Connect>
</Response>"#
    )
}

pub fn render_reject_twiml() -> &'static str {
    r#"<?xml version="1.0" encoding="UTF-8"?>
<Response>
  <Say>Voice channel is not available.</Say>
  <Reject reason="rejected" />
</Response>"#
}

pub fn render_error_twiml() -> &'static str {
    r#"<?xml version="1.0" encoding="UTF-8"?>
<Response>
  <Say>An error occurred. Please try again later.</Say>
  <Hangup />
</Response>"#
}

pub fn is_terminal_status(status: &str) -> bool {
    matches!(
        status,
        "completed" | "failed" | "busy" | "no-answer" | "canceled"
    )
}

pub fn append_token(ws_url: &str, token: &str) -> String {
    if ws_url.contains('?') {
        format!("{ws_url}&token={token}")
    } else {
        format!("{ws_url}?token={token}")
    }
}

pub fn build_multipart_body(audio_bytes: &[u8], filename: &str, model: &str) -> (String, Vec<u8>) {
    let boundary = format!("----RuaBoundary{}", uuid::Uuid::new_v4().simple());
    let mime = mime_guess::from_path(filename)
        .first_or_octet_stream()
        .to_string();

    let mut body = Vec::new();

    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(b"Content-Disposition: form-data; name=\"model\"\r\n\r\n");
    body.extend_from_slice(model.as_bytes());
    body.extend_from_slice(b"\r\n");

    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        format!("Content-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\n")
            .as_bytes(),
    );
    body.extend_from_slice(format!("Content-Type: {mime}\r\n\r\n").as_bytes());
    body.extend_from_slice(audio_bytes);
    body.extend_from_slice(b"\r\n");

    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());

    (boundary, body)
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

pub fn render_channel_twiml(ws_url: &str, config: &serde_json::Value) -> String {
    let mut attrs = format!(
        r#"url="{}" dtmfDetection="true" interruptible="true""#,
        xml_escape(ws_url)
    );
    for (key, attribute) in [
        ("welcome_greeting", "welcomeGreeting"),
        ("tts_provider", "ttsProvider"),
        ("tts_voice", "voice"),
        ("stt_provider", "transcriptionProvider"),
        ("language", "language"),
    ] {
        if let Some(value) = config[key].as_str().filter(|value| !value.is_empty()) {
            attrs.push_str(&format!(r#" {attribute}="{}""#, xml_escape(value)));
        }
    }
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><Response><Connect><ConversationRelay {attrs} /></Connect></Response>"#
    )
}

pub fn public_ws_url(public_url: &str) -> rua_core::Result<String> {
    let mut url =
        reqwest::Url::parse(public_url).map_err(|error| rua_core::RuaError::Validation {
            message: format!("voice public_url: {error}"),
        })?;
    let scheme = match url.scheme() {
        "https" => "wss",
        "http" => "ws",
        _ => {
            return Err(rua_core::RuaError::Validation {
                message: "voice public_url must use HTTP(S)".into(),
            })
        }
    };
    url.set_scheme(scheme)
        .map_err(|_| rua_core::RuaError::Validation {
            message: "invalid voice URL".into(),
        })?;
    let path = format!("{}/voice/ws", url.path().trim_end_matches('/'));
    url.set_path(&path);
    url.set_query(None);
    url.set_fragment(None);
    Ok(url.into())
}

pub fn validate_signature(
    token: &str,
    url: &str,
    params: &std::collections::BTreeMap<String, String>,
    signature: &str,
) -> bool {
    use base64::Engine;
    use hmac::{Hmac, Mac};
    if token.is_empty() {
        return false;
    }
    let Ok(mut mac) = Hmac::<sha1::Sha1>::new_from_slice(token.as_bytes()) else {
        return false;
    };
    mac.update(url.as_bytes());
    for (key, value) in params {
        mac.update(key.as_bytes());
        mac.update(value.as_bytes());
    }
    let Ok(signature) = base64::engine::general_purpose::STANDARD.decode(signature) else {
        return false;
    };
    mac.verify_slice(&signature).is_ok()
}

/// Configure an existing Twilio number; does not purchase or place a call.
pub async fn configure_voice_webhook(config: &serde_json::Value) -> rua_core::Result<()> {
    let value = |key: &str| config[key].as_str().unwrap_or("").trim();
    for key in [
        "twilio_account_sid",
        "twilio_auth_token",
        "phone_number_sid",
        "public_url",
    ] {
        if value(key).is_empty() {
            return Err(rua_core::RuaError::Validation {
                message: format!("voice {key} is required"),
            });
        }
    }
    public_ws_url(value("public_url"))?;
    let base = config["twilio_api_base"]
        .as_str()
        .unwrap_or("https://api.twilio.com/2010-04-01");
    let mut url = reqwest::Url::parse(base).map_err(|error| rua_core::RuaError::Validation {
        message: format!("Twilio API URL: {error}"),
    })?;
    url.path_segments_mut()
        .map_err(|_| rua_core::RuaError::Validation {
            message: "invalid Twilio API URL".into(),
        })?
        .pop_if_empty()
        .push("Accounts")
        .push(value("twilio_account_sid"))
        .push("IncomingPhoneNumbers")
        .push(&format!("{}.json", value("phone_number_sid")));
    let public_url = value("public_url").trim_end_matches('/');
    let response = rua_http::default_client_or_default("rua-voice")
        .post(url)
        .basic_auth(
            value("twilio_account_sid"),
            Some(value("twilio_auth_token")),
        )
        .form(&[
            ("VoiceUrl", format!("{public_url}/voice/incoming")),
            ("VoiceMethod", "POST".into()),
            (
                "StatusCallback",
                format!("{public_url}/voice/status-callback"),
            ),
            ("StatusCallbackMethod", "POST".into()),
        ])
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .map_err(|error| rua_core::RuaError::Runtime {
            message: format!("Twilio webhook update: {error}"),
        })?;
    if !response.status().is_success() {
        return Err(rua_core::RuaError::Runtime {
            message: format!("Twilio webhook update returned {}", response.status()),
        });
    }
    Ok(())
}
