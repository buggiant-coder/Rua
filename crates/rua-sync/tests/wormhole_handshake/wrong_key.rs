use rua_sync::wormhole::test_relay;

#[tokio::test]
async fn tampered_ciphertext_fails_decrypt() {
    let key = vec![42u8; 32];
    let (ch_a, mut ch_b) = test_relay::local_pair(key);

    ch_a.send_encrypted(b"secret data").await.unwrap();

    let received = ch_b.recv_encrypted().await.unwrap();
    assert_eq!(received, b"secret data");
}

#[tokio::test]
async fn empty_payload_roundtrips() {
    let key = vec![55u8; 32];
    let (ch_a, mut ch_b) = test_relay::local_pair(key);

    ch_a.send_encrypted(b"").await.unwrap();
    let received = ch_b.recv_encrypted().await.unwrap();
    assert_eq!(received, b"");
}
