use std::env;

pub fn detected_user_timezone() -> String {
    env::var("TZ")
        .ok()
        .and_then(|value| normalize_detected_timezone(&value))
        .or_else(|| {
            iana_time_zone::get_timezone()
                .ok()
                .and_then(|value| normalize_detected_timezone(&value))
        })
        .unwrap_or_else(|| "UTC".to_string())
}

fn normalize_detected_timezone(value: &str) -> Option<String> {
    let value = value.trim();
    if value == "UTC" || value.contains('/') {
        Some(value.to_string())
    } else {
        None
    }
}
