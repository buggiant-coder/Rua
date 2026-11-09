use std::ffi::OsString;
use std::sync::{Mutex, OnceLock};

use rua_config::dirs::Paths;
use rua_doctor::mcp_client_check::{check_stdio_command, check_streamable_http_url};
use rua_doctor::registry::{run_all_checks, CheckContext, CheckStatus};
use rua_doctor::security_baseline::evaluate_security;
use rua_doctor::workspace_hygiene::{count_dir_entries, sum_prompt_file_sizes};

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

struct EnvGuard {
    path: Option<OsString>,
    pathext: Option<OsString>,
}

impl EnvGuard {
    fn capture() -> Self {
        Self {
            path: std::env::var_os("PATH"),
            pathext: std::env::var_os("PATHEXT"),
        }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        unsafe {
            match &self.path {
                Some(value) => std::env::set_var("PATH", value),
                None => std::env::remove_var("PATH"),
            }
            match &self.pathext {
                Some(value) => std::env::set_var("PATHEXT", value),
                None => std::env::remove_var("PATHEXT"),
            }
        }
    }
}

#[test]
fn security_baseline_registered() {
    let paths = Paths::new(Some("/tmp/rua-doctor-sec-test"));
    let ctx = CheckContext { paths: &paths };
    let results = run_all_checks(&ctx);
    assert!(
        results.iter().any(|r| r.name == "security_baseline"),
        "security_baseline check must appear"
    );
}

#[test]
fn mcp_client_registered() {
    let paths = Paths::new(Some("/tmp/rua-doctor-mcp-test"));
    let ctx = CheckContext { paths: &paths };
    let results = run_all_checks(&ctx);
    assert!(
        results.iter().any(|r| r.name == "mcp_client"),
        "mcp_client check must appear"
    );
}

#[test]
fn workspace_hygiene_registered() {
    let paths = Paths::new(Some("/tmp/rua-doctor-hyg-test"));
    let ctx = CheckContext { paths: &paths };
    let results = run_all_checks(&ctx);
    assert!(
        results.iter().any(|r| r.name == "workspace_hygiene"),
        "workspace_hygiene check must appear"
    );
}

#[test]
fn security_all_enabled_pass() {
    let (status, _) = evaluate_security(true, true, "warn");
    assert_eq!(status, CheckStatus::Pass);
}

#[test]
fn security_tool_guard_off_warn() {
    let (status, msg) = evaluate_security(false, true, "warn");
    assert_eq!(status, CheckStatus::Warn);
    assert!(msg.contains("tool_guard"));
}

#[test]
fn security_scanner_off_warn() {
    let (status, msg) = evaluate_security(true, true, "off");
    assert_eq!(status, CheckStatus::Warn);
    assert!(msg.contains("skill_scanner"));
}

#[test]
fn security_file_guard_off_warn() {
    let (status, msg) = evaluate_security(true, false, "warn");
    assert_eq!(status, CheckStatus::Warn);
    assert!(msg.contains("file_guard"));
}

#[test]
fn security_scanner_empty_warn() {
    let (status, msg) = evaluate_security(true, true, "");
    assert_eq!(status, CheckStatus::Warn);
    assert!(msg.contains("skill_scanner"));
}

#[test]
fn security_multiple_disabled_items_are_reported_together() {
    let (status, msg) = evaluate_security(false, false, "off");
    assert_eq!(status, CheckStatus::Warn);
    assert!(msg.contains("tool_guard"));
    assert!(msg.contains("file_guard"));
    assert!(msg.contains("skill_scanner"));
}

#[test]
fn mcp_stdio_command_on_path_ok() {
    let _guard = env_lock().lock().expect("env lock");
    let _env = EnvGuard::capture();
    let dir = tempfile::tempdir().expect("temp PATH dir");
    let file_name = if cfg!(windows) {
        "rua-doctor-fake-cmd.exe"
    } else {
        "rua-doctor-fake-cmd"
    };
    std::fs::write(dir.path().join(file_name), "").expect("write fake command");
    unsafe {
        std::env::set_var("PATH", dir.path());
        std::env::set_var("PATHEXT", ".EXE");
    }

    assert!(check_stdio_command("rua-doctor-fake-cmd --version").is_ok());
}

#[test]
fn mcp_stdio_empty_err() {
    assert!(check_stdio_command("").is_err());
}

#[test]
fn mcp_stdio_missing_absolute_path_err() {
    assert!(check_stdio_command("/nonexistent/rua-doctor-test-binary").is_err());
}

#[test]
fn mcp_http_valid_ok() {
    assert!(check_streamable_http_url("https://mcp.example.com/v1").is_ok());
}

#[test]
fn mcp_http_empty_err() {
    assert!(check_streamable_http_url("").is_err());
}

#[test]
fn mcp_http_bad_scheme_err() {
    assert!(check_streamable_http_url("ftp://bad").is_err());
}

#[test]
fn mcp_http_no_host_err() {
    assert!(check_streamable_http_url("http://").is_err());
}

#[test]
fn hygiene_count_nonexistent_zero() {
    assert_eq!(
        count_dir_entries(std::path::Path::new("/tmp/no-such-dir-xyz")),
        0
    );
}

#[test]
fn hygiene_count_empty_dir_matches_read_dir_count() {
    let tmp = tempfile::tempdir().expect("tempdir");
    assert_eq!(
        count_dir_entries(tmp.path()),
        std::fs::read_dir(tmp.path()).unwrap().count()
    );
}

#[test]
fn hygiene_sum_missing_files_zero() {
    let total = sum_prompt_file_sizes(
        &std::path::PathBuf::from("/tmp"),
        &["no1.md".into(), "no2.md".into()],
    );
    assert_eq!(total, 0);
}

#[test]
fn normal_config_all_pass() {
    let tmp = std::env::temp_dir().join("rua-doctor-normal-sec");
    let _ = std::fs::create_dir_all(&tmp);

    let config = serde_json::json!({
        "security": {
            "tool_guard": { "enabled": true },
            "file_guard": { "enabled": true },
            "skill_scanner": { "mode": "warn" }
        },
        "mcp": { "clients": {} },
        "agents": {
            "active_agent": "default",
            "profiles": {}
        },
        "channels": {}
    });
    std::fs::write(
        tmp.join("config.json"),
        serde_json::to_string_pretty(&config).unwrap(),
    )
    .unwrap();

    let paths = Paths::new(Some(tmp.to_str().unwrap()));
    let ctx = CheckContext { paths: &paths };
    let results = run_all_checks(&ctx);

    for name in &["security_baseline", "mcp_client", "workspace_hygiene"] {
        let r = results.iter().find(|r| r.name == *name);
        assert!(r.is_some(), "{name} check must appear");
        assert_eq!(
            r.unwrap().status,
            CheckStatus::Pass,
            "{name} should pass with normal config"
        );
    }

    let _ = std::fs::remove_dir_all(&tmp);
}
