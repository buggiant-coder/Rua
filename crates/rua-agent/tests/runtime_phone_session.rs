#![cfg(any(target_os = "macos", target_os = "linux"))]

use super::*;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

async fn bridge_fixture() -> (
    tempfile::TempDir,
    rua_core::phone_use::PhoneBridge,
    tokio::sync::mpsc::Receiver<Value>,
    tokio::task::JoinHandle<()>,
) {
    let temp = tempfile::Builder::new()
        .prefix("phone-hook-")
        .tempdir_in("/tmp")
        .unwrap();
    let path = temp.path().join("bridge.sock");
    let listener = tokio::net::UnixListener::bind(&path).unwrap();
    let (tx, rx) = tokio::sync::mpsc::channel(8);
    let server = tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut line = String::new();
            BufReader::new(&mut socket)
                .read_line(&mut line)
                .await
                .unwrap();
            let input: Value = serde_json::from_str(&line).unwrap();
            let result = if input["method"] == "hello" {
                json!({"protocol_version":2,"generation":"phone-hook-test","auth_token":"a".repeat(64)})
            } else {
                tx.send(input.clone()).await.unwrap();
                if input["method"] == "bind_turn" {
                    json!({"bound":true})
                } else {
                    json!({"released":true})
                }
            };
            let response = format!("{}\n", json!({"id":input["id"],"result":result}));
            socket.write_all(response.as_bytes()).await.unwrap();
        }
    });
    let bridge = rua_core::phone_use::PhoneBridge::connect(
        path,
        std::process::id(),
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    (temp, bridge, rx, server)
}

fn session_context() -> rua_core::phone_use::PhoneSessionContext {
    rua_core::phone_use::PhoneSessionContext::from_metadata(
        "default",
        &HashMap::from([
            (
                crate::hooks::SECURITY_CHAT_ADDRESS_METADATA_KEY.into(),
                json!(rua_core::ChatAddress::new("phone-chat")),
            ),
            ("turn_id".into(), json!("phone-turn")),
            ("authority_epoch".into(), json!("authority-epoch")),
        ]),
    )
    .unwrap()
}

#[tokio::test]
async fn normal_turn_completion_retains_the_phone_session() {
    let (_temp, bridge, mut requests, server) = bridge_fixture().await;
    let hook = PhoneSessionHook {
        bridge: &bridge,
        context: session_context(),
        cancel: CancellationToken::new(),
    };
    hook.on_finish(1, &Ok(())).await;
    assert!(requests.try_recv().is_err());
    server.abort();
}

#[tokio::test]
async fn next_turn_binds_the_existing_chat_session_before_model_execution() {
    let (_temp, bridge, mut requests, server) = bridge_fixture().await;
    let first = PhoneSessionHook {
        bridge: &bridge,
        context: session_context(),
        cancel: CancellationToken::new(),
    };
    first.bind_turn().await;
    let initial_binding = requests.try_recv().expect("initial Turn binding");
    assert_eq!(initial_binding["method"], "bind_turn");
    first.on_finish(1, &Ok(())).await;
    assert!(requests.try_recv().is_err());

    let mut next_context = session_context();
    next_context.turn_id = "next-phone-turn".into();
    next_context.authority_epoch = "next-authority-epoch".into();
    let next = PhoneSessionHook {
        bridge: &bridge,
        context: next_context,
        cancel: CancellationToken::new(),
    };
    next.bind_turn().await;
    let next_binding = requests.try_recv().expect("binding before model execution");
    assert_eq!(next_binding["method"], "bind_turn");
    let invocation = &next_binding["params"]["invocation"];
    assert_eq!(
        invocation["owner"],
        initial_binding["params"]["invocation"]["owner"]
    );
    assert_eq!(
        invocation["chat_address"],
        initial_binding["params"]["invocation"]["chat_address"]
    );
    assert_eq!(invocation["turn_id"], "next-phone-turn");
    assert_eq!(invocation["authority_epoch"], "next-authority-epoch");
    assert!(requests.try_recv().is_err());
    server.abort();
}

#[tokio::test]
async fn failed_cancelled_and_timed_out_turns_release_the_captured_session() {
    for (result, cancelled) in [
        (
            Err(react_agent::AgentError::Internal("test failure".into())),
            false,
        ),
        (Err(react_agent::AgentError::Cancelled), false),
        (
            Err(react_agent::AgentError::Timeout { timeout_secs: 1 }),
            false,
        ),
        (Ok(()), true),
    ] {
        let (_temp, bridge, mut requests, server) = bridge_fixture().await;
        let cancel = CancellationToken::new();
        if cancelled {
            cancel.cancel();
        }
        let hook = PhoneSessionHook {
            bridge: &bridge,
            context: session_context(),
            cancel,
        };
        if cancelled {
            hook.bind_turn().await;
            assert!(requests.try_recv().is_err());
        }
        hook.on_finish(1, &result).await;
        let request = requests.try_recv().expect("session cleanup IPC");
        assert_eq!(request["method"], "release");
        assert_eq!(request["params"]["generation"], "phone-hook-test");
        assert_eq!(
            request["params"]["invocation"]["chat_address"],
            json!(rua_core::ChatAddress::new("phone-chat"))
        );
        assert_eq!(request["params"]["invocation"]["turn_id"], "phone-turn");
        assert_eq!(
            request["params"]["invocation"]["authority_epoch"],
            "authority-epoch"
        );
        assert!(requests.try_recv().is_err());
        server.abort();
    }
}
