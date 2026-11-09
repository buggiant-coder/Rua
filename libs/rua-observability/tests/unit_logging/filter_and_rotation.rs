use std::io::Write;
use std::sync::{Arc, Mutex};

use rua_observability::{LogConfig, SizeRotatingAppender};
use tracing_subscriber::layer::SubscriberExt;

#[derive(Clone)]
struct TestWriter(Arc<Mutex<Vec<String>>>);

impl Write for TestWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap()
            .push(String::from_utf8_lossy(buf).to_string());
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for TestWriter {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

fn capture_with_filter<F>(config: &LogConfig, emit: F) -> String
where
    F: FnOnce(),
{
    capture_with_env_filter(rua_observability::build_env_filter(config), emit)
}

fn capture_with_env_filter<F>(filter: tracing_subscriber::EnvFilter, emit: F) -> String
where
    F: FnOnce(),
{
    let captured = Arc::new(Mutex::new(Vec::<String>::new()));
    let writer = TestWriter(captured.clone());
    let subscriber = tracing_subscriber::registry().with(filter).with(
        tracing_subscriber::fmt::layer()
            .with_writer(move || writer.clone())
            .with_ansi(false),
    );

    tracing::subscriber::with_default(subscriber, emit);

    let output = captured.lock().unwrap().join("");
    output
}

#[test]
fn default_filter_keeps_rua_info_and_dependency_warnings() {
    let filter = rua_observability::build_env_filter_with_rust_log(&LogConfig::default(), None);
    let output = capture_with_env_filter(filter, || {
        tracing::info!(target: "hyper", "hyper info should be quiet");
        tracing::warn!(target: "hyper", "hyper warn should be visible");
        tracing::info!(target: "rua_server", "rua info should be visible");
    });

    assert!(!output.contains("hyper info should be quiet"));
    assert!(output.contains("hyper warn should be visible"));
    assert!(output.contains("rua info should be visible"));
}

#[test]
fn configured_log_level_opens_rua_debug_but_keeps_noisy_deps_quiet() {
    let config = LogConfig {
        log_level: Some("debug".into()),
        ..Default::default()
    };

    let output = capture_with_filter(&config, || {
        tracing::debug!(target: "rua_agent", "rua debug should be visible");

        tracing::debug!(target: "hyper", "hyper debug should stay quiet");
        tracing::debug!(target: "iroh::net_report::reportgen", "iroh debug should stay quiet");
        tracing::debug!(target: "hickory_resolver::name_server_pool", "dns debug should stay quiet");
        tracing::debug!(target: "rustls::client::hs", "rustls debug should stay quiet");
        tracing::debug!(target: "noq_proto::connection", "quic debug should stay quiet");
        tracing::debug!(target: "agent_client_protocol::jsonrpc", "acp wire debug should stay quiet");
        tracing::debug!(target: "swarm_discovery::receiver", "mdns debug should stay quiet");
        tracing::debug!(target: "iroh_mdns_address_lookup", "mdns address lookup debug should stay quiet");

        tracing::warn!(target: "swarm_discovery::receiver", "mdns warn should stay visible");
        tracing::warn!(target: "iroh::net_report::reportgen", "iroh warn should stay visible");
        tracing::info!(target: "agent_client_protocol::jsonrpc", "acp info should stay visible");
        tracing::trace!(target: "rua_agent", "rua trace should stay quiet");
    });

    assert!(output.contains("rua debug should be visible"));
    assert!(!output.contains("hyper debug should stay quiet"));
    assert!(!output.contains("iroh debug should stay quiet"));
    assert!(!output.contains("dns debug should stay quiet"));
    assert!(!output.contains("rustls debug should stay quiet"));
    assert!(!output.contains("quic debug should stay quiet"));
    assert!(!output.contains("acp wire debug should stay quiet"));
    assert!(!output.contains("mdns debug should stay quiet"));
    assert!(!output.contains("mdns address lookup debug should stay quiet"));
    assert!(output.contains("mdns warn should stay visible"));
    assert!(output.contains("iroh warn should stay visible"));
    assert!(output.contains("acp info should stay visible"));
    assert!(!output.contains("rua trace should stay quiet"));
}

#[test]
fn invalid_configured_log_level_falls_back_to_default_filter() {
    let config = LogConfig {
        log_level: Some("verbose-but-invalid".into()),
        ..Default::default()
    };

    let output = capture_with_filter(&config, || {
        tracing::info!(target: "hyper", "hyper info should stay quiet");
        tracing::warn!(target: "hyper", "hyper warn should be visible");
        tracing::info!(target: "rua_server", "rua info should be visible");
    });

    assert!(!output.contains("hyper info should stay quiet"));
    assert!(output.contains("hyper warn should be visible"));
    assert!(output.contains("rua info should be visible"));
}

#[test]
fn rust_log_env_filter_is_used_when_config_level_is_absent() {
    let filter = rua_observability::build_env_filter_with_rust_log(
        &LogConfig::default(),
        Some("rua_agent=trace,hyper=error".into()),
    );

    let output = capture_with_env_filter(filter, || {
        tracing::trace!(target: "rua_agent", "rua trace from rust_log");
        tracing::warn!(target: "hyper", "hyper warn should stay quiet");
        tracing::error!(target: "hyper", "hyper error from rust_log");
    });

    assert!(output.contains("rua trace from rust_log"));
    assert!(!output.contains("hyper warn should stay quiet"));
    assert!(output.contains("hyper error from rust_log"));
}

#[test]
fn configured_log_level_takes_priority_over_rust_log_env() {
    let config = LogConfig {
        log_level: Some("info".into()),
        ..Default::default()
    };
    let filter = rua_observability::build_env_filter_with_rust_log(
        &config,
        Some("rua_agent=trace,hyper=error".into()),
    );

    let output = capture_with_env_filter(filter, || {
        tracing::trace!(target: "rua_agent", "rua trace should be overridden");
        tracing::info!(target: "rua_agent", "rua info from config");
        tracing::warn!(target: "hyper", "hyper warn from config");
    });

    assert!(!output.contains("rua trace should be overridden"));
    assert!(output.contains("rua info from config"));
    assert!(output.contains("hyper warn from config"));
}

#[test]
fn size_rotating_appender_uses_existing_active_file_size_for_rotation() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("rua.log"), "already-full").unwrap();

    let mut appender = SizeRotatingAppender::new(dir.path(), "rua.log", 12, 2);
    write!(appender, "next").unwrap();
    appender.flush().unwrap();

    assert_eq!(
        std::fs::read_to_string(dir.path().join("rua.log.1")).unwrap(),
        "already-full"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("rua.log")).unwrap(),
        "next"
    );
}

