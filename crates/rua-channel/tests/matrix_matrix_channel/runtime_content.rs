use super::*;
use rua_channel::matrix::MatrixOptions;
use serde_json::{json, Value};

fn message(id: &str, sender: &str, content: Value) -> Value {
    json!({"type":"m.room.message","event_id":id,"sender":sender,
        "origin_server_ts":1715000000000_u64,"content":content})
}

async fn setup(
    events: Vec<Value>,
    options: MatrixOptions,
    acl: Value,
) -> (
    MatrixChannel,
    mock_homeserver::MockState,
    tokio::task::JoinHandle<()>,
    tempfile::TempDir,
    Arc<Mutex<Vec<AgentRequest>>>,
) {
    let state = mock_homeserver::MockState::new(json!({
        "next_batch":"s1","rooms":{"join":{"!testroom:localhost":{
            "timeline":{"events":events,"limited":false,"prev_batch":"t1"},
            "state":{"events":[]},"ephemeral":{"events":[]}
        }}}
    }));
    let (server, url) = mock_homeserver::start(state.clone()).await;
    let tmp = tempfile::tempdir().unwrap();
    let channel = MatrixChannel::new(MatrixConfig {
        homeserver_url: url,
        user_id: "@bot:localhost".into(),
        password: "password".into(),
        store_path: Some(tmp.path().join("store").to_string_lossy().into_owned()),
        media_dir: tmp.path().join("media").to_string_lossy().into_owned(),
        ..Default::default()
    })
    .with_options(options)
    .with_acl_config(acl);
    let requests = Arc::new(Mutex::new(Vec::new()));
    channel
        .start(Arc::new(RecordingProcess {
            requests: requests.clone(),
            notify: Arc::default(),
        }))
        .await
        .unwrap();
    (channel, state, server, tmp, requests)
}

