use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

const SIGNED_HEADER_NAMES: &[&str] = &[
    "host",
    "x-acs-action",
    "x-acs-date",
    "x-acs-signature-nonce",
    "x-acs-version",
];

pub struct SignInput<'a> {
    pub method: &'a str,
    pub uri: &'a str,
    pub query_params: &'a [(&'a str, &'a str)],
    pub host: &'a str,
    pub action: &'a str,
    pub version: &'a str,
    pub date: &'a str,
    pub nonce: &'a str,
    pub body: &'a [u8],
    pub access_key_id: &'a str,
    pub access_key_secret: &'a str,
}

pub fn sign(input: &SignInput<'_>) -> String {
    let sorted_query = build_sorted_query(input.query_params);
    let canonical_headers = build_canonical_headers(
        input.host,
        input.action,
        input.date,
        input.nonce,
        input.version,
    );
    let signed_headers = SIGNED_HEADER_NAMES.join(";");
    let body_hash = sha256_hex(input.body);

    let canonical_request = format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        input.method, input.uri, sorted_query, canonical_headers, signed_headers, body_hash,
    );

    let string_to_sign = format!(
        "ACS3-HMAC-SHA256\n{}",
        sha256_hex(canonical_request.as_bytes())
    );

    let signature = hmac_sha256_hex(
        input.access_key_secret.as_bytes(),
        string_to_sign.as_bytes(),
    );

    format!(
        "ACS3-HMAC-SHA256 Credential={},SignedHeaders={},Signature={}",
        input.access_key_id, signed_headers, signature,
    )
}

fn build_sorted_query(params: &[(&str, &str)]) -> String {
    let mut sorted: Vec<(&str, &str)> = params.to_vec();
    sorted.sort_by_key(|(k, _)| *k);
    sorted
        .iter()
        .map(|(k, v)| format!("{}={}", percent_encode(k), percent_encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

fn build_canonical_headers(
    host: &str,
    action: &str,
    date: &str,
    nonce: &str,
    version: &str,
) -> String {
    let mut out = String::new();
    for &name in SIGNED_HEADER_NAMES {
        let value = match name {
            "host" => host,
            "x-acs-action" => action,
            "x-acs-date" => date,
            "x-acs-signature-nonce" => nonce,
            "x-acs-version" => version,
            _ => "",
        };
        out.push_str(name);
        out.push(':');
        out.push_str(value.trim());
        out.push('\n');
    }
    out
}

fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

fn hmac_sha256_hex(key: &[u8], data: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(data);
    hex::encode(mac.finalize().into_bytes())
}

fn percent_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for &b in input.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push('%');
            out.push_str(&format!("{b:02X}"));
        }
    }
    out
}
