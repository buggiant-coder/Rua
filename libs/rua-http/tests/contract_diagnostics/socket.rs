use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};

use rua_http::diagnostics::{SocketCounters, SocketSampler};

fn pair(address: &str) -> (TcpStream, TcpStream) {
    let listener = TcpListener::bind(address).unwrap();
    let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let peer = listener.accept().unwrap().0;
    for socket in [&client, &peer] {
        socket
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        socket
            .set_write_timeout(Some(Duration::from_secs(3)))
            .unwrap();
    }
    (client, peer)
}

fn sample(sampler: &mut SocketSampler, socket: &TcpStream) -> SocketCounters {
    let reading = sampler.sample(socket.local_addr().unwrap(), socket.peer_addr().unwrap());
    assert_eq!(reading.status, "observed", "{reading:?}");
    reading.counters.unwrap()
}

fn wait_for(mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while !check() {
        assert!(
            Instant::now() < deadline,
            "socket observation did not converge"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn loopback_counters_match_known_payloads_in_both_address_families() {
    for address in ["127.0.0.1:0", "[::1]:0"] {
        let (mut client, mut peer) = pair(address);
        let mut sampler = SocketSampler::default();
        let before = sample(&mut sampler, &client);
        let sent = vec![0x3a; 12_345];
        let received = vec![0x5c; 6_789];
        client.write_all(&sent).unwrap();
        let mut buffer = vec![0; sent.len()];
        peer.read_exact(&mut buffer).unwrap();
        assert_eq!(buffer, sent);
        peer.write_all(&received).unwrap();
        buffer.resize(received.len(), 0);
        client.read_exact(&mut buffer).unwrap();
        assert_eq!(buffer, received);
        let after = sample(&mut sampler, &client);
        // These exact deltas also guard the Darwin C bitfield/counter ABI.
        assert_eq!(after.tx_bytes - before.tx_bytes, sent.len() as u64);
        assert_eq!(after.rx_bytes - before.rx_bytes, received.len() as u64);
        assert!(after.tx_packets > before.tx_packets);
        assert!(after.rx_packets > before.rx_packets);
        assert!(after.tx_retransmit_bytes <= after.tx_bytes);
        assert_eq!(after.tcp_state, 4); // TCPS_ESTABLISHED
    }
}

#[test]
fn empty_send_queue_does_not_mean_the_remote_application_read_the_request() {
    let (mut client, mut peer) = pair("127.0.0.1:0");
    let payload = vec![0x7d; 8192];
    client.write_all(&payload).unwrap();
    let mut sender = SocketSampler::default();
    let mut receiver = SocketSampler::default();
    wait_for(|| sample(&mut sender, &client).send_queue_bytes == 0);
    // The TCP peer acknowledged the bytes, but its application has read none.
    assert_eq!(
        sample(&mut receiver, &peer).receive_queue_bytes,
        Some(payload.len() as u32)
    );
    let mut buffer = vec![0; payload.len()];
    peer.read_exact(&mut buffer).unwrap();
    assert_eq!(buffer, payload, "sampling must not consume socket data");
    assert_eq!(sample(&mut receiver, &peer).receive_queue_bytes, Some(0));
}

#[test]
fn stalled_reader_leaves_observable_bytes_in_the_send_buffer() {
    let (mut client, mut peer) = pair("127.0.0.1:0");
    client.set_nonblocking(true).unwrap();
    let block = [0xab; 16_384];
    let mut sent = 0;
    loop {
        match client.write(&block) {
            Ok(count) => {
                sent += count;
                assert!(
                    sent < 64 * 1024 * 1024,
                    "expected backpressure from unread peer"
                );
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(error) => panic!("{error}"),
        }
    }
    let mut sampler = SocketSampler::default();
    assert!(sample(&mut sampler, &client).send_queue_bytes > 0);
    let mut buffer = vec![0; sent];
    peer.read_exact(&mut buffer).unwrap();
    assert!(buffer.iter().all(|byte| *byte == 0xab));
    wait_for(|| sample(&mut sampler, &client).send_queue_bytes == 0);
}

#[test]
fn cached_descriptor_is_revalidated_and_never_keeps_a_socket_alive() {
    let (client, mut peer) = pair("127.0.0.1:0");
    let local = client.local_addr().unwrap();
    let remote = client.peer_addr().unwrap();
    let mut sampler = SocketSampler::default();
    sample(&mut sampler, &client);
    drop(client);
    assert_eq!(
        peer.read(&mut [0; 1]).unwrap(),
        0,
        "the sampler retained an open duplicate fd"
    );
    let (mut other, mut other_peer) = pair("127.0.0.1:0");
    other.write_all(&[0; 500]).unwrap();
    other_peer.read_exact(&mut [0; 500]).unwrap();
    let absent = sampler.sample(local, remote);
    assert_eq!(absent.status, "socket_not_found");
    assert!(
        absent.counters.is_none(),
        "closed/reused fd was attributed to the old socket"
    );
    assert_eq!(sample(&mut sampler, &other).tx_bytes, 500);
}
