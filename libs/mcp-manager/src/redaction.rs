use std::collections::BTreeMap;

pub fn mask_secret(value: &str) -> String {
    if value.is_empty() {
        return String::new();
    }
    if value.len() <= 6 {
        return "****".to_string();
    }
    let prefix = &value[..2];
    let suffix = &value[value.len() - 4..];
    format!("{prefix}****{suffix}")
}

pub fn is_masked(value: &str) -> bool {
    value == "****" || (value.contains("****") && value.len() <= 10)
}

pub fn mask_env_map(env: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    env.iter()
        .map(|(k, v)| (k.clone(), mask_secret(v)))
        .collect()
}

pub fn mask_headers(headers: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    headers
        .iter()
        .map(|(k, v)| {
            let masked = if k.to_lowercase().contains("auth")
                || k.to_lowercase().contains("key")
                || k.to_lowercase().contains("token")
                || k.to_lowercase().contains("secret")
            {
                mask_secret(v)
            } else {
                v.clone()
            };
            (k.clone(), masked)
        })
        .collect()
}

pub fn restore_original_values(
    submitted: &BTreeMap<String, String>,
    original: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    submitted
        .iter()
        .map(|(k, submitted_v)| {
            let restored = if let Some(original_v) = original.get(k) {
                if *submitted_v == mask_secret(original_v) {
                    original_v.clone()
                } else {
                    submitted_v.clone()
                }
            } else {
                submitted_v.clone()
            };
            (k.clone(), restored)
        })
        .collect()
}

pub fn redact_for_log(message: &str, secrets: &[&str]) -> String {
    let mut result = message.to_string();
    for secret in secrets {
        if !secret.is_empty() && result.contains(*secret) {
            result = result.replace(*secret, &mask_secret(secret));
        }
    }
    result
}
