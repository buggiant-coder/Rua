use rua_core::desktop_control::{DesktopControlCommand, DesktopControlEndpoint};
use rua_core::{Result, RuaError};
use serde_json::json;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn native_transport_authenticates_dispatches_errors_and_cancels_pending_work() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let endpoint = DesktopControlEndpoint::new(address).unwrap();
    let wrong_endpoint = DesktopControlEndpoint::new(address).unwrap();
    let cancel = CancellationToken::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let called = calls.clone();
    let server = tokio::spawn(
        endpoint
            .clone()
            .serve(listener, cancel.clone(), move |command| {
                let called = called.clone();
                async move {
                    called.fetch_add(1, Ordering::SeqCst);
                    match command {
                        DesktopControlCommand::LegacyStatus => Ok(json!({"cliInstalled": false})),
                        DesktopControlCommand::UpdateStatus => Err(RuaError::Runtime {
                            message: "host failed".into(),
                        }),
                        _ => std::future::pending::<Result<serde_json::Value>>().await,
                    }
                }
            }),
    );
    assert!(wrong_endpoint
        .call(DesktopControlCommand::LegacyStatus)
        .await
        .is_err());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        endpoint
            .call(DesktopControlCommand::LegacyStatus)
            .await
            .unwrap(),
        json!({"cliInstalled":false})
    );
    assert!(
        matches!(endpoint.call(DesktopControlCommand::UpdateStatus).await, Err(RuaError::Runtime {message}) if message == "host failed")
    );
    let waiting =
        tokio::spawn(async move { endpoint.call(DesktopControlCommand::CheckUpdate).await });
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while calls.load(Ordering::SeqCst) != 3 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    cancel.cancel();
    tokio::time::timeout(std::time::Duration::from_secs(2), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(waiting.await.unwrap().is_err());
}

#[test]
fn native_endpoint_refuses_external_or_unspecified_addresses() {
    for address in ["0.0.0.0:8765", "192.168.1.2:8765", "127.0.0.1:0"] {
        assert!(DesktopControlEndpoint::new(address.parse().unwrap()).is_err());
    }
}
