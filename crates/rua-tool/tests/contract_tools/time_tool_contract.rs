use std::ffi::OsString;
use std::sync::OnceLock;

use rua_config::{save_config, RootConfig};
use rua_contract::Tool;
use rua_core::tool::ToolContext;
use rua_tool::extended::time::GetCurrentTimeTool;
use serde_json::json;
use tempfile::TempDir;
use tokio::sync::Mutex;

fn config_env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

struct ConfigFileEnvGuard {
    previous: Option<OsString>,
}

impl ConfigFileEnvGuard {
    fn set(path: &std::path::Path) -> Self {
        let previous = std::env::var_os("RUA_CONFIG_FILE");
        std::env::set_var("RUA_CONFIG_FILE", path);
        Self { previous }
    }
}

impl Drop for ConfigFileEnvGuard {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => std::env::set_var("RUA_CONFIG_FILE", value),
            None => std::env::remove_var("RUA_CONFIG_FILE"),
        }
    }
}

#[tokio::test]
async fn time_now_uses_timezone_from_root_config_when_argument_is_omitted() {
    let _lock = config_env_lock().lock().await;
    let root = TempDir::new().expect("tempdir");
    let config_path = root.path().join("config.json");
    let _guard = ConfigFileEnvGuard::set(&config_path);

    let config = RootConfig {
        user_timezone: Some("Asia/Shanghai".to_string()),
        ..Default::default()
    };
    save_config(&config, &config_path).expect("save root config");

    let result = GetCurrentTimeTool::new()
        .call(json!({}), ToolContext::default())
        .await
        .expect("time_now result");
    let text = result.text_content();

    assert!(
        text.contains(" Asia/Shanghai ("),
        "time_now should use config.json.user_timezone, got: {text}"
    );
}
