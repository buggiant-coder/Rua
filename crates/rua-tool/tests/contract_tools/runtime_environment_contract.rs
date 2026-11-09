use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::sync::{Arc, OnceLock};

use rua_core::tool::ToolContext;
use rua_tool::{register_all_builtin_tools, CommandExecutor, MockCommandExecutor, ToolRegistry};
use serde_json::json;
use tempfile::TempDir;
use tokio::sync::Mutex;

fn ctx() -> ToolContext {
    ToolContext {
        agent_id: "agent".into(),
        session_id: "chat".into(),
        working_dir: std::env::current_dir().expect("cwd"),
        metadata: HashMap::new(),
        cancel: None,
        task_runtime: None,
        ..Default::default()
    }
}

fn ctx_with_metadata(metadata: HashMap<String, serde_json::Value>) -> ToolContext {
    ToolContext { metadata, ..ctx() }
}

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

struct EnvRestore {
    path: Option<OsString>,
    cache_ttl: Option<OsString>,
}

impl EnvRestore {
    fn capture() -> Self {
        Self {
            path: std::env::var_os("PATH"),
            cache_ttl: std::env::var_os("RUA_RUNTIME_ENV_CAPABILITY_CACHE_TTL_MS"),
        }
    }
}

impl Drop for EnvRestore {
    fn drop(&mut self) {
        match &self.path {
            Some(value) => std::env::set_var("PATH", value),
            None => std::env::remove_var("PATH"),
        }
        match &self.cache_ttl {
            Some(value) => {
                std::env::set_var("RUA_RUNTIME_ENV_CAPABILITY_CACHE_TTL_MS", value);
            }
            None => std::env::remove_var("RUA_RUNTIME_ENV_CAPABILITY_CACHE_TTL_MS"),
        }
    }
}

fn runtime_tool() -> Arc<dyn rua_contract::Tool> {
    let mut registry = ToolRegistry::new();
    let executor: Arc<dyn CommandExecutor> = Arc::new(MockCommandExecutor::new());
    register_all_builtin_tools(&mut registry, executor, None).expect("builtin tools register");
    registry
        .get("runtime_info")
        .expect("runtime environment tool")
        .clone()
}

async fn runtime_environment_json() -> serde_json::Value {
    let result = runtime_tool()
        .call(json!({}), ctx())
        .await
        .expect("tool call");
    assert!(!result.is_error);
    serde_json::from_str(&result.text_content()).expect("runtime environment json")
}

fn write_executable(dir: &TempDir, name: &str) -> std::path::PathBuf {
    let path = dir.path().join(name);
    fs::write(&path, "#!/bin/sh\nexit 0\n").expect("write fake command");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&path)
            .expect("fake command metadata")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).expect("mark fake command executable");
    }
    path
}

#[tokio::test]
async fn runtime_get_runtime_info_returns_stable_read_only_schema() {
    let body = runtime_environment_json().await;
    for key in [
        "device",
        "known_devices",
        "chat_execution",
        "os",
        "runtime",
        "paths",
        "agent_path_contract",
        "runtime_authority",
        "capabilities",
        "android",
        "sync",
        "warnings",
    ] {
        assert!(body.get(key).is_some(), "missing {key}: {body}");
    }
    assert_eq!(
        body["paths"]["workspace_dir"].as_str(),
        Some(std::env::current_dir().unwrap().to_string_lossy().as_ref())
    );
    assert!(body["capabilities"]["python"].get("available").is_some());
    assert_eq!(
        body["capabilities"]["node"]["path"],
        body["runtime_authority"]["node"]["path"]
    );
    assert_eq!(
        body["capabilities"]["npm"]["path"],
        body["runtime_authority"]["npm"]["path"]
    );
    assert_eq!(
        body["capabilities"]["npx"]["path"],
        body["runtime_authority"]["npx"]["path"]
    );
    assert!(body["capabilities"]["uv"].get("available").is_some());
    assert!(body["capabilities"]["node"].get("available").is_some());
    assert!(body["sync"].is_object());
    assert_eq!(body["chat_execution"]["mode"], "local_per_turn");
    assert!(body["known_devices"].is_array());
    assert_eq!(
        body["agent_path_contract"]["workspace_dir"].as_str(),
        Some(std::env::current_dir().unwrap().to_string_lossy().as_ref())
    );
}

