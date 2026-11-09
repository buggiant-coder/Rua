use std::collections::HashMap;
use tempfile::TempDir;

use rua_core::runtime_authority::{RuntimeAuthority, RuntimeAuthorityConfig};
use rua_core::runtime_layout::{HostProfile, LinuxCompatLayout, RuntimeKind, RuntimeLayout};
use rua_plugin::{
    plugin_pip_install_launch_plan, plugin_pip_install_launch_plan_with_authority, InstallRequest,
    PluginManager, PluginRecord,
};

#[cfg(unix)]
fn set_dir_mode(path: &std::path::Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path).unwrap().permissions();
    perms.set_mode(mode);
    std::fs::set_permissions(path, perms).unwrap();
}

#[tokio::test]
async fn source_url_persists_across_reload() {
    let tmp = TempDir::new().unwrap();
    let store_path = tmp.path().join("plugins.json");

    {
        let mgr = PluginManager::with_persistence(store_path.clone()).unwrap();
        mgr.install(InstallRequest {
            plugin_id: "url-plugin".into(),
            version: Some("1.0.0".into()),
            source_url: Some("https://example.com/plugin.wasm".into()),
            extra: Default::default(),
        })
        .await
        .unwrap();
    }

    {
        let mgr = PluginManager::with_persistence(store_path).unwrap();
        let list = mgr.list().await;
        assert_eq!(list.len(), 1);
        assert_eq!(
            list[0].source_url.as_deref(),
            Some("https://example.com/plugin.wasm")
        );
    }
}

#[tokio::test]
async fn old_manifest_without_source_url_loads() {
    let tmp = TempDir::new().unwrap();
    let store_path = tmp.path().join("plugins.json");

    let mut legacy: HashMap<String, serde_json::Value> = HashMap::new();
    legacy.insert(
        "old-plugin".into(),
        serde_json::json!({
            "id": "old-plugin",
            "name": "Old Plugin",
            "version": "0.1.0",
            "description": "",
            "enabled": true,
            "installed": true
        }),
    );
    std::fs::write(&store_path, serde_json::to_string(&legacy).unwrap()).unwrap();

    let mgr = PluginManager::with_persistence(store_path).unwrap();
    let list = mgr.list().await;
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, "old-plugin");
    assert!(list[0].source_url.is_none());
}

#[tokio::test]
async fn install_from_path_has_no_source_url() {
    let mgr = PluginManager::new();
    let record = mgr
        .install(InstallRequest {
            plugin_id: "local-plugin".into(),
            version: Some("0.1.0".into()),
            source_url: None,
            extra: Default::default(),
        })
        .await
        .unwrap();
    assert!(record.source_url.is_none());
}

#[cfg(unix)]
#[tokio::test]
async fn install_rolls_back_memory_when_persist_fails() {
    let tmp = TempDir::new().unwrap();
    let store_path = tmp.path().join("plugins.json");
    let mgr = PluginManager::with_persistence(store_path).unwrap();

    set_dir_mode(tmp.path(), 0o555);
    if std::fs::write(tmp.path().join("probe"), b"x").is_ok() {
        let _ = std::fs::remove_file(tmp.path().join("probe"));
        set_dir_mode(tmp.path(), 0o755);
        return;
    }

    let err = mgr
        .install(InstallRequest {
            plugin_id: "rollback-plugin".into(),
            version: Some("1.0.0".into()),
            source_url: None,
            extra: Default::default(),
        })
        .await
        .expect_err("persist must fail in a read-only directory");

    set_dir_mode(tmp.path(), 0o755);
    assert!(err.to_string().contains("failed to write plugin store"));
    assert!(
        mgr.list().await.is_empty(),
        "failed persist must not leave plugin in memory"
    );
}

