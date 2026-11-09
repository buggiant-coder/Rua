use rua_sync::wormhole::test_relay;

#[tokio::test]
async fn roundtrip_encrypt_decrypt() {
    let key = vec![42u8; 32];
    let (ch_a, mut ch_b) = test_relay::local_pair(key);

    let msg = b"hello from device A";
    ch_a.send_encrypted(msg).await.unwrap();

    let received = ch_b.recv_encrypted().await.unwrap();
    assert_eq!(received, msg);
}

#[tokio::test]
async fn bidirectional_messages() {
    let key = vec![99u8; 32];
    let (ch_a, mut ch_b) = test_relay::local_pair(key);

    ch_a.send_encrypted(b"ping").await.unwrap();
    let r1 = ch_b.recv_encrypted().await.unwrap();
    assert_eq!(r1, b"ping");

    ch_b.send_encrypted(b"pong").await.unwrap();
}

#[tokio::test]
async fn large_payload() {
    let key = vec![7u8; 32];
    let (ch_a, mut ch_b) = test_relay::local_pair(key);

    let big_msg = vec![0xAB; 64 * 1024];
    ch_a.send_encrypted(&big_msg).await.unwrap();

    let received = ch_b.recv_encrypted().await.unwrap();
    assert_eq!(received, big_msg);
}