#[tokio::test]
async fn runtime_get_runtime_info_reports_mobile_apk_resident_and_agent_runtimes() {
    let tool = runtime_tool();
    let metadata = HashMap::from([
        ("host_profile".to_string(), json!("mobile-apk")),
        ("device_id".to_string(), json!("0102030405060708")),
        ("device_friendly_name".to_string(), json!("Pixel Tablet")),
        (
            "known_devices".to_string(),
            json!([{"device_id": "0102030405060708", "friendly_name": "Pixel Tablet", "is_self": true}]),
        ),
        (
            "chat_execution".to_string(),
            json!({"mode": "fixed_target", "target_device_id": "0102030405060708"}),
        ),
    ]);
    let result = tool
        .call(json!({}), ctx_with_metadata(metadata))
        .await
        .expect("tool call");

    let body: serde_json::Value =
        serde_json::from_str(&result.text_content()).expect("runtime environment json");
    assert_eq!(body["device"]["host_profile"], "mobile-apk");
    assert_eq!(body["device"]["device_id"], "0102030405060708");
    assert_eq!(body["device"]["friendly_name"], "Pixel Tablet");
    assert_eq!(body["known_devices"][0]["friendly_name"], "Pixel Tablet");
    assert_eq!(body["chat_execution"]["mode"], "fixed_target");
    assert_eq!(
        body["chat_execution"]["target_device_id"],
        "0102030405060708"
    );
    assert_eq!(body["runtime"]["resident_runtime"], "native-android");
    assert_eq!(body["runtime"]["shell_runtime"], "linux-compat");
    assert_eq!(
        body["runtime"]["linux_compat_runtime"]["kind"],
        "proot-debian"
    );
    assert_eq!(body["android"]["is_android"], true);
    assert_eq!(body["android"]["shared_storage"]["permission"], "unknown");
    assert_eq!(
        body["agent_path_contract"]["aliases"]["mobile_shared_storage"],
        "/sdcard"
    );
    // Host profile metadata alone must not invent APK paths or a configured guest.
    assert!(body["agent_path_contract"]["aliases"]["linux_guest_home"].is_null());
    assert!(body["agent_path_contract"]["rules"]
        .as_array()
        .is_some_and(|rules| rules.iter().any(|rule| rule
            .as_str()
            .is_some_and(|text| text.contains("Use /sdcard")))));
    assert_eq!(
        body["runtime"]["linux_compat_runtime"]["default"],
        serde_json::Value::Bool(true)
    );
    assert_eq!(
        body["runtime"]["default_external_runtime"],
        serde_json::Value::String("linux_compat".to_string())
    );
    assert!(body["warnings"]
        .as_array()
        .is_some_and(|warnings| warnings.iter().any(|warning| warning
            .as_str()
            .is_some_and(|text| text.contains("PRoot Debian")))));
}

#[tokio::test]
async fn runtime_get_runtime_info_refreshes_runtime_authority_capabilities() {
    let _guard = env_lock().lock().await;
    let _restore = EnvRestore::capture();
    let bin_dir = tempfile::tempdir().expect("tempdir");
    let node_path = write_executable(&bin_dir, "node");
    std::env::set_var("PATH", bin_dir.path());
    std::env::set_var("RUA_RUNTIME_ENV_CAPABILITY_CACHE_TTL_MS", "50");

    let first = runtime_environment_json().await;
    assert_eq!(first["capabilities"]["node"]["available"], true);
    assert_eq!(
        first["capabilities"]["node"]["path"].as_str(),
        Some(node_path.to_string_lossy().as_ref())
    );

    fs::remove_file(&node_path).expect("remove fake command");
    let refreshed = runtime_environment_json().await;
    assert_eq!(refreshed["capabilities"]["node"]["available"], false);
    assert_eq!(
        refreshed["capabilities"]["node"]["reason"].as_str(),
        Some("node not found on PATH")
    );
    assert_eq!(
        refreshed["capabilities"]["node"]["available"],
        refreshed["runtime_authority"]["node"]["available"]
    );
}
