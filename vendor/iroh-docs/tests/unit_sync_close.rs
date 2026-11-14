//! Protocol completion must release the peer before transport shutdown finishes.
use super::*;
use crate::{actor::OpenOpts, engine::LiveEvent, protocol::Docs, Capability, NamespaceSecret};
use iroh::{endpoint::presets, protocol::Router};
use n0_future::StreamExt;

#[tokio::test]
async fn completed_sync_does_not_wait_for_the_previous_peers_fin() -> anyhow::Result<()> {
    let server = Endpoint::builder(presets::Minimal)
        .clear_ip_transports()
        .bind_addr("127.0.0.1:0")?
        .bind()
        .await?;
    let client = Endpoint::builder(presets::Minimal)
        .clear_ip_transports()
        .bind_addr("127.0.0.1:0")?
        .bind()
        .await?;
    let blobs = iroh_blobs::store::mem::MemStore::new();
    let gossip = iroh_gossip::net::Gossip::builder().spawn(server.clone());
    let docs = Docs::memory()
        .spawn(server.clone(), (*blobs).clone(), gossip.clone())
        .await?;
    let router = Router::builder(server.clone())
        .accept(ALPN, docs.clone())
        .accept(iroh_gossip::ALPN, gossip)
        .spawn();
    let namespace = NamespaceSecret::new(&mut rand::rng());
    let doc = docs
        .api()
        .import_namespace(Capability::Write(namespace.clone()))
        .await?;
    doc.start_sync(vec![]).await?;
    let mut events = doc.subscribe().await?;
    let mut store = crate::store::Store::memory();
    let replica = store.new_replica(namespace.clone())?;
    drop(replica);
    store.close_replica(namespace.id());
    let handle = SyncHandle::spawn(store, None, "close-regression".into());
    handle
        .open(namespace.id(), OpenOpts::default().sync())
        .await?;
    let addr = EndpointAddr::new(server.id()).with_ip_addr(server.bound_sockets()[0]);
    let conn = client.connect(addr.clone(), ALPN).await?;
    let (mut send, mut recv) = conn.open_bi().await?;
    tokio::time::timeout(
        Duration::from_secs(2),
        codec::run_alice(&mut send, &mut recv, &handle, namespace.id(), server.id()),
    )
    .await??;
    // Keep the client write half open: reconciliation is complete, but the
    // server's graceful transport close cannot receive this client's FIN.
    let finished = tokio::time::timeout(Duration::from_millis(500), async {
        while let Some(event) = events.next().await {
            if let LiveEvent::SyncFinished(event) = event? {
                if event.peer == client.id() && event.result.is_ok() {
                    return Ok::<_, anyhow::Error>(());
                }
            }
        }
        anyhow::bail!("event stream ended before protocol completion")
    })
    .await;
    assert!(
        finished.is_ok(),
        "completed protocol remained busy waiting for peer FIN"
    );
    finished??;
    // A new sync from the same peer must now be admitted, not AlreadySyncing.
    tokio::time::timeout(
        Duration::from_secs(1),
        connect_and_sync(&client, &handle, namespace.id(), addr, None),
    )
    .await??;
    send.finish()?;
    drop((send, recv, conn));
    client.close().await;
    router.shutdown().await?;
    handle.shutdown().await?;
    Ok(())
}