#[test]
fn size_rotating_appender_creates_active_log_file() {
    let dir = tempfile::tempdir().unwrap();
    let mut appender = SizeRotatingAppender::new(dir.path(), "rua.log", 1024, 3);

    write!(appender, "hello").unwrap();
    appender.flush().unwrap();

    assert_eq!(
        std::fs::read_to_string(dir.path().join("rua.log")).unwrap(),
        "hello"
    );
}

#[test]
fn size_rotating_appender_rotates_when_next_write_exceeds_threshold() {
    let dir = tempfile::tempdir().unwrap();
    let mut appender = SizeRotatingAppender::new(dir.path(), "rua.log", 100, 3);

    write!(appender, "{}", "x".repeat(60)).unwrap();
    write!(appender, "{}", "y".repeat(60)).unwrap();
    appender.flush().unwrap();

    assert_eq!(
        std::fs::read_to_string(dir.path().join("rua.log.1")).unwrap(),
        "x".repeat(60)
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("rua.log")).unwrap(),
        "y".repeat(60)
    );
}

#[test]
fn size_rotating_appender_keeps_only_configured_backup_count() {
    let dir = tempfile::tempdir().unwrap();
    let mut appender = SizeRotatingAppender::new(dir.path(), "rua.log", 5, 2);

    for chunk in ["11111", "22222", "33333", "44444"] {
        write!(appender, "{chunk}").unwrap();
    }
    appender.flush().unwrap();

    assert!(dir.path().join("rua.log").exists());
    assert!(dir.path().join("rua.log.1").exists());
    assert!(dir.path().join("rua.log.2").exists());
    assert!(!dir.path().join("rua.log.3").exists());
}

#[test]
fn cleanup_legacy_logs_removes_old_daily_files_only() {
    let dir = tempfile::tempdir().unwrap();
    let old_date = (chrono::Local::now().date_naive() - chrono::Duration::days(10))
        .format("%Y-%m-%d")
        .to_string();
    let recent_date = (chrono::Local::now().date_naive() - chrono::Duration::days(3))
        .format("%Y-%m-%d")
        .to_string();

    let old_file = dir.path().join(format!("rua.log.{old_date}"));
    let recent_file = dir.path().join(format!("rua.log.{recent_date}"));
    let numbered_backup = dir.path().join("rua.log.1");
    let unrelated = dir.path().join("other.log.2000-01-01");
    std::fs::write(&old_file, "old").unwrap();
    std::fs::write(&recent_file, "recent").unwrap();
    std::fs::write(&numbered_backup, "backup").unwrap();
    std::fs::write(&unrelated, "other").unwrap();

    rua_observability::cleanup_legacy_logs(dir.path());

    assert!(!old_file.exists(), "old daily legacy log should be removed");
    assert!(
        recent_file.exists(),
        "recent daily legacy log should be kept"
    );
    assert!(
        numbered_backup.exists(),
        "size-rotation backups are not daily logs"
    );
    assert!(
        unrelated.exists(),
        "unrelated files are outside the cleanup scope"
    );
}
