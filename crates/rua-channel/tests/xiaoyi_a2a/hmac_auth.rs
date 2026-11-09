use rua_channel::xiaoyi::{generate_auth_headers, generate_signature};

#[test]
fn signature_is_deterministic() {
    let sig1 = generate_signature("secret-abc", "1717401600000");
    let sig2 = generate_signature("secret-abc", "1717401600000");
    assert_eq!(sig1, sig2);
    assert!(!sig1.is_empty());
}

#[test]
fn different_keys_produce_different_signatures() {
    let sig1 = generate_signature("key-A", "1717401600000");
    let sig2 = generate_signature("key-B", "1717401600000");
    assert_ne!(sig1, sig2);
}

#[test]
fn different_timestamps_produce_different_signatures() {
    let sig1 = generate_signature("same-key", "1000");
    let sig2 = generate_signature("same-key", "2000");
    assert_ne!(sig1, sig2);
}

#[test]
fn auth_headers_contain_all_required_fields() {
    let headers = generate_auth_headers("ak-test", "sk-test", "agent-001");
    let keys: Vec<&str> = headers.iter().map(|(k, _)| k.as_str()).collect();
    assert!(keys.contains(&"x-access-key"), "missing x-access-key");
    assert!(keys.contains(&"x-sign"), "missing x-sign");
    assert!(keys.contains(&"x-ts"), "missing x-ts");
    assert!(keys.contains(&"x-agent-id"), "missing x-agent-id");
}

#[test]
fn auth_headers_values_match_input() {
    let headers = generate_auth_headers("my-ak", "my-sk", "my-agent");
    let ak = headers.iter().find(|(k, _)| k == "x-access-key").unwrap();
    assert_eq!(ak.1, "my-ak");
    let agent_id = headers.iter().find(|(k, _)| k == "x-agent-id").unwrap();
    assert_eq!(agent_id.1, "my-agent");
    let ts = headers.iter().find(|(k, _)| k == "x-ts").unwrap();
    assert!(!ts.1.is_empty());
    let sign = headers.iter().find(|(k, _)| k == "x-sign").unwrap();
    assert!(!sign.1.is_empty());
}
