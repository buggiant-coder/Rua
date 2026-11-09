#![cfg(any(target_os = "macos", target_os = "linux"))]
use rua_core::phone_use::PhoneBridge;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio_util::sync::CancellationToken;

#[test]
fn observation_rejects_invalid_references_geometry_and_protected_text() {
    use rua_core::phone_use::PhoneObservation;
    let id = uuid::Uuid::new_v4();
    let value = json!({
        "observation_id":id,"service_generation":uuid::Uuid::new_v4(),
        "package_name":"test.app","window_id":1,"width":1080,"height":1920,
        "rotation":0,"truncated":false,"future":"preserved",
        "nodes":[{"token":format!("{id}:0"),"path":[],"text":"测试",
            "description":"","view_id":"field","class_name":"EditText",
            "bounds":[0,0,100,40],"clickable":true,"editable":true,
            "scrollable":false,"enabled":true,"visible":true,"password":false}]
    });
    let valid: PhoneObservation = serde_json::from_value(value.clone()).unwrap();
    valid.validate(30, 10).unwrap();
    assert_eq!(valid.extra["future"], "preserved");
    assert!(valid.validate(0, 10).is_err());
    let mut oversized_image = value.clone();
    oversized_image["screenshot"] =
        json!({"mime_type":"image/jpeg","width":1281,"height":100,"data":"/9j/2Q=="});
    assert!(serde_json::from_value::<PhoneObservation>(oversized_image)
        .unwrap()
        .validate(30, 10)
        .is_err());
    for (pointer, replacement) in [
        ("/nodes/0/token", json!("forged:0")),
        ("/nodes/0/password", json!(true)),
        ("/nodes/0/bounds", json!([100, 0, 1, 40])),
        ("/rotation", json!(4)),
        ("/width", json!(0)),
    ] {
        let mut invalid = value.clone();
        *invalid.pointer_mut(pointer).unwrap() = replacement;
        let observation: PhoneObservation = serde_json::from_value(invalid).unwrap();
        assert!(observation.validate(30, 10).is_err(), "{pointer}");
    }
}

#[test]
fn failed_post_action_observation_preserves_submitted_effect() {
    let mut value = json!({"state":"submitted","submitted":true,"effect_confirmed":false,"observation":{"invalid":true}});
    rua_core::phone_use::sanitize_action_observation(&mut value);
    assert_eq!(value["state"], "submitted");
    assert_eq!(value["submitted"], true);
    assert!(value.get("observation").is_none());
    assert!(value["observation_error"].is_string());
}

async fn fixture(listener: UnixListener, changed: bool) {
    for step in 0..2 {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut line = String::new();
        BufReader::new(&mut socket)
            .read_line(&mut line)
            .await
            .unwrap();
        let input: Value = serde_json::from_str(&line).unwrap();
        let result = if step == 0 {
            assert_eq!(input["method"], "hello");
            json!({"protocol_version":1,"generation":"first","auth_token":"a".repeat(64)})
        } else {
            assert_eq!(input["params"]["generation"], "first");
            json!({"generation":if changed {"other"} else {"first"},"platform":"android","state":"permission_required","accessibility":false,"camera_permission":true,"future":42})
        };
        let response = format!("{}\n", json!({"id":input["id"],"result":result}));
        socket.write_all(response.as_bytes()).await.unwrap();
    }
}

#[tokio::test]
async fn private_status_checks_peer_and_generation_without_exposing_credentials() {
    let temp = tempfile::Builder::new()
        .prefix("phone-")
        .tempdir_in("/tmp")
        .unwrap();
    let path = temp.path().join("bridge.sock");
    let listener = UnixListener::bind(&path).unwrap();
    let server = tokio::spawn(fixture(listener, false));
    let cancel = CancellationToken::new();
    let bridge = PhoneBridge::connect(path, std::process::id(), &cancel)
        .await
        .unwrap();
    assert!(bridge.accepts_token(&"a".repeat(64)));
    assert!(!bridge.accepts_token(&"b".repeat(64)));
    bridge.revoke_token();
    assert!(!bridge.accepts_token(&"a".repeat(64)));
    let status = bridge.status(&cancel).await.unwrap();
    assert!(!status.accessibility);
    assert!(status.camera_permission);
    assert_eq!(status.extra["future"], 42);
    assert!(!serde_json::to_string(&status)
        .unwrap()
        .contains("auth_token"));
    server.await.unwrap();
}