async fn await_requests(requests: &Mutex<Vec<AgentRequest>>, count: usize) {
    tokio::time::timeout(Duration::from_secs(15), async {
        while requests.lock().await.len() < count {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn live_sync_downloads_all_media_and_thread_replies_keep_the_root() {
    let events = [
        ("m.image", "media123", "image/png"),
        ("m.audio", "audio123", "audio/mpeg"),
        ("m.video", "video123", "video/mp4"),
        ("m.file", "file123", "application/pdf"),
    ]
    .into_iter()
    .enumerate()
    .map(|(i, (kind, id, mime))| {
        message(
            &format!("$media{i}"),
            "@alice:localhost",
            json!({
                "msgtype":kind,"body":format!("{id}.bin"),"url":format!("mxc://example.com/{id}"),
                "info":{"mimetype":mime},"m.mentions":{"user_ids":["@bot:localhost"]},
                "m.relates_to":{"rel_type":"m.thread","event_id":"$thread-root"}
            }),
        )
    })
    .collect();
    let (channel, state, server, tmp, requests) = setup(
        events,
        MatrixOptions {
            mention_pill_in_body: true,
            ..Default::default()
        },
        json!({"streaming_enabled":false}),
    )
    .await;
    await_requests(&requests, 4).await;
    let requests = requests.lock().await;
    assert_eq!(requests.len(), 4);
    for req in requests.iter() {
        assert_eq!(
            req.session_id,
            "matrix_thread:!testroom:localhost:$thread-root"
        );
        assert_eq!(req.metadata["matrix_thread_root_event_id"], "$thread-root");
    }
    let media: Vec<_> = requests
        .iter()
        .flat_map(|req| &req.input)
        .filter(|part| !matches!(part, IncomingContentPart::Text { .. }))
        .collect();
    assert!(media
        .iter()
        .any(|part| matches!(part,IncomingContentPart::Image { path,.. } if path.exists())));
    assert!(media
        .iter()
        .any(|part| matches!(part,IncomingContentPart::Audio { path,.. } if path.exists())));
    assert!(media
        .iter()
        .any(|part| matches!(part,IncomingContentPart::Video { path,.. } if path.exists())));
    assert!(media
        .iter()
        .any(|part| matches!(part,IncomingContentPart::File { path,.. } if path.exists())));
    let attachment_path = media
        .iter()
        .find_map(|part| match part {
            IncomingContentPart::File { path, .. } => Some(path.clone()),
            _ => None,
        })
        .unwrap();
    let mut attachment = make_outgoing_msg("");
    attachment.content = vec![OutgoingContentPart::File {
        path: Some(attachment_path),
        url: None,
        name: Some("thread.pdf".into()),
    }];
    let mut to = make_target("!testroom:localhost");
    to.metadata = json!({"matrix_thread_root_event_id":"$thread-root"});
    channel.send(to.clone(), attachment).await.unwrap();
    channel
        .send(to, make_outgoing_msg("hi @alice:localhost <ok>"))
        .await
        .unwrap();
    let sent = state.sent_messages.lock().await;
    let media_reply = sent
        .iter()
        .find(|event| event["msgtype"] == "m.file")
        .expect("thread attachment sent");
    assert_eq!(media_reply["m.relates_to"]["rel_type"], "m.thread");
    assert_eq!(media_reply["m.relates_to"]["event_id"], "$thread-root");
    let reply = sent.last().unwrap();
    assert_eq!(reply["m.relates_to"]["event_id"], "$thread-root");
    assert_eq!(reply["m.mentions"]["user_ids"], json!(["@alice:localhost"]));
    assert!(reply["formatted_body"]
        .as_str()
        .unwrap()
        .contains("&lt;ok&gt;"));
    assert!(reply["formatted_body"]
        .as_str()
        .unwrap()
        .contains("https://matrix.to/#/"));
    drop(sent);
    drop(requests);
    let token_path = tmp.path().join("store/sync-token");
    // Recording the last request precedes completing the sync response and
    // persisting its cursor. Stop only after that response has completed.
    tokio::time::timeout(Duration::from_secs(5), async {
        while tokio::fs::read_to_string(&token_path)
            .await
            .unwrap_or_default()
            != "s1"
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    channel.stop().await.unwrap();
    assert_eq!(std::fs::read_to_string(token_path).unwrap(), "s1");
    server.abort();
}

#[tokio::test]
async fn group_history_is_bounded_skips_denied_senders_and_commands_stay_commands() {
    let events = vec![
        message(
            "$denied",
            "@mallory:localhost",
            json!({"msgtype":"m.text","body":"denied secret"}),
        ),
        message(
            "$discarded",
            "@alice:localhost",
            json!({"msgtype":"m.text","body":"oldest context"}),
        ),
        message(
            "$retained",
            "@alice:localhost",
            json!({"msgtype":"m.text","body":"useful context"}),
        ),
        message(
            "$mention",
            "@alice:localhost",
            json!({"msgtype":"m.text","body":"@bot:localhost help"}),
        ),
        message(
            "$context",
            "@alice:localhost",
            json!({"msgtype":"m.text","body":"before command"}),
        ),
        message(
            "$command",
            "@alice:localhost",
            json!({"msgtype":"m.text","body":"@bot:localhost /reset"}),
        ),
        message(
            "$silent",
            "@alice:localhost",
            json!({"msgtype":"m.text","body":"@bot:localhost NO_REPLY"}),
        ),
    ];
    let (channel,_,server,_tmp,requests)=setup(events,MatrixOptions {
        history_limit:1, group_allow_from:vec!["@alice:localhost".into()],..Default::default()
    },json!({"group_policy":"allowlist","allow_from":["@nobody:localhost"],"streaming_enabled":false})).await;
    await_requests(&requests, 2).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let requests = requests.lock().await;
    assert_eq!(requests.len(), 2);
    let text = requests[0]
        .input
        .iter()
        .filter_map(|part| {
            if let IncomingContentPart::Text { text } = part {
                Some(text.as_str())
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("useful context"));
    assert!(!text.contains("oldest context"));
    assert!(!text.contains("denied secret"));
    assert!(matches!(&requests[1].input[0],IncomingContentPart::Text { text } if text=="/clear"));
    drop(requests);
    channel.stop().await.unwrap();
    server.abort();
}

#[tokio::test]
async fn per_room_auto_reply_and_vision_disabled_apply_to_live_events() {
    let events = vec![message(
        "$image",
        "@alice:localhost",
        json!({
            "msgtype":"m.image","body":"photo.png","url":"mxc://example.com/media123"
        }),
    )];
    let (channel, _, server, _tmp, requests) = setup(
        events,
        MatrixOptions {
            vision_enabled: false,
            groups: std::collections::BTreeMap::from([("*".into(), json!({"autoReply":true}))]),
            ..Default::default()
        },
        json!({}),
    )
    .await;
    await_requests(&requests, 1).await;
    let requests = requests.lock().await;
    assert!(requests[0]
        .input
        .iter()
        .all(|part| matches!(part, IncomingContentPart::Text { .. })));
    assert!(
        matches!(&requests[0].input[0],IncomingContentPart::Text { text } if text.contains("vision disabled"))
    );
    drop(requests);
    channel.stop().await.unwrap();
    server.abort();
}

#[tokio::test]
async fn restart_restores_device_and_sync_cursor_without_another_password_login() {
    let events = vec![message(
        "$ping",
        "@alice:localhost",
        json!({"msgtype":"m.text","body":"@bot:localhost ping"}),
    )];
    let (channel, state, server, tmp, requests) = setup(
        events,
        MatrixOptions {
            sync_timeout_ms: 9000,
            ..Default::default()
        },
        json!({}),
    )
    .await;
    await_requests(&requests, 1).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while std::fs::read_to_string(tmp.path().join("store/sync-token"))
            .ok()
            .as_deref()
            != Some("s1")
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    channel.stop().await.unwrap();
    let previous_queries = state.sync_queries.lock().await.len();
    channel.start(Arc::new(MockProcess)).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while state.sync_queries.lock().await.len() <= previous_queries {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        state.login_count.load(std::sync::atomic::Ordering::SeqCst),
        1
    );
    let queries = state.sync_queries.lock().await;
    assert_eq!(
        queries.last().unwrap().get("since").map(String::as_str),
        Some("s1")
    );
    assert_eq!(
        queries.last().unwrap().get("timeout").map(String::as_str),
        Some("9000")
    );
    drop(queries);
    channel.stop().await.unwrap();
    server.abort();
}

#[tokio::test]
async fn encrypted_media_is_decrypted_and_tampered_ciphertext_is_rejected() {
    use aes::cipher::{BlockEncrypt, KeyInit};
    use base64::{
        engine::general_purpose::{STANDARD_NO_PAD, URL_SAFE_NO_PAD},
        Engine,
    };
    use sha2::{Digest, Sha256};

    // Fixed AES-CTR test vector; encrypt locally without another SDK dependency.
    let key = [0x42u8; 32];
    let iv = [0u8; 16];
    let plain = b"private matrix attachment";
    let cipher = aes::Aes256::new_from_slice(&key).unwrap();
    let mut encrypted = Vec::new();
    for (counter, part) in plain.chunks(16).enumerate() {
        let mut block = aes::cipher::Block::<aes::Aes256>::from((counter as u128).to_be_bytes());
        cipher.encrypt_block(&mut block);
        encrypted.extend(part.iter().zip(block.iter()).map(|(a, b)| a ^ b));
    }
    let hash = STANDARD_NO_PAD.encode(Sha256::digest(&encrypted));
    let file = json!({
        "url":"mxc://example.com/encrypted","v":"v2",
        "key":{"kty":"oct","key_ops":["encrypt","decrypt"],"alg":"A256CTR","k":URL_SAFE_NO_PAD.encode(key),"ext":true},
        "iv":STANDARD_NO_PAD.encode(iv),"hashes":{"sha256":hash}
    });
    let bad_file = {
        let mut value = file.clone();
        value["url"] = json!("mxc://example.com/tampered");
        value
    };
    let events = vec![
        message(
            "$encrypted",
            "@alice:localhost",
            json!({"msgtype":"m.file","body":"private.txt","file":file,"m.mentions":{"user_ids":["@bot:localhost"]}}),
        ),
        message(
            "$tampered",
            "@alice:localhost",
            json!({"msgtype":"m.file","body":"bad.txt","file":bad_file,"m.mentions":{"user_ids":["@bot:localhost"]}}),
        ),
    ];
    let state = mock_homeserver::MockState::new(json!({
        "next_batch":"s1","rooms":{"join":{"!testroom:localhost":{
            "timeline":{"events":events,"limited":false,"prev_batch":"t1"},
            "state":{"events":[]},"ephemeral":{"events":[]}
        }}}
    }));
    state
        .download_bytes
        .lock()
        .await
        .insert("encrypted".into(), encrypted.clone());
    encrypted[0] ^= 1;
    state
        .download_bytes
        .lock()
        .await
        .insert("tampered".into(), encrypted);
    let (server, url) = mock_homeserver::start(state).await;
    let tmp = tempfile::tempdir().unwrap();
    let channel = MatrixChannel::new(MatrixConfig {
        homeserver_url: url,
        password: "test".into(),
        user_id: "@bot:localhost".into(),
        store_path: Some(tmp.path().join("store").to_string_lossy().into_owned()),
        media_dir: tmp.path().join("media").to_string_lossy().into_owned(),
        ..Default::default()
    });
    let requests = Arc::new(Mutex::new(Vec::new()));
    channel
        .start(Arc::new(RecordingProcess {
            requests: requests.clone(),
            notify: Arc::default(),
        }))
        .await
        .unwrap();
    await_requests(&requests, 2).await;
    let requests = requests.lock().await;
    let path = requests[0]
        .input
        .iter()
        .find_map(|part| match part {
            IncomingContentPart::File { path, .. } => Some(path),
            _ => None,
        })
        .expect("decrypted file");
    assert_eq!(std::fs::read(path).unwrap(), plain);
    assert!(requests[1]
        .input
        .iter()
        .all(|part| matches!(part, IncomingContentPart::Text { .. })));
    assert!(requests[1].input.iter().any(|part|matches!(part,IncomingContentPart::Text { text } if text.contains("Media unavailable"))));
    drop(requests);
    channel.stop().await.unwrap();
    server.abort();
}

#[tokio::test]
async fn live_stream_edits_preserve_thread_root_and_structured_mentions() {
    struct ReplyProcess;
    #[async_trait]
    impl Process for ReplyProcess {
        async fn handle(&self, _: AgentRequest) -> BoxStream<'static, RuntimeEvent> {
            Box::pin(futures::stream::iter(vec![
                RuntimeEvent::TextDelta {
                    delta: "hi @alice:localhost".into(),
                    message_id: None,
                },
                RuntimeEvent::TextDelta {
                    delta: " from Rua".into(),
                    message_id: None,
                },
            ]))
        }
    }

    let state = mock_homeserver::MockState::new(json!({
        "next_batch":"s1","rooms":{"join":{"!testroom:localhost":{
            "timeline":{"events":[message("$thread-input", "@alice:localhost", json!({
                "msgtype":"m.text","body":"@bot:localhost hello",
                "m.mentions":{"user_ids":["@bot:localhost"]},
                "m.relates_to":{"rel_type":"m.thread","event_id":"$root"}
            }))],"limited":false,"prev_batch":"t1"},
            "state":{"events":[]},"ephemeral":{"events":[]}
        }}}
    }));
    let (server, url) = mock_homeserver::start(state.clone()).await;
    let tmp = tempfile::tempdir().unwrap();
    let channel = MatrixChannel::new(MatrixConfig {
        homeserver_url: url,
        user_id: "@bot:localhost".into(),
        password: "password".into(),
        store_path: Some(tmp.path().join("store").to_string_lossy().into_owned()),
        ..Default::default()
    })
    .with_acl_config(json!({"streaming_enabled":true}));
    channel.start(Arc::new(ReplyProcess)).await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if state
                .sent_messages
                .lock()
                .await
                .iter()
                .any(|v| v["m.new_content"]["body"] == "hi @alice:localhost from Rua")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();

    let messages = state.sent_messages.lock().await;
    assert_eq!(messages[0]["m.relates_to"]["rel_type"], "m.thread");
    assert_eq!(messages[0]["m.relates_to"]["event_id"], "$root");
    let edit = messages
        .iter()
        .find(|v| v["m.new_content"]["body"] == "hi @alice:localhost from Rua")
        .unwrap();
    assert_eq!(edit["m.relates_to"]["rel_type"], "m.replace");
    assert_eq!(edit["m.new_content"]["m.relates_to"]["event_id"], "$root");
    assert_eq!(
        edit["m.new_content"]["m.mentions"]["user_ids"],
        json!(["@alice:localhost"])
    );
    drop(messages);
    channel.stop().await.unwrap();
    server.abort();
}
