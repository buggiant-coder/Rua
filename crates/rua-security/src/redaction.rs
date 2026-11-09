use regex::Regex;

pub fn redact_secrets(input: &str) -> String {
    let mut result = input.to_string();

    if let Ok(re) = Regex::new(r"(?i)(sk-[a-zA-Z0-9]{20,})") {
        result = re.replace_all(&result, "sk-***").to_string();
    }

    if let Ok(re) = Regex::new(r#"(?i)(api[_-]?key[=:]\s*)['"]?([a-zA-Z0-9_\-]{16,})['"]?"#) {
        result = re.replace_all(&result, "${1}***").to_string();
    }

    if let Ok(re) = Regex::new(r"(?i)(bearer\s+)([a-zA-Z0-9_\-\.]{20,})") {
        result = re.replace_all(&result, "${1}***").to_string();
    }

    if let Ok(re) = Regex::new(r"(AKIA[0-9A-Z]{16})") {
        result = re.replace_all(&result, "AKIA***").to_string();
    }

    if let Ok(re) = Regex::new(r"(://[^:]+:)([^@]+)(@)") {
        result = re.replace_all(&result, "${1}***${3}").to_string();
    }

    if let Ok(re) = Regex::new(
        r#"(?i)(password|secret|token|api_key|apikey)\s*[=:]\s*['"]?([^\s'"}{,]{8,})['"]?"#,
    ) {
        result = re.replace_all(&result, "${1}=***").to_string();
    }

    result
}

pub fn redact_path(path: &str) -> String {
    if let Some(pos) = path.rfind('/') {
        let filename = &path[pos + 1..];
        if filename.is_empty() {
            ".../<redacted>/".into()
        } else {
            format!(".../<redacted>/{}", filename)
        }
    } else {
        path.to_string()
    }
}

pub fn truncate_value(value: &str, max_len: usize) -> String {
    if value.chars().count() <= max_len {
        value.to_string()
    } else {
        format!("{}...", value.chars().take(max_len).collect::<String>())
    }
}
