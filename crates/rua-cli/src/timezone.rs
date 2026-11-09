use chrono_tz::Tz;

pub fn local_tz() -> Tz {
    if let Ok(tz_str) = std::env::var("TZ") {
        if let Ok(tz) = tz_str.parse::<Tz>() {
            return tz;
        }
    }

    match iana_time_zone::get_timezone() {
        Ok(tz_str) => match tz_str.parse::<Tz>() {
            Ok(tz) => tz,
            Err(_) => {
                eprintln!(
                    "warning: cannot parse system timezone '{}', using UTC",
                    tz_str
                );
                chrono_tz::UTC
            }
        },
        Err(_) => {
            eprintln!("warning: cannot detect system timezone, using UTC");
            chrono_tz::UTC
        }
    }
}

pub fn local_tz_name() -> String {
    local_tz().name().to_string()
}
