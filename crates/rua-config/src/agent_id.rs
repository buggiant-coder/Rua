use std::collections::HashSet;

const AGENT_ID_MIN_LENGTH: usize = 2;
const AGENT_ID_MAX_LENGTH: usize = 64;

static RESERVED_AGENT_IDS: &[&str] = &["default"];

pub fn generate_short_agent_id() -> String {
    let u = uuid::Uuid::new_v4();
    u.simple().to_string()[..12].to_string()
}

pub fn sanitize_agent_id(raw: &str) -> String {
    raw.trim().to_string()
}

pub fn validate_agent_id(agent_id: &str, existing_ids: &HashSet<String>) -> Result<(), String> {
    if agent_id.len() < AGENT_ID_MIN_LENGTH {
        return Err(format!(
            "Agent ID must be at least {} characters, got {}.",
            AGENT_ID_MIN_LENGTH,
            agent_id.len()
        ));
    }
    if agent_id.len() > AGENT_ID_MAX_LENGTH {
        return Err(format!(
            "Agent ID must be at most {} characters, got {}.",
            AGENT_ID_MAX_LENGTH,
            agent_id.len()
        ));
    }

    let bytes = agent_id.as_bytes();
    let is_alnum = |b: u8| b.is_ascii_alphanumeric();
    let is_valid_mid = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'-';

    if !is_alnum(bytes[0]) || !is_alnum(bytes[bytes.len() - 1]) {
        return Err(format!(
            "Agent ID '{}' contains invalid characters. \
             Only letters, digits, hyphens, and underscores \
             are allowed. Cannot start or end with '-' or '_'.",
            agent_id
        ));
    }
    if !bytes.iter().all(|&b| is_valid_mid(b)) {
        return Err(format!(
            "Agent ID '{}' contains invalid characters. \
             Only letters, digits, hyphens, and underscores \
             are allowed. Cannot start or end with '-' or '_'.",
            agent_id
        ));
    }

    if RESERVED_AGENT_IDS.contains(&agent_id) {
        return Err(format!(
            "Agent ID '{}' is reserved and cannot be used.",
            agent_id
        ));
    }

    if existing_ids.contains(agent_id) {
        return Err(format!("Agent ID '{}' already exists.", agent_id));
    }

    Ok(())
}
