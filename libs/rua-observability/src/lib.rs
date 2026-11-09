use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use rua_core::runtime_layout::RuntimeLayout;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer, Registry};

const LOG_FILE_NAME: &str = "rua.log";
const LEGACY_LOG_PREFIX: &str = "rua.log";
const LEGACY_RETENTION_DAYS: i64 = 7;

const QUIET_NOISE_TARGETS: &str = "swarm_discovery=warn,\
iroh_mdns_address_lookup=warn,\
iroh=warn,\
iroh_relay=warn,\
iroh_gossip=warn,\
noq_proto=warn,\
hickory_net=warn,\
hickory_resolver=warn,\
net_report=warn,\
rustls=warn,\
portmapper=warn,\
igd_next=warn,\
reqwest=warn,\
hyper=warn,\
hyper_util=warn,\
agent_client_protocol=info";

static TRACING_INIT: OnceLock<()> = OnceLock::new();

#[derive(Debug, Clone)]
pub struct LogConfig {
    pub log_level: Option<String>,

    pub log_format: Option<String>,

    pub rotate_size_mb: u64,

    pub rotate_keep: u32,

    pub log_file: Option<PathBuf>,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            log_level: None,
            log_format: None,
            rotate_size_mb: 5,
            rotate_keep: 3,
            log_file: None,
        }
    }
}

pub fn build_env_filter(config: &LogConfig) -> EnvFilter {
    build_env_filter_inner(config, std::env::var("RUST_LOG").ok())
}

#[doc(hidden)]
pub fn build_env_filter_with_rust_log(config: &LogConfig, rust_log: Option<String>) -> EnvFilter {
    build_env_filter_inner(config, rust_log)
}

fn build_env_filter_inner(config: &LogConfig, rust_log: Option<String>) -> EnvFilter {
    if let Some(ref level) = config.log_level {
        let _ = &rust_log;
        let mut directives = format!("rua={level},{level},{QUIET_NOISE_TARGETS}");
        if let Ok(extra) = std::env::var("RUA_DEBUG_UNQUIET") {
            let extra = extra.trim();
            if !extra.is_empty() {
                directives.push(',');
                directives.push_str(extra);
            }
        }
        EnvFilter::try_new(&directives).unwrap_or_else(|_| EnvFilter::new("rua=info,warn"))
    } else if let Some(val) = rust_log {
        EnvFilter::try_new(&val).unwrap_or_else(|_| EnvFilter::new("rua=info,warn"))
    } else {
        EnvFilter::new("rua=info,warn")
    }
}

fn resolve_format(config: &LogConfig) -> &str {
    if let Some(ref fmt) = config.log_format {
        return fmt.as_str();
    }
    if let Ok(val) = std::env::var("RUA_LOG_FORMAT") {
        if val == "json" {
            return "json";
        }
    }
    "compact"
}

pub fn init_tracing(
    working_dir: &Path,
    config: &LogConfig,
    extra_layers: Vec<Box<dyn Layer<Registry> + Send + Sync>>,
) {
    TRACING_INIT.get_or_init(|| {
        let log_dir = working_dir.join("logs");
        let _ = std::fs::create_dir_all(&log_dir);

        let filter = build_env_filter(config);

        let appender = SizeRotatingAppender::new(
            &log_dir,
            LOG_FILE_NAME,
            config.rotate_size_mb * 1024 * 1024,
            config.rotate_keep,
        );
        let (file_writer, guard) = tracing_appender::non_blocking(appender);
        std::mem::forget(guard);

        let mut extra_layers = extra_layers;
        if let Some(ref log_file) = config.log_file {
            if let Some(parent) = log_file.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(log_file);
            match file {
                Ok(f) => {
                    let (writer, json_guard) = tracing_appender::non_blocking(f);
                    std::mem::forget(json_guard);
                    let json_layer = tracing_subscriber::fmt::layer()
                        .json()
                        .with_writer(writer)
                        .with_ansi(false);
                    extra_layers.push(Box::new(json_layer));
                }
                Err(e) => {
                    eprintln!("failed to open log_file {:?}: {e}", log_file);
                }
            }
        }

        cleanup_legacy_logs(&log_dir);

        if RuntimeLayout::from_current_process(None)
            .host_profile
            .is_mobile()
        {
            let _ = extra_layers;
            tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_writer(file_writer)
                .with_ansi(false)
                .init();
            return;
        }

        let format = resolve_format(config);
        match format {
            "json" => {
                tracing_subscriber::registry()
                    .with(extra_layers)
                    .with(filter)
                    .with(tracing_subscriber::fmt::layer().json())
                    .with(
                        tracing_subscriber::fmt::layer()
                            .json()
                            .with_writer(file_writer)
                            .with_ansi(false),
                    )
                    .init();
            }
            "pretty" => {
                tracing_subscriber::registry()
                    .with(extra_layers)
                    .with(filter)
                    .with(tracing_subscriber::fmt::layer().pretty())
                    .with(
                        tracing_subscriber::fmt::layer()
                            .with_writer(file_writer)
                            .with_ansi(false),
                    )
                    .init();
            }
            _ => {
                tracing_subscriber::registry()
                    .with(extra_layers)
                    .with(filter)
                    .with(tracing_subscriber::fmt::layer().compact())
                    .with(
                        tracing_subscriber::fmt::layer()
                            .with_writer(file_writer)
                            .with_ansi(false),
                    )
                    .init();
            }
        }
    });
}

