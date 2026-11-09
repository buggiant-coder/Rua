use std::path::{Path, PathBuf};
use std::sync::{Mutex, Once, OnceLock};

static PANIC_DIR: OnceLock<Mutex<PathBuf>> = OnceLock::new();
static INSTALL_HOOK: Once = Once::new();

pub fn install_panic_hook(data_dir: &Path) {
    let slot = PANIC_DIR.get_or_init(|| Mutex::new(data_dir.to_path_buf()));
    if let Ok(mut dir) = slot.lock() {
        *dir = data_dir.to_path_buf();
    }

    INSTALL_HOOK.call_once(|| {
        let previous_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let message = panic_message(info);
            let location = info
                .location()
                .map(|loc| format!("{}:{}:{}", loc.file(), loc.line(), loc.column()))
                .unwrap_or_else(|| "unknown".to_string());
            let thread = std::thread::current()
                .name()
                .unwrap_or("unnamed")
                .to_string();
            let backtrace = std::backtrace::Backtrace::force_capture();

            tracing::error!(
                panic.message = %message,
                panic.location = %location,
                panic.thread = %thread,
                "rua-server panic"
            );

            write_panic_log(&thread, &location, &message, &backtrace);
            previous_hook(info);
        }));
    });
}

fn panic_message(info: &std::panic::PanicHookInfo<'_>) -> String {
    if let Some(message) = info.payload().downcast_ref::<&'static str>() {
        (*message).to_string()
    } else if let Some(message) = info.payload().downcast_ref::<String>() {
        message.clone()
    } else {
        "non-string panic payload".to_string()
    }
}

fn write_panic_log(
    thread: &str,
    location: &str,
    message: &str,
    backtrace: &std::backtrace::Backtrace,
) {
    let Some(slot) = PANIC_DIR.get() else {
        return;
    };
    let Ok(data_dir) = slot.lock() else {
        return;
    };
    let log_dir = data_dir.join("logs");
    if std::fs::create_dir_all(&log_dir).is_err() {
        return;
    }
    let path = log_dir.join("panic.log");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    let entry = format!(
        "\n=== panic unix_ms={now} thread={thread} location={location} ===\n{message}\n{backtrace}\n"
    );
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut file| std::io::Write::write_all(&mut file, entry.as_bytes()));
}
