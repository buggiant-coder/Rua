use super::*;

// Upstream #4444: a disconnected relay must still answer route queries while backing off.
#[tokio::test(start_paused = true)]
async fn relay_backoff_answers_route_queries_and_cancels_promptly() {
    let secret_key = SecretKey::from_bytes(&[1u8; 32]);
    let peer = SecretKey::from_bytes(&[2u8; 32]).public();
    let (prio_tx, prio_rx) = mpsc::channel(8);
    let (_inbox_tx, inbox) = mpsc::channel(16);
    let (_send_tx, relay_datagrams_send) = mpsc::channel(16);
    let (relay_datagrams_recv, _recv_rx) = mpsc::channel(16);
    let stop_token = CancellationToken::new();
    let mut actor = ActiveRelayActor::new(ActiveRelayActorOptions {
        url: "https://relay.invalid".parse().unwrap(),
        prio_inbox_: prio_rx,
        inbox,
        relay_datagrams_send,
        relay_datagrams_recv,
        connection_opts: RelayConnectionOptions {
            secret_key,
            dns_resolver: DnsResolver::new(),
            proxy_url: None,
            prefer_ipv6: Arc::new(AtomicBool::new(true)),
            tls_config: iroh_relay::tls::CaRootsConfig::insecure_skip_verify()
                .client_config(iroh_relay::tls::default_provider())
                .unwrap(),
            auth_token: None,
        },
        stop_token: stop_token.clone(),
        metrics: Default::default(),
        my_relay: Default::default(),
    });
    let backoff = tokio::spawn(async move { actor.sleep_backoff(Duration::from_secs(10)).await });
    let (tx, rx) = oneshot::channel();
    prio_tx
        .send(ActiveRelayPrioMessage::HasEndpointRoute(peer, tx))
        .await
        .unwrap();
    let reply = time::timeout(Duration::from_millis(100), rx)
        .await
        .expect("route query stalled behind relay reconnect backoff")
        .unwrap();
    assert!(!reply);
    assert!(
        !backoff.is_finished(),
        "query must not skip reconnect backoff"
    );
    stop_token.cancel();
    assert!(
        !time::timeout(Duration::from_millis(100), backoff)
            .await
            .expect("cancellation stalled behind relay reconnect backoff")
            .unwrap()
    );
}
