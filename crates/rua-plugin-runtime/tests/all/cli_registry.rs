use std::io::Write;

use rua_plugin_runtime::cli_registry::CliRegistry;
use rua_plugin_runtime::{CliArgSpec, CliCommandSpec, PluginCliCommand};
use tempfile::TempDir;

fn make_cmd(plugin_id: &str, name: &str) -> PluginCliCommand {
    PluginCliCommand {
        plugin_id: plugin_id.to_string(),
        spec: CliCommandSpec {
            name: name.to_string(),
            description: format!("{name} command"),
            args: vec![CliArgSpec {
                name: "target".to_string(),
                description: "Target".to_string(),
                required: false,
            }],
            callback: format!("handle_{name}"),
        },
    }
}

fn create_plugin_dir(dir: &TempDir, plugin_id: &str, cli_commands: &[CliCommandSpec]) {
    let plugin_dir = dir.path().join(plugin_id);
    std::fs::create_dir_all(&plugin_dir).unwrap();

    let manifest = serde_json::json!({
        "id": plugin_id,
        "name": plugin_id,
        "version": "1.0.0",
        "entry_point": "plugin.py",
        "cli_commands": cli_commands,
    });

    let mut f = std::fs::File::create(plugin_dir.join("plugin.json")).unwrap();
    f.write_all(serde_json::to_string(&manifest).unwrap().as_bytes())
        .unwrap();
}

#[test]
fn empty_registry() {
    let reg = CliRegistry::new();
    assert!(reg.commands().is_empty());
    assert!(reg.find_command("anything").is_none());
}

#[test]
fn register_and_find() {
    let mut reg = CliRegistry::new();
    let cmd = make_cmd("p1", "greet");
    reg.register(cmd, "/tmp/p1".into());

    assert_eq!(reg.commands().len(), 1);
    let found = reg.find_command("greet").unwrap();
    assert_eq!(found.plugin_id, "p1");
    assert_eq!(found.spec.name, "greet");
}

#[test]
fn find_command_not_found() {
    let reg = CliRegistry::new();
    assert!(reg.find_command("nope").is_none());
}

#[test]
fn unregister_plugin_removes_commands() {
    let mut reg = CliRegistry::new();
    reg.register(make_cmd("p1", "greet"), "/tmp/p1".into());
    reg.register(make_cmd("p1", "farewell"), "/tmp/p1".into());
    reg.register(make_cmd("p2", "search"), "/tmp/p2".into());

    assert_eq!(reg.commands().len(), 3);

    reg.unregister_plugin("p1");

    assert_eq!(reg.commands().len(), 1);
    assert!(reg.find_command("greet").is_none());
    assert!(reg.find_command("farewell").is_none());
    assert!(reg.find_command("search").is_some());
}

#[test]
fn unregister_nonexistent_plugin_is_noop() {
    let mut reg = CliRegistry::new();
    reg.register(make_cmd("p1", "greet"), "/tmp/p1".into());
    reg.unregister_plugin("nonexistent");
    assert_eq!(reg.commands().len(), 1);
}

#[test]
fn from_plugins_dir_scans_manifests() {
    let dir = TempDir::new().unwrap();

    let cli_cmds = vec![CliCommandSpec {
        name: "hello".to_string(),
        description: "Say hello".to_string(),
        args: vec![],
        callback: "handle_hello".to_string(),
    }];
    create_plugin_dir(&dir, "test-plugin", &cli_cmds);

    let reg = CliRegistry::from_plugins_dir(dir.path());

    assert_eq!(reg.commands().len(), 1);
    let cmd = reg.find_command("hello").unwrap();
    assert_eq!(cmd.plugin_id, "test-plugin");
    assert_eq!(cmd.spec.callback, "handle_hello");
}

#[test]
fn from_plugins_dir_multiple_plugins() {
    let dir = TempDir::new().unwrap();

    create_plugin_dir(
        &dir,
        "plugin-a",
        &[CliCommandSpec {
            name: "cmd-a".to_string(),
            description: "A".to_string(),
            args: vec![],
            callback: "a".to_string(),
        }],
    );
    create_plugin_dir(
        &dir,
        "plugin-b",
        &[CliCommandSpec {
            name: "cmd-b".to_string(),
            description: "B".to_string(),
            args: vec![],
            callback: "b".to_string(),
        }],
    );

    let reg = CliRegistry::from_plugins_dir(dir.path());
    assert_eq!(reg.commands().len(), 2);
    assert!(reg.find_command("cmd-a").is_some());
    assert!(reg.find_command("cmd-b").is_some());
}

#[test]
fn from_plugins_dir_skips_bad_manifest() {
    let dir = TempDir::new().unwrap();

    let bad_dir = dir.path().join("bad-plugin");
    std::fs::create_dir_all(&bad_dir).unwrap();
    std::fs::write(bad_dir.join("plugin.json"), "not json").unwrap();

    create_plugin_dir(
        &dir,
        "good-plugin",
        &[CliCommandSpec {
            name: "good".to_string(),
            description: "Good".to_string(),
            args: vec![],
            callback: "handle".to_string(),
        }],
    );

    let reg = CliRegistry::from_plugins_dir(dir.path());
    assert_eq!(reg.commands().len(), 1);
    assert!(reg.find_command("good").is_some());
}

#[test]
fn from_plugins_dir_nonexistent() {
    let reg = CliRegistry::from_plugins_dir(std::path::Path::new("/nonexistent/path"));
    assert!(reg.commands().is_empty());
}

#[test]
fn disable_removes_from_registry() {
    let dir = TempDir::new().unwrap();

    create_plugin_dir(
        &dir,
        "my-plugin",
        &[CliCommandSpec {
            name: "greet".to_string(),
            description: "Greet".to_string(),
            args: vec![],
            callback: "handle_greet".to_string(),
        }],
    );

    let mut reg = CliRegistry::from_plugins_dir(dir.path());
    assert!(reg.find_command("greet").is_some());

    reg.unregister_plugin("my-plugin");
    assert!(reg.find_command("greet").is_none());
    assert!(reg.commands().is_empty());
}