#[test]
fn source_url_roundtrips_through_serde() {
    let record = PluginRecord {
        id: "test".into(),
        source_url: Some("https://example.com/p.wasm".into()),
        ..Default::default()
    };
    let json = serde_json::to_string(&record).unwrap();
    let deserialized: PluginRecord = serde_json::from_str(&json).unwrap();
    assert_eq!(
        deserialized.source_url.as_deref(),
        Some("https://example.com/p.wasm")
    );

    let no_url = PluginRecord {
        id: "test2".into(),
        source_url: None,
        ..Default::default()
    };
    let json2 = serde_json::to_string(&no_url).unwrap();
    let deserialized2: PluginRecord = serde_json::from_str(&json2).unwrap();
    assert!(deserialized2.source_url.is_none());
}

#[test]
fn plugin_pip_install_plan_uses_runtime_authority_python_package_install() {
    let tmp = TempDir::new().unwrap();
    let target = tmp.path().join("plugins/example");
    let data = tmp.path().join("data");

    let plan = plugin_pip_install_launch_plan("example-plugin==1.2.3", &target, &data);

    assert_eq!(
        plan.process_shape,
        rua_core::runtime_authority::ProcessShape::Install
    );
    assert_eq!(plan.runtime_kind, RuntimeKind::Native);
    assert!(
        plan.process()
            .expect("valid launch")
            .command
            .ends_with("python3")
            || plan
                .process()
                .expect("valid launch")
                .command
                .ends_with("python")
    );
    assert_eq!(plan.process().expect("valid launch").args[0], "-m");
    assert_eq!(plan.process().expect("valid launch").args[1], "pip");
    assert!(plan
        .process()
        .expect("valid launch")
        .args
        .iter()
        .any(|arg| arg == "--target"));
    assert!(plan
        .process()
        .expect("valid launch")
        .args
        .iter()
        .any(|arg| arg == "example-plugin==1.2.3"));
    assert_eq!(plan.process().expect("valid launch").cwd, Some(target));
}

#[test]
fn plugin_pip_install_android_plan_defaults_to_linux_compat_runtime() {
    let tmp = TempDir::new().unwrap();
    let native = tmp.path().join("native");
    let rootfs = native.parent().unwrap().join("files/runtime/debian");
    std::fs::create_dir_all(native.clone()).unwrap();
    std::fs::create_dir_all(rootfs.join("usr/bin")).unwrap();
    std::fs::write(native.join("libproot.so"), "").unwrap();
    std::fs::write(rootfs.join("usr/bin/env"), "").unwrap();

    std::fs::write(rootfs.join("usr/bin/python3"), "").unwrap();
    let target = tmp.path().join("files/home/plugins/example");
    std::fs::create_dir_all(&target).unwrap();
    let layout = RuntimeLayout {
        host_profile: HostProfile::MobileApk,
        os_family: "android".into(),
        linux_compat: Some(LinuxCompatLayout::android(
            &native,
            &native.parent().unwrap().join("files"),
            &native.parent().unwrap().join("files/home"),
        )),
    };
    let authority = RuntimeAuthority::from_layout_with_probe(
        layout,
        RuntimeAuthorityConfig {
            workspace_dir: target.clone(),
            cache_dir: tmp.path().join("files/home/.rua.cache"),
            path_entries: Vec::new(),
            pathext: Vec::new(),
            windows_shell_preference: None,
        },
        |path| path.is_file(),
    );

    let plan =
        plugin_pip_install_launch_plan_with_authority("example-plugin==1.2.3", &target, &authority);

    assert_eq!(plan.runtime_kind, RuntimeKind::LinuxCompat);
    assert!(plan
        .process()
        .expect("valid launch")
        .command
        .to_string_lossy()
        .ends_with("/librua-runtime-gate.so"));
    assert!(plan
        .process()
        .expect("valid launch")
        .args
        .iter()
        .any(|arg| arg == "/usr/bin/python3"));
    assert!(plan
        .process()
        .expect("valid launch")
        .args
        .iter()
        .any(|arg| arg == "-m"));
    assert!(plan
        .process()
        .expect("valid launch")
        .args
        .iter()
        .any(|arg| arg == "pip"));
    assert!(plan
        .process()
        .expect("valid launch")
        .args
        .iter()
        .any(|arg| arg == "example-plugin==1.2.3"));
}