#[tokio::test]
async fn private_ready_publishes_the_bound_port_and_validates_acknowledgement() {
    for changed in [false, true] {
        let occupied = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let http = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = http.local_addr().unwrap().port();
        assert_ne!(occupied.local_addr().unwrap().port(), port);
        let temp = tempfile::Builder::new()
            .prefix("phone-")
            .tempdir_in("/tmp")
            .unwrap();
        let path = temp.path().join("bridge.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let server = tokio::spawn(async move {
            for step in 0..2 {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut line = String::new();
                BufReader::new(&mut socket)
                    .read_line(&mut line)
                    .await
                    .unwrap();
                let input: Value = serde_json::from_str(&line).unwrap();
                let result = if step == 0 {
                    json!({"protocol_version":1,"generation":"first","auth_token":"a".repeat(64)})
                } else {
                    assert_eq!(input["method"], "resident_ready");
                    assert_eq!(input["params"]["generation"], "first");
                    assert_eq!(input["params"]["port"], port);
                    json!({"generation":if changed { "stale" } else { "first" },"port":port})
                };
                let response = format!("{}\n", json!({"id":input["id"],"result":result}));
                socket.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let cancel = CancellationToken::new();
        let bridge = PhoneBridge::connect(path, std::process::id(), &cancel)
            .await
            .unwrap();
        assert!(bridge.publish_http_port(0, &cancel).await.is_err());
        assert_eq!(
            bridge.publish_http_port(port, &cancel).await.is_err(),
            changed
        );
        server.await.unwrap();
    }
}

#[tokio::test]
async fn changed_instance_is_not_silently_reconnected() {
    let temp = tempfile::Builder::new()
        .prefix("phone-")
        .tempdir_in("/tmp")
        .unwrap();
    let path = temp.path().join("bridge.sock");
    let listener = UnixListener::bind(&path).unwrap();
    let server = tokio::spawn(fixture(listener, true));
    let cancel = CancellationToken::new();
    let bridge = PhoneBridge::connect(path, std::process::id(), &cancel)
        .await
        .unwrap();
    assert!(bridge
        .status(&cancel)
        .await
        .unwrap_err()
        .contains("instance changed"));
    server.await.unwrap();
}

#[tokio::test]
async fn wrong_peer_and_pre_cancelled_call_are_rejected() {
    let temp = tempfile::Builder::new()
        .prefix("phone-")
        .tempdir_in("/tmp")
        .unwrap();
    let path = temp.path().join("bridge.sock");
    let _listener = UnixListener::bind(&path).unwrap();
    let cancel = CancellationToken::new();
    assert!(
        PhoneBridge::connect(path.clone(), std::process::id() + 1, &cancel)
            .await
            .err()
            .unwrap()
            .contains("Untrusted")
    );
    cancel.cancel();
    assert!(PhoneBridge::connect(path, std::process::id(), &cancel)
        .await
        .err()
        .unwrap()
        .contains("cancelled"));
}

#[tokio::test]
async fn oversized_frame_and_wrong_response_id_are_rejected() {
    for oversized in [false, true] {
        let temp = tempfile::Builder::new()
            .prefix("phone-")
            .tempdir_in("/tmp")
            .unwrap();
        let path = temp.path().join("bridge.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut line = String::new();
            BufReader::new(&mut socket)
                .read_line(&mut line)
                .await
                .unwrap();
            let response = if oversized {
                vec![b'x'; 65537]
            } else {
                b"{\"id\":\"wrong\",\"result\":{}}\n".to_vec()
            };
            let _ = socket.write_all(&response).await;
        });
        let error = PhoneBridge::connect(path, std::process::id(), &CancellationToken::new())
            .await
            .err()
            .unwrap();
        assert!(
            error.contains(if oversized { "frame" } else { "Mismatched" }),
            "{error}"
        );
        server.await.unwrap();
    }
}
