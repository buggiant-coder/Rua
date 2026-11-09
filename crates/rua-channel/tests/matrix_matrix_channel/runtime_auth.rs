use super::*;

async fn authenticated_media_roundtrip(use_token: bool) {
    let state = mock_homeserver::MockState::new(serde_json::json!({
        "next_batch":"s1",
        "rooms":{"join":{"!testroom:localhost":{
            "timeline":{"events":[{
                "type":"m.room.message","event_id":"$self:localhost","sender":"@bot:localhost",
                "origin_server_ts":1715000000000_u64,"content":{"msgtype":"m.text","body":"own reply"}
            },{
                "type":"m.room.message","event_id":"$other:localhost","sender":"@alice:localhost",
                "origin_server_ts":1715000000000_u64,"content":{"msgtype":"m.text","body":"hello from alice","m.mentions":{"user_ids":["@bot:localhost"]}}
            }],"limited":false,"prev_batch":"t1"},
            "state":{"events":[]},"ephemeral":{"events":[]}
        }}}
    }));
    let (server, url) = mock_homeserver::start(state.clone()).await;
    let tmp = tempfile::tempdir().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let notify = Arc::new(tokio::sync::Notify::new());
    let channel = MatrixChannel::new(MatrixConfig {
        homeserver_url: url,
        // Localpart login or token-only: sync must use the server's user ID.
        user_id: if use_token {
            String::new()
        } else {
            "bot".into()
        },
        access_token: if use_token {
            "syt_test_token".into()
        } else {
            String::new()
        },
        password: if use_token {
            String::new()
        } else {
            "password".into()
        },
        default_room_id: "!different-room:localhost".into(),
        store_path: Some(tmp.path().join("store").to_string_lossy().into_owned()),
        ..Default::default()
    });
    channel
        .start(Arc::new(RecordingProcess {
            requests: requests.clone(),
            notify: notify.clone(),
        }))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), notify.notified())
        .await
        .unwrap();
    {
        let incoming = requests.lock().await;
        assert_eq!(incoming.len(), 1, "own events must be ignored");
        assert_eq!(incoming[0].user_id, "@alice:localhost");
        assert_eq!(
            incoming[0].conversation_id, "!testroom:localhost",
            "reply belongs to the incoming room"
        );
        assert_eq!(incoming[0].metadata["is_group"], true);
    }
    let file = tmp.path().join("attachment.txt");
    {
        use std::io::Write;
        let mut temporary = tempfile::NamedTempFile::new_in(tmp.path()).unwrap();
        temporary.write_all(b"matrix attachment").unwrap();
        temporary.as_file().sync_all().unwrap();
        temporary.persist(&file).unwrap();
        std::fs::File::open(tmp.path()).unwrap().sync_all().unwrap();
    }
    let mut message = make_outgoing_msg("");
    message.content = vec![OutgoingContentPart::File {
        url: None,
        path: Some(file),
        name: Some("attachment.txt".into()),
    }];
    channel
        .send(make_target("!testroom:localhost"), message)
        .await
        .unwrap();
    assert_eq!(*state.media_auth.lock().await, ["Bearer syt_test_token"]);
    let messages = state.sent_messages.lock().await;
    assert!(
        messages
            .iter()
            .any(|message| message["msgtype"] == "m.file"),
        "{messages:?}"
    );
    assert_eq!(
        state.whoami_count.load(std::sync::atomic::Ordering::SeqCst),
        usize::from(use_token)
    );
    channel.stop().await.unwrap();
    server.abort();
}

#[tokio::test]
async fn token_restores_server_identity_and_authenticated_media() {
    authenticated_media_roundtrip(true).await;
}

#[tokio::test]
async fn password_login_reuses_issued_token_for_media_and_own_event_filter() {
    authenticated_media_roundtrip(false).await;
}
