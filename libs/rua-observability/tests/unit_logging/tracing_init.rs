use std::sync::{Arc, Mutex};

use rua_observability::LogConfig;
use tracing_subscriber::{Layer, Registry};

#[derive(Clone)]
struct CapturingLayer {
    events: Arc<Mutex<Vec<String>>>,
}

impl<S> Layer<S> for CapturingLayer
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let mut visitor = MessageVisitor(String::new());
        event.record(&mut visitor);
        self.events.lock().unwrap().push(visitor.0);
    }
}

struct MessageVisitor(String);

impl tracing::field::Visit for MessageVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.0 = format!("{value:?}");
        }
    }
}

#[test]
fn init_tracing_wires_default_log_file_json_log_file_and_extra_layers() {
    let tmp = tempfile::tempdir().unwrap();
    let json_log = tmp.path().join("kernel.log");
    let captured = Arc::new(Mutex::new(Vec::<String>::new()));

    let config = LogConfig {
        log_file: Some(json_log.clone()),
        ..Default::default()
    };
    let extra: Vec<Box<dyn Layer<Registry> + Send + Sync>> = vec![Box::new(CapturingLayer {
        events: captured.clone(),
    })];

    rua_observability::init_tracing(tmp.path(), &config, extra);

    tracing::info!(target: "rua::lifecycle", "engine_started");

    std::thread::sleep(std::time::Duration::from_millis(200));

    let events = captured.lock().unwrap();
    assert!(
        events.iter().any(|e| e.contains("engine_started")),
        "custom layer should have captured the event, got: {events:?}"
    );

    let default_log = tmp.path().join("logs/rua.log");
    assert!(default_log.exists(), "default log file should be created");

    assert!(json_log.exists(), "JSON log file should be created");
    let content = std::fs::read_to_string(&json_log).unwrap();
    assert!(
        content.contains("engine_started"),
        "JSON log should contain the event, got: {content}"
    );

    for line in content.lines().filter(|line| !line.is_empty()) {
        let parsed: serde_json::Value = serde_json::from_str(line)
            .unwrap_or_else(|err| panic!("line should be valid JSON: {err}\nline: {line}"));
        assert!(parsed.is_object(), "each line should be a JSON object");
    }
}
