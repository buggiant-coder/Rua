use std::collections::HashMap;
use std::env;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use rua_contract::Tool;
use rua_core::runtime_authority::{RuntimeAuthority, RuntimeCapability};
use rua_core::runtime_layout::RuntimeLayout;
use rua_core::tool::{ToolContext, ToolResult, ToolSchema};
use rua_core::Result;

use crate::annotations::ExtendedToolAnnotations;
use crate::schema::ToolSpec;

const DEFAULT_CAPABILITY_CACHE_TTL: Duration = Duration::from_secs(300);
const CAPABILITY_CACHE_TTL_ENV: &str = "RUA_RUNTIME_ENV_CAPABILITY_CACHE_TTL_MS";

pub struct RuntimeGetRuntimeInfoTool;

impl RuntimeGetRuntimeInfoTool {
    pub fn new() -> Self {
        Self
    }

    pub fn spec() -> ToolSpec {
        ToolSpec {
            name: "runtime_info".to_string(),
            description:
                "Inspect the current device, OS, runtime, paths, and tool capabilities. Read-only."
                    .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {},
                "required": [],
                "additionalProperties": false
            }),
            annotations: ExtendedToolAnnotations::read_only(),
            namespace: "builtin".into(),
        }
    }
}

impl Default for RuntimeGetRuntimeInfoTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for RuntimeGetRuntimeInfoTool {
    fn name(&self) -> &str {
        "runtime_info"
    }

    fn schema(&self) -> ToolSchema {
        let spec = Self::spec();
        ToolSchema {
            name: spec.name,
            description: spec.description,
            parameters: spec.parameters,
        }
    }

    async fn call(&self, _args: serde_json::Value, ctx: ToolContext) -> Result<ToolResult> {
        let env = build_environment_report(&ctx);
        let text = serde_json::to_string_pretty(&env).unwrap_or_else(|_| "{}".to_string());
        Ok(ToolResult::text("runtime_info", text))
    }
}

fn build_environment_report(ctx: &ToolContext) -> serde_json::Value {
    let host_profile = host_profile(ctx);
    let os_family = os_family();
    let is_android_runtime = os_family == "android" || host_profile == "mobile-apk";
    let cache_dir = rua_config::dirs::resolve_cache_dir(&ctx.working_dir);
    let runtime_authority =
        RuntimeAuthority::from_current_process(Some(&host_profile), &ctx.working_dir, cache_dir);
    let inventory = runtime_authority.inventory();
    let runtime_layout = RuntimeLayout::from_current_process(Some(&host_profile));
    let agent_path_contract = runtime_layout.agent_path_contract(&ctx.working_dir);
    let linux_compat_available = inventory.linux_compat_available;
    let path_entries = runtime_path_entries(is_android_runtime, &runtime_layout);
    let device_id = ctx
        .metadata
        .get("device_id")
        .and_then(|value| value.as_str())
        .map(str::to_string);
    let device_friendly_name = ctx
        .metadata
        .get("device_friendly_name")
        .and_then(|value| value.as_str())
        .map(str::to_string)
        .or_else(device_friendly_name);
    let known_devices = ctx
        .metadata
        .get("known_devices")
        .cloned()
        .unwrap_or_else(|| serde_json::Value::Array(Vec::new()));
    let chat_execution = ctx
        .metadata
        .get("chat_execution")
        .cloned()
        .unwrap_or_else(|| {
            serde_json::json!({
                "mode": "local_per_turn",
                "target_device_id": serde_json::Value::Null,
            })
        });

    serde_json::json!({
        "device": {
            "device_id": device_id,
            "friendly_name": device_friendly_name,
            "host_profile": host_profile,
        },
        "known_devices": known_devices,
        "chat_execution": chat_execution,
        "os": {
            "family": os_family,
            "arch": env::consts::ARCH,
            "kernel": serde_json::Value::Null,
            "version": serde_json::Value::Null,
        },
        "runtime": {
            "resident_runtime": resident_runtime(is_android_runtime, &host_profile),
            "shell_runtime": shell_runtime(is_android_runtime),
            "linux_compat_runtime": {
                "available": linux_compat_available,
                "kind": "proot-debian",
                "default": inventory.default_external_runtime == rua_core::runtime_layout::RuntimeKind::LinuxCompat,
            },
            "default_external_runtime": inventory.default_external_runtime,
        },
        "runtime_authority": inventory,
        "paths": {
            "workspace_dir": ctx.working_dir.to_string_lossy(),
            "data_dir": serde_json::Value::Null,
            "home": env::var("HOME").ok(),
            "tmp": env::var("TMPDIR").or_else(|_| env::var("TEMP")).ok(),
            "path": path_entries,
        },
        "agent_path_contract": agent_path_contract,
        "capabilities": {
            "shell": shell_capability(&inventory.shell),
            "python": capability_json(&inventory.python),
            "uv": capability_json(&inventory.uv),
            "node": capability_json(&inventory.node),
            "npm": capability_json(&inventory.npm),
            "npx": capability_json(&inventory.npx),
            "git": capability_json(&inventory.git),
            "rg": capability_json(&inventory.rg),
            "ffmpeg": capability_json(&inventory.ffmpeg),
            "browser_automation": {
                "available": false,
                "reason": if is_android_runtime { "not verified on mobile" } else { "not checked by this tool" },
            },
            "docker": {
                "available": command_on_path("docker").is_some() && !is_android_runtime,
                "reason": docker_reason(is_android_runtime),
            },
        },
        "android": android_report(is_android_runtime, &runtime_layout, &agent_path_contract),
        "sync": {
            "enabled": serde_json::Value::Null,
            "device_id": device_id,
            "network_id": serde_json::Value::Null,
            "power_policy": serde_json::Value::Null,
        },
        "warnings": warnings(is_android_runtime),
    })
}

