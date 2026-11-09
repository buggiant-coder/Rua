use std::path::Path;

use rua_observability::LogConfig;
use tracing_subscriber::{Layer, Registry};

pub use rua_observability::init_test_tracing;

#[cfg(feature = "metrics")]
pub use rua_observability::init_metrics;

pub fn init_tracing(working_dir: &Path, config: &LogConfig) {
    let audit_dir = working_dir.join("audit");
    let audit_sink = rua_security::FileAuditSink::new(audit_dir);

    let extra: Vec<Box<dyn Layer<Registry> + Send + Sync>> = vec![Box::new(audit_sink)];
    rua_observability::init_tracing(working_dir, config, extra);
}
