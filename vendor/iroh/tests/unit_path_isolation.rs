//! An old connection's path failure must not close another connection's working path.
use super::*;
use crate::{
    metrics::EndpointMetrics,
    tls::{self, TlsConfig},
};
use iroh_base::SecretKey;
use n0_watcher::Watchable;

#[tokio::test]
async fn abandoned_path_does_not_close_a_healthy_sibling_connection() {
    let server_key = SecretKey::generate();
    let server_tls = TlsConfig::new(server_key.clone(), 8, tls::default_provider());
    let mut transport = noq::TransportConfig::default();
    transport.max_concurrent_multipath_paths(4);
    let transport = Arc::new(transport);
    let mut server_config =
        noq::ServerConfig::with_crypto(Arc::new(server_tls.make_server_config(false).unwrap()));
    server_config.transport_config(transport.clone());
    let server = noq::Endpoint::server(server_config, "0.0.0.0:0".parse().unwrap()).unwrap();
    let port = server.local_addr().unwrap().port();
    let primary: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let proxy = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let backup = proxy.local_addr().unwrap();
    let cancel = CancellationToken::new();
    let proxy_cancel = cancel.clone();
    let forwarding = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let proxy_forwarding = forwarding.clone();
    let _guard = cancel.drop_guard();
    let proxy_task = tokio::spawn(async move {
        let mut client_addr = None;
        let mut buffer = [0u8; 65536];
        loop {
            let (len, source) = tokio::select! {
                _ = proxy_cancel.cancelled() => break,
                packet = proxy.recv_from(&mut buffer) => packet.unwrap(),
            };
            if !proxy_forwarding.load(std::sync::atomic::Ordering::Relaxed) {
                continue;
            }
            let target = if source == primary {
                client_addr.unwrap()
            } else {
                client_addr = Some(source);
                primary
            };
            proxy.send_to(&buffer[..len], target).await.unwrap();
        }
    });
    let client_tls = TlsConfig::new(SecretKey::generate(), 8, tls::default_provider());
    let mut client_config =
        noq::ClientConfig::new(Arc::new(client_tls.make_client_config(false).unwrap()));
    client_config.transport_config(transport);
    let client = noq::Endpoint::client("127.0.0.1:0".parse().unwrap()).unwrap();
    client.set_default_client_config(client_config);
    let mut clients = Vec::new();
    let mut servers = Vec::new();
    let mut backups = Vec::new();
    for _ in 0..2 {
        let (dialed, accepted) = tokio::join!(
            client
                .connect(primary, &tls::name::encode(server_key.public()))
                .unwrap(),
            async { server.accept().await.unwrap().await.unwrap() }
        );
        let dialed = dialed.unwrap();
        let backup_path = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                match dialed.open_path(backup, PathStatus::Backup).await {
                    Err(PathError::RemoteCidsExhausted) => {
                        tokio::time::sleep(Duration::from_millis(10)).await
                    }
                    result => break result.unwrap(),
                }
            }
        })
        .await
        .unwrap();
        backups.push(backup_path);
        clients.push(dialed);
        servers.push(accepted);
    }
    let direct = Watchable::new(BTreeSet::new());
    let metrics = EndpointMetrics::default();
    let mut actor = RemoteStateActor::new(
        server_key.public(),
        direct.watch(),
        Default::default(),
        Default::default(),
        metrics.socket.clone(),
        Default::default(),
        Default::default(),
    );
    let addr = transports::Addr::Ip(primary);
    actor.selected_path = Some(addr.clone());
    actor
        .paths
        .insert_open_path(addr.clone(), Source::Connection);
    for conn in &clients {
        let (path_state, _receiver) = PathStateSender::new();
        let mut paths = FxHashMap::default();
        // Only the shared primary needs tracking for this event. The real backup
        // keeps close(primary) legal, exposing the cross-connection close bug.
        paths.insert(PathId::ZERO, addr.clone());
        actor.connections.insert(
            ConnId(conn.stable_id()),
            ConnectionState {
                handle: conn.weak_handle(),
                path_state,
                paths,
                has_been_direct: true,
            },
        );
    }
    clients[0].path(PathId::ZERO).unwrap().close().unwrap();
    let mut sibling_events = clients[1].path_events();
    actor.handle_path_event(
        ConnId(clients[0].stable_id()),
        Ok(NoqPathEvent::Abandoned {
            id: PathId::ZERO,
            reason: noq_proto::PathAbandonReason::UnusableAfterNetworkChange,
        }),
    );
    let abandoned = tokio::time::timeout(Duration::from_millis(100), async {
        while let Some(event) = sibling_events.next().await {
            if matches!(
                event,
                Ok(NoqPathEvent::Abandoned {
                    id: PathId::ZERO,
                    ..
                })
            ) {
                return;
            }
        }
        panic!("healthy connection event stream ended");
    })
    .await;
    assert!(
        abandoned.is_err(),
        "abandonment on the old connection closed the healthy connection's primary path"
    );
    // The old connection retains a low RTT for the proxy route after that route
    // stops forwarding. The new connection still has a working direct route.
    let backup_addr = transports::Addr::Ip(backup);
    actor
        .connections
        .get_mut(&ConnId(clients[0].stable_id()))
        .unwrap()
        .add_open_path(backup_addr.clone(), backups[0].id(), &metrics.socket);
    let new_state = actor
        .connections
        .get_mut(&ConnId(clients[1].stable_id()))
        .unwrap();
    new_state.add_open_path(addr.clone(), PathId::ZERO, &metrics.socket);
    new_state.add_open_path(backup_addr.clone(), backups[1].id(), &metrics.socket);
    new_state.path_state.record_selected(addr.clone().into());
    actor.selected_path = Some(backup_addr.clone());
    forwarding.store(false, std::sync::atomic::Ordering::Relaxed);
    let new_primary = clients[1].path(PathId::ZERO).unwrap();
    actor.configure_path(&new_primary, &addr);
    actor.configure_path(&backups[1], &backup_addr);
    actor.select_path();
    let redundant_abandoned = tokio::time::timeout(Duration::from_millis(100), async {
        while let Some(event) = sibling_events.next().await {
            if matches!(event, Ok(NoqPathEvent::Abandoned { id, .. }) if id == backups[1].id()) {
                return;
            }
        }
        panic!("healthy connection event stream ended");
    })
    .await;
    assert!(
        redundant_abandoned.is_ok(),
        "a redundant path must be retired even when the selected path is unchanged"
    );
    assert_eq!(
        new_primary.status().unwrap(),
        PathStatus::Available,
        "the old connection's selected address demoted the new connection's working path"
    );
    tokio::time::timeout(Duration::from_millis(500), async {
        let (sent, received) = tokio::join!(
            async {
                let mut stream = clients[1].open_uni().await.unwrap();
                stream.write_all(b"working replacement path").await.unwrap();
                stream.finish().unwrap();
            },
            async {
                servers[1]
                    .accept_uni()
                    .await
                    .unwrap()
                    .read_to_end(128)
                    .await
                    .unwrap()
            }
        );
        let () = sent;
        assert_eq!(received, b"working replacement path");
    })
    .await
    .expect("new connection waited for the old route to time out");
    client.close(0u32.into(), b"test complete");
    server.close(0u32.into(), b"test complete");
    drop(_guard);
    proxy_task.await.unwrap();
}