fn capability_json(capability: &RuntimeCapability) -> serde_json::Value {
    serde_json::json!({
        "available": capability.available,
        "command": capability.name,
        "path": capability.path.as_ref().map(|path| path.to_string_lossy().to_string()),
        "version": capability.version,
        "reason": capability.diagnostics.first().cloned(),
        "source": capability.source,
    })
}

fn shell_capability(shell: &rua_core::runtime_layout::NativeShellSpec) -> serde_json::Value {
    serde_json::json!({
        "available": true,
        "command": shell.program,
        "path": shell.program,
        "version": serde_json::Value::Null,
        "kind": shell.contract_kind(),
    })
}

fn docker_reason(is_android: bool) -> serde_json::Value {
    if is_android {
        serde_json::json!("not supported on Android")
    } else {
        serde_json::Value::Null
    }
}

fn host_profile(ctx: &ToolContext) -> String {
    if let Some(value) = ctx
        .metadata
        .get("host_profile")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
    {
        return value.to_string();
    }
    env::var("RUA_HOST_PROFILE")
        .or_else(|_| env::var("RUA_SERVER_PROFILE"))
        .unwrap_or_else(|_| {
            if cfg!(target_os = "android") {
                "mobile-apk".to_string()
            } else {
                "desktop".to_string()
            }
        })
}

fn os_family() -> &'static str {
    if cfg!(target_os = "android") {
        "android"
    } else {
        env::consts::OS
    }
}

fn resident_runtime(is_android: bool, host_profile: &str) -> &'static str {
    if is_android || host_profile == "mobile-apk" {
        "native-android"
    } else if host_profile == "headless" {
        "headless-native"
    } else {
        "desktop-native"
    }
}

fn shell_runtime(is_android: bool) -> &'static str {
    if is_android {
        "linux-compat"
    } else {
        "desktop-shell"
    }
}

fn runtime_path_entries(is_android: bool, layout: &RuntimeLayout) -> Vec<String> {
    if is_android {
        return layout
            .linux_compat
            .as_ref()
            .and_then(|linux| linux.base_env(&[]).get("PATH").cloned())
            .map(|path| path.split(':').map(str::to_string).collect())
            .unwrap_or_default();
    }
    path_entries()
}

fn device_friendly_name() -> Option<String> {
    env::var("HOSTNAME")
        .or_else(|_| env::var("COMPUTERNAME"))
        .ok()
        .filter(|value| !value.trim().is_empty())
}

fn path_entries() -> Vec<String> {
    env::var_os("PATH")
        .map(|paths| {
            env::split_paths(&paths)
                .map(|p| p.to_string_lossy().to_string())
                .collect()
        })
        .unwrap_or_default()
}

