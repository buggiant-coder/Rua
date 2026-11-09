use std::time::Duration;

use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn bridge_fires_token_when_watch_flips_true() {
    let (tx, rx) = watch::channel(false);
    let token = CancellationToken::new();
    rua_agent::runtime::bridge_watch_to_token(rx, token.clone());

    assert!(!token.is_cancelled(), "token must start uncancelled");

    tx.send(true).expect("send cancel");

    tokio::time::timeout(Duration::from_millis(200), token.cancelled())
        .await
        .expect("token must be cancelled within 200ms of watch flip");
    assert!(token.is_cancelled());
}

#[tokio::test]
async fn bridge_fires_immediately_if_watch_already_true() {
    let (_tx, rx) = watch::channel(true);
    let token = CancellationToken::new();
    rua_agent::runtime::bridge_watch_to_token(rx, token.clone());

    tokio::time::timeout(Duration::from_millis(200), token.cancelled())
        .await
        .expect("token must be cancelled within 200ms when watch starts true");
}

#[tokio::test]
async fn bridge_does_not_fire_on_redundant_false_updates() {
    let (tx, rx) = watch::channel(false);
    let token = CancellationToken::new();
    rua_agent::runtime::bridge_watch_to_token(rx, token.clone());

    tx.send(false).expect("send redundant false");
    tx.send(false).expect("send another redundant false");
    tokio::time::sleep(Duration::from_millis(50)).await;

    assert!(
        !token.is_cancelled(),
        "token must not fire while watch stays false"
    );
}

#[tokio::test]
async fn bridge_exits_quietly_when_sender_dropped() {
    let (tx, rx) = watch::channel(false);
    let token = CancellationToken::new();
    rua_agent::runtime::bridge_watch_to_token(rx, token.clone());

    drop(tx);
    tokio::time::sleep(Duration::from_millis(50)).await;

    assert!(!token.is_cancelled());
}
