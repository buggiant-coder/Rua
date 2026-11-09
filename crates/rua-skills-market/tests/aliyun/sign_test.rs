use rua_skills_market::providers::aliyun_sign::{sign, SignInput};

#[test]
fn known_input_produces_deterministic_authorization() {
    let input = SignInput {
        method: "GET",
        uri: "/openapi/skills",
        query_params: &[("keyword", "weather"), ("maxResults", "10")],
        host: "agentexplorer.aliyuncs.com",
        action: "SearchSkills",
        version: "2026-03-17",
        date: "2026-06-08T12:00:00Z",
        nonce: "test-nonce-1234",
        body: b"",
        access_key_id: "LTAI5tTestKeyId",
        access_key_secret: "TestSecretKeyValue",
    };
    let auth = sign(&input);

    assert!(auth.starts_with("ACS3-HMAC-SHA256 "));
    assert!(auth.contains("Credential=LTAI5tTestKeyId,"));
    assert!(auth.contains(
        "SignedHeaders=host;x-acs-action;x-acs-date;x-acs-signature-nonce;x-acs-version,"
    ));
    assert!(auth.contains("Signature="));

    let sig_start = auth.find("Signature=").unwrap() + "Signature=".len();
    let signature = &auth[sig_start..];
    assert_eq!(signature.len(), 64, "HMAC-SHA256 hex is 64 chars");
    assert!(
        signature.chars().all(|c| c.is_ascii_hexdigit()),
        "signature must be hex"
    );
}

#[test]
fn same_input_same_signature() {
    let make = || SignInput {
        method: "GET",
        uri: "/openapi/skills",
        query_params: &[("keyword", "test")],
        host: "agentexplorer.aliyuncs.com",
        action: "SearchSkills",
        version: "2026-03-17",
        date: "2026-06-08T00:00:00Z",
        nonce: "fixed-nonce",
        body: b"",
        access_key_id: "AK",
        access_key_secret: "SK",
    };
    assert_eq!(sign(&make()), sign(&make()));
}

#[test]
fn different_secret_different_signature() {
    let base = || SignInput {
        method: "GET",
        uri: "/openapi/skills",
        query_params: &[],
        host: "agentexplorer.aliyuncs.com",
        action: "SearchSkills",
        version: "2026-03-17",
        date: "2026-06-08T00:00:00Z",
        nonce: "n",
        body: b"",
        access_key_id: "AK",
        access_key_secret: "secret-a",
    };
    let a = sign(&base());

    let mut input_b = base();
    input_b.access_key_secret = "secret-b";
    let b = sign(&input_b);

    assert_ne!(a, b);
}

#[test]
fn query_params_sorted_by_key() {
    let input_a = SignInput {
        method: "GET",
        uri: "/openapi/skills",
        query_params: &[("z_param", "1"), ("a_param", "2")],
        host: "h",
        action: "A",
        version: "V",
        date: "D",
        nonce: "N",
        body: b"",
        access_key_id: "AK",
        access_key_secret: "SK",
    };
    let input_b = SignInput {
        query_params: &[("a_param", "2"), ("z_param", "1")],
        ..input_a
    };
    assert_eq!(sign(&input_a), sign(&input_b));
}
