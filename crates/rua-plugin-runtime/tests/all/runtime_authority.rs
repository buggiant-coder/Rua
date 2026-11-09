use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use rua_core::runtime_authority::{RuntimeAuthority, RuntimeAuthorityConfig};
use rua_core::runtime_layout::{HostProfile, LinuxCompatLayout, RuntimeLayout};
use rua_mcp::transports::stdio::StdioConfig;
use rua_plugin_runtime::client::{plugin_stdio_config_with_authority, PluginClientConfig};

fn stdio_config(command: &str, cwd: PathBuf) -> StdioConfig {
    StdioConfig {
        server_key: "plugin:test".into(),
        server_name: "test".into(),
        command: command.into(),
        args: vec!["-m".into(), "test_plugin".into()],
        env: BTreeMap::from([("PLUGIN_SECRET".into(), "secret".into())]),
        cwd: Some(cwd),
        startup_timeout: Duration::from_secs(5),
        call_timeout: Duration::from_secs(5),
        stderr_log_prefix: Some("plugin:test".into()),
    }
}

#[test]
fn plugin_stdio_native_plan_uses_runtime_authority_python_and_preserves_env() {
    let temp = tempfile::tempdir().unwrap();
    let bin = temp.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let python = bin.join("python3");
    std::fs::write(&python, "").unwrap();
    let workspace = temp.path().join("workspace");
    let cache = temp.path().join("cache");

    let authority = RuntimeAuthority::from_layout_with_probe(
        RuntimeLayout::from_current_process(Some("desktop")),
        RuntimeAuthorityConfig {
            workspace_dir: workspace.clone(),
            cache_dir: cache,
            path_entries: vec![bin],
            pathext: Vec::new(),
            windows_shell_preference: None,
        },
        |path| path.is_file(),
    );

    let config = plugin_stdio_config_with_authority(
        stdio_config("python", workspace.clone()),
        &authority,
        Default::default(),
    )
    .expect("ready runtime");

    assert_eq!(config.command, python.to_string_lossy());
    assert_eq!(config.args, vec!["-m", "test_plugin"]);
    assert_eq!(config.env["PLUGIN_SECRET"], "secret");
    assert_eq!(config.cwd, Some(workspace));
}

#[test]
fn plugin_stdio_android_plan_defaults_to_linux_compat_runtime() {
    let temp = tempfile::tempdir().unwrap();
    let native = temp.path().join("native");
    let data = temp.path().join("files");
    let home = data.join("home");
    let rootfs = data.join("runtime/debian");
    std::fs::create_dir_all(&native).unwrap();
    std::fs::create_dir_all(rootfs.join("usr/bin")).unwrap();
    std::fs::write(rootfs.join("usr/bin/env"), "").unwrap();
    std::fs::write(rootfs.join("usr/bin/python3"), "").unwrap();
    std::fs::write(native.join("libproot.so"), "").unwrap();

    let workspace = temp.path().join("files/home/workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let layout = RuntimeLayout {
        host_profile: HostProfile::MobileApk,
        os_family: "android".into(),
        linux_compat: Some(LinuxCompatLayout::android(&native, &data, &home)),
    };
    let authority = RuntimeAuthority::from_layout_with_probe(
        layout,
        RuntimeAuthorityConfig {
            workspace_dir: workspace.clone(),
            cache_dir: temp.path().join("files/home/.rua.cache"),
            path_entries: Vec::new(),
            pathext: Vec::new(),
            windows_shell_preference: None,
        },
        |path| path.is_file(),
    );

    let config = plugin_stdio_config_with_authority(
        stdio_config("python", workspace),
        &authority,
        Default::default(),
    )
    .expect("ready runtime");

    assert!(config.command.ends_with("/native/librua-runtime-gate.so"));
    assert!(config
        .args
        .iter()
        .any(|arg| arg.ends_with("/native/libproot.so")));
    assert!(config.args.iter().any(|arg| arg == "/usr/bin/python3"));
    assert!(config.args.iter().any(|arg| arg == "test_plugin"));
    assert_eq!(config.env["PLUGIN_SECRET"], "secret");
    assert!(!config.args.iter().any(|arg| arg.contains("secret")));
}

#[test]
fn plugin_client_config_keeps_runtime_cache_dir_device_local() {
    let mut config = PluginClientConfig::new("plugin-a", "entry");
    config.runtime_cache_dir = PathBuf::from("/device/local/cache");

    assert_eq!(
        config.runtime_cache_dir,
        PathBuf::from("/device/local/cache")
    );
}