pub struct SizeRotatingAppender {
    log_dir: PathBuf,
    base_name: String,
    max_size: u64,
    max_files: u32,
    file: std::fs::File,
    written: u64,
}

impl SizeRotatingAppender {
    pub fn new(log_dir: &Path, base_name: &str, max_size: u64, max_files: u32) -> Self {
        let _ = std::fs::create_dir_all(log_dir);
        let path = log_dir.join(base_name);
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .expect("failed to open log file");
        let written = file.metadata().map(|m| m.len()).unwrap_or(0);
        Self {
            log_dir: log_dir.to_path_buf(),
            base_name: base_name.to_string(),
            max_size,
            max_files,
            file,
            written,
        }
    }

    fn rotate(&mut self) {
        let oldest = self
            .log_dir
            .join(format!("{}.{}", self.base_name, self.max_files));
        let _ = std::fs::remove_file(oldest);

        for i in (1..self.max_files).rev() {
            let from = self.log_dir.join(format!("{}.{}", self.base_name, i));
            let to = self.log_dir.join(format!("{}.{}", self.base_name, i + 1));
            if from.exists() {
                let _ = std::fs::rename(&from, &to);
            }
        }

        let current = self.log_dir.join(&self.base_name);
        let first_backup = self.log_dir.join(format!("{}.1", self.base_name));
        let _ = std::fs::rename(&current, &first_backup);

        if let Ok(f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&current)
        {
            self.file = f;
        }
        self.written = 0;
    }
}

impl Write for SizeRotatingAppender {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.written + buf.len() as u64 > self.max_size {
            self.rotate();
        }
        let n = self.file.write(buf)?;
        self.written += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

#[doc(hidden)]
pub fn cleanup_legacy_logs(log_dir: &Path) {
    let cutoff = chrono::Local::now().date_naive() - chrono::Duration::days(LEGACY_RETENTION_DAYS);
    let entries = match std::fs::read_dir(log_dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if let Some(date_str) = name_str.strip_prefix(&format!("{LEGACY_LOG_PREFIX}.")) {
            if let Ok(file_date) = chrono::NaiveDate::parse_from_str(date_str, "%Y-%m-%d") {
                if file_date < cutoff {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
    }
}

#[cfg(feature = "metrics")]
mod metrics_init {
    use std::net::SocketAddr;
    use std::sync::OnceLock;

    static METRICS_INIT: OnceLock<()> = OnceLock::new();

    pub fn init_metrics(addr: SocketAddr) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut result: Result<(), Box<dyn std::error::Error + Send + Sync>> = Ok(());
        METRICS_INIT.get_or_init(|| {
            match metrics_exporter_prometheus::PrometheusBuilder::new()
                .with_http_listener(addr)
                .install()
            {
                Ok(()) => {
                    tracing::info!(%addr, "prometheus metrics exporter started");
                }
                Err(e) => {
                    result = Err(e.into());
                }
            }
        });
        result
    }
}

#[cfg(feature = "metrics")]
pub use metrics_init::init_metrics;

pub fn init_test_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("rua_server=debug,warn")
        .with_test_writer()
        .try_init();
}