fn command_on_path(command: &str) -> Option<PathBuf> {
    if command.contains('/') || command.contains('\\') {
        let path = PathBuf::from(command);
        return path.is_file().then_some(path);
    }
    let path_env = env::var_os("PATH")
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_default();
    let cache_key = format!("{command}\0{path_env}");
    if let Some(path) = cached_command_lookup(&cache_key) {
        return path;
    }
    let resolved = find_command_on_path(command);
    store_command_lookup(cache_key, resolved.clone());
    resolved
}

fn find_command_on_path(command: &str) -> Option<PathBuf> {
    let paths = env::var_os("PATH")?;
    for dir in env::split_paths(&paths) {
        for name in command_file_names(command) {
            let candidate = dir.join(name);
            if is_executable_file(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

fn command_file_names(command: &str) -> Vec<String> {
    #[cfg(windows)]
    {
        let mut names = vec![command.to_string()];
        if Path::new(command).extension().is_none() {
            let pathext = env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
            for ext in pathext.split(';').filter(|ext| !ext.trim().is_empty()) {
                names.push(format!("{command}{ext}"));
            }
        }
        names
    }
    #[cfg(not(windows))]
    {
        vec![command.to_string()]
    }
}

#[derive(Clone)]
struct CachedCommandLookup {
    checked_at: Instant,
    path: Option<PathBuf>,
}

fn command_cache() -> &'static Mutex<HashMap<String, CachedCommandLookup>> {
    static CACHE: OnceLock<Mutex<HashMap<String, CachedCommandLookup>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cached_command_lookup(cache_key: &str) -> Option<Option<PathBuf>> {
    let mut cache = command_cache().lock().ok()?;
    let entry = cache.get(cache_key)?;
    if entry.checked_at.elapsed() > capability_cache_ttl() {
        cache.remove(cache_key);
        return None;
    }
    Some(entry.path.clone())
}

fn capability_cache_ttl() -> Duration {
    env::var(CAPABILITY_CACHE_TTL_ENV)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_millis)
        .unwrap_or(DEFAULT_CAPABILITY_CACHE_TTL)
}

fn store_command_lookup(cache_key: String, path: Option<PathBuf>) {
    if let Ok(mut cache) = command_cache().lock() {
        cache.insert(
            cache_key,
            CachedCommandLookup {
                checked_at: Instant::now(),
                path,
            },
        );
    }
}

fn is_executable_file(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .map(|meta| meta.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn android_report(
    is_android: bool,
    runtime_layout: &RuntimeLayout,
    agent_path_contract: &rua_core::runtime_layout::AgentPathContract,
) -> serde_json::Value {
    if !is_android {
        return serde_json::Value::Null;
    }
    let native_dir = runtime_layout
        .linux_compat
        .as_ref()
        .and_then(|layout| layout.proot_path.parent())
        .map(|path| path.to_string_lossy().into_owned());
    let app_home = agent_path_contract.aliases.get("app_home").cloned();
    let shared_paths = agent_path_contract
        .user_file_roots
        .iter()
        .map(|path| serde_json::Value::String(path.clone()))
        .collect::<Vec<_>>();
    let linux_compat_binds = runtime_layout
        .linux_compat
        .as_ref()
        .map(|layout| {
            layout
                .binds
                .iter()
                .map(|bind| {
                    if bind.host == bind.guest {
                        bind.host.to_string_lossy().into_owned()
                    } else {
                        format!("{}:{}", bind.host.display(), bind.guest.display())
                    }
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    serde_json::json!({
        "is_android": true,
        "native_library_dir": native_dir,
        "app_home": app_home,
        "shared_storage": {
            "paths": shared_paths,
            "permission": "unknown",
        },
        "linux_compat_binds": linux_compat_binds,
        "notes": [
            "Resident server runs as a native Android process launched by the APK.",
            "Agent tools run inside Linux Compatibility Runtime.",
            "The user-facing terminal uses Linux Compatibility Runtime.",
            "Missing Debian tools are reported before execution; there is no host command fallback.",
            "Absolute desktop paths from other devices are not available here.",
            "/sdcard access depends on Android storage permission."
        ],
    })
}

fn warnings(is_android: bool) -> Vec<String> {
    if is_android {
        vec![
            "The resident server runs natively on Android; external commands run in PRoot Debian.".to_string(),
            "Use workspace_dir for Agent files and /sdcard for user-visible phone files; avoid ~ in tool arguments.".to_string(),
        ]
    } else {
        vec!["Use portable workspace paths where possible.".to_string()]
    }
}
