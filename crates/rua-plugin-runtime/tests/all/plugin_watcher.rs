use std::time::Duration;

use rua_plugin_runtime::{PluginEvent, PluginRegistry, PluginWatcher};

fn write_plugin_dir(plugins_dir: &std::path::Path, id: &str) {
    let dir = plugins_dir.join(id);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("plugin.json"),
        format!(r#"{{"id":"{id}","name":"{id}","version":"1.0.0","description":"test"}}"#,),
    )
    .unwrap();
}

#[tokio::test]
async fn watcher_detects_new_plugin() {
    let tmp = tempfile::tempdir().unwrap();
    let plugins_dir = tmp.path().join("plugins");
    std::fs::create_dir_all(&plugins_dir).unwrap();

    let registry = PluginRegistry::new();
    let mut rx = registry.subscribe();

    let _watcher = PluginWatcher::start_with_debounce(
        plugins_dir.clone(),
        registry.clone(),
        Duration::from_millis(200),
    )
    .unwrap();

    tokio::time::sleep(Duration::from_millis(100)).await;

    write_plugin_dir(&plugins_dir, "hot-plugin");

    let event = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("should receive event within 5s")
        .expect("channel not closed");

    assert!(matches!(event, PluginEvent::Installed { ref plugin_id } if plugin_id == "hot-plugin"));
    assert_eq!(registry.list().unwrap(), vec!["hot-plugin"]);
}

#[tokio::test]
async fn watcher_detects_manifest_delete() {
    let tmp = tempfile::tempdir().unwrap();
    let plugins_dir = tmp.path().join("plugins");
    std::fs::create_dir_all(&plugins_dir).unwrap();

    write_plugin_dir(&plugins_dir, "del-plugin");

    let registry = PluginRegistry::new();
    let plugin = rua_plugin_runtime::PluginLoader::load(&plugins_dir.join("del-plugin")).unwrap();
    registry.add(plugin).unwrap();

    let mut rx = registry.subscribe();

    let _watcher = PluginWatcher::start_with_debounce(
        plugins_dir.clone(),
        registry.clone(),
        Duration::from_millis(200),
    )
    .unwrap();

    tokio::time::sleep(Duration::from_millis(100)).await;

    std::fs::remove_file(plugins_dir.join("del-plugin/plugin.json")).unwrap();

    let event = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("should receive event within 5s")
        .expect("channel not closed");

    assert!(
        matches!(event, PluginEvent::Uninstalled { ref plugin_id } if plugin_id == "del-plugin")
    );
    assert!(registry.is_empty().unwrap());
}

#[tokio::test]
async fn watcher_reloads_on_manifest_modify() {
    let tmp = tempfile::tempdir().unwrap();
    let plugins_dir = tmp.path().join("plugins");
    std::fs::create_dir_all(&plugins_dir).unwrap();

    write_plugin_dir(&plugins_dir, "mod-plugin");

    let registry = PluginRegistry::new();
    let plugin = rua_plugin_runtime::PluginLoader::load(&plugins_dir.join("mod-plugin")).unwrap();
    registry.add(plugin).unwrap();

    let mut rx = registry.subscribe();

    let _watcher = PluginWatcher::start_with_debounce(
        plugins_dir.clone(),
        registry.clone(),
        Duration::from_millis(200),
    )
    .unwrap();

    tokio::time::sleep(Duration::from_millis(100)).await;

    std::fs::write(
        plugins_dir.join("mod-plugin/plugin.json"),
        r#"{"id":"mod-plugin","name":"mod-plugin","version":"1.0.0","description":"test"}"#,
    )
    .unwrap();

    let mut got_uninstall = false;
    let mut got_install = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);

    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_secs(3), rx.recv()).await {
            Ok(Ok(PluginEvent::Uninstalled { ref plugin_id })) if plugin_id == "mod-plugin" => {
                got_uninstall = true;
            }
            Ok(Ok(PluginEvent::Installed { ref plugin_id })) if plugin_id == "mod-plugin" => {
                got_install = true;
            }
            _ => {}
        }
        if got_uninstall && got_install {
            break;
        }
    }

    assert!(got_uninstall, "should emit Uninstalled on reload");
    assert!(got_install, "should emit Installed on reload");
    assert_eq!(registry.list().unwrap(), vec!["mod-plugin"]);
}

#[tokio::test]
async fn watcher_broadcasts_on_add() {
    let tmp = tempfile::tempdir().unwrap();
    let plugins_dir = tmp.path().join("plugins");
    std::fs::create_dir_all(&plugins_dir).unwrap();

    let registry = PluginRegistry::new();
    let mut rx1 = registry.subscribe();
    let mut rx2 = registry.subscribe();

    let _watcher = PluginWatcher::start_with_debounce(
        plugins_dir.clone(),
        registry.clone(),
        Duration::from_millis(200),
    )
    .unwrap();

    tokio::time::sleep(Duration::from_millis(100)).await;

    write_plugin_dir(&plugins_dir, "bc-plugin");

    let e1 = tokio::time::timeout(Duration::from_secs(5), rx1.recv())
        .await
        .expect("rx1 timeout")
        .expect("rx1 closed");
    let e2 = tokio::time::timeout(Duration::from_secs(5), rx2.recv())
        .await
        .expect("rx2 timeout")
        .expect("rx2 closed");

    assert!(matches!(e1, PluginEvent::Installed { ref plugin_id } if plugin_id == "bc-plugin"));
    assert!(matches!(e2, PluginEvent::Installed { ref plugin_id } if plugin_id == "bc-plugin"));
}

#[tokio::test]
async fn watcher_detects_plugin_directory_installed_by_rename() {
    let tmp = tempfile::tempdir().unwrap();
    let plugins_dir = tmp.path().join("plugins");
    std::fs::create_dir_all(&plugins_dir).unwrap();
    let staging_dir = tmp.path().join("staging");
    write_plugin_dir(&staging_dir, "atomic-plugin");
    let registry = PluginRegistry::new();
    let mut rx = registry.subscribe();
    let _watcher = PluginWatcher::start_with_debounce(
        plugins_dir.clone(),
        registry.clone(),
        Duration::from_millis(200),
    )
    .unwrap();

    std::fs::rename(
        staging_dir.join("atomic-plugin"),
        plugins_dir.join("atomic-plugin"),
    )
    .unwrap();

    let event = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("installed plugin directory must be discovered")
        .unwrap();
    assert!(
        matches!(event, PluginEvent::Installed { ref plugin_id } if plugin_id == "atomic-plugin")
    );
    assert_eq!(registry.list().unwrap(), vec!["atomic-plugin"]);
}
