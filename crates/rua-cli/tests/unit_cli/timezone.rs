use rua_cli::timezone::{local_tz, local_tz_name};

#[test]
fn local_tz_returns_valid_timezone() {
    let tz = local_tz();
    let name = tz.name();
    assert!(!name.is_empty());
}

#[test]
fn local_tz_name_returns_string() {
    let name = local_tz_name();
    assert!(!name.is_empty());
}

#[test]
fn tz_env_override_wins_over_system_timezone() {
    let old = std::env::var_os("TZ");

    unsafe {
        std::env::set_var("TZ", "Europe/London");
    }
    let tz = local_tz();
    unsafe {
        match old {
            Some(value) => std::env::set_var("TZ", value),
            None => std::env::remove_var("TZ"),
        }
    }

    assert_eq!(tz, chrono_tz::Europe::London);
}
