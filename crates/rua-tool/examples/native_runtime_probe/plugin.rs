use rua_plugin_runtime::{ActivePluginRegistry, PluginClient, PluginClientConfig};
use serde_json::json;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
async fn pids(path: &Path) -> Result<Vec<u32>> {
    for _ in 0..100 {
        let values: Vec<u32> = std::fs::read_to_string(path)
            .unwrap_or_default()
            .split_whitespace()
            .filter_map(|s| s.parse().ok())
            .collect();
        if values.len() == 2 {
            return Ok(values);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Err(format!("children did not start: {}", path.display()).into())
}
async fn gone(pids: &[u32]) -> Result<()> {
    for _ in 0..100 {
        if pids
            .iter()
            .all(|pid| !Path::new(&format!("/proc/{pid}")).exists())
        {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Err(format!("plugin descendants survived: {pids:?}").into())
}
fn config(workspace: &Path) -> PluginClientConfig {
    let mut cfg = PluginClientConfig::new("native-plugin", "fixture");
    cfg.command = Some("python3".into());
    cfg.args = Some(vec!["fixture.py".into()]);
    cfg.working_dir = workspace.to_owned();
    cfg.runtime_cache_dir = workspace.join("cache");
    cfg.call_timeout = Duration::from_secs(1);
    cfg.heartbeat_interval = Duration::from_secs(120);
    cfg.env
        .insert("PLUGIN_SECRET".into(), "fixture-secret".into());
    cfg.env
        .insert("PLUGIN_REMOVE".into(), "declared-value".into());
    cfg.env_remove.insert("PLUGIN_REMOVE".into());
    cfg
}
pub async fn run() -> Result<()> {
    let home = PathBuf::from(std::env::var("HOME")?);
    let workspace = home.join(".rua/workspaces/default/native-plugin-acceptance");
    std::fs::create_dir_all(&workspace)?;
    let mut fixture = tempfile::NamedTempFile::new_in(&workspace)?;
    use std::io::Write;
    fixture.write_all(include_bytes!(
        "../../../rua-plugin-runtime/tests/fixtures/native_android_plugin.py"
    ))?;
    fixture.as_file().sync_all()?;
    fixture.persist(workspace.join("fixture.py"))?;
    std::fs::File::open(&workspace)?.sync_all()?;
    std::env::set_var("PLUGIN_REMOVE", "inherited-value");
    let cfg = config(&workspace);
    let data = PathBuf::from(std::env::var("RUA_ANDROID_DATA_DIR")?);
    std::env::set_var("RUA_ANDROID_DATA_DIR", workspace.join("missing-data"));
    let missing = PluginClient::spawn(cfg.clone()).await;
    std::env::set_var("RUA_ANDROID_DATA_DIR", &data);
    assert!(missing
        .err()
        .ok_or("missing runtime launched")?
        .to_string()
        .contains("Linux Compatibility Runtime"));
    let python = data.join("runtime/debian/usr/bin/python3");
    let backup = python.with_file_name("python3.native-plugin-test");
    assert!(!backup.exists());
    std::fs::rename(&python, &backup)?;
    let missing = PluginClient::spawn(cfg.clone()).await;
    std::fs::rename(&backup, &python)?;
    assert!(missing
        .err()
        .ok_or("missing Python launched")?
        .to_string()
        .contains("python3"));
    assert_eq!(cfg.command.as_deref(), Some("python3"));
    let mut client = PluginClient::spawn(cfg.clone()).await?;
    assert_eq!(
        client.initialize_result().await?.server_name,
        "native-plugin"
    );
    assert_eq!(client.list_tools().await?.len(), 2);
    assert!(client
        .call_tool("probe", json!({}))
        .await?
        .text()
        .contains("NATIVE_PLUGIN_OK"));
    let output: serde_json::Value =
        serde_json::from_slice(&std::fs::read(workspace.join("plugin-output.json"))?)?;
    assert_eq!(output["nested"], "Linux");
    assert_eq!(output["python"], "/usr/bin/python3");
    assert_eq!(
        client
            .invoke_cli(rua_plugin_runtime::protocol::InvokeCliParams {
                plugin_id: "native-plugin".into(),
                cmd_name: "probe".into(),
                args: json!({})
            })
            .await?
            .output,
        "NATIVE_PLUGIN_CLI_OK"
    );
    let server_pids = pids(&workspace.join("plugin-server-pids")).await?;
    let native = std::path::PathBuf::from(std::env::var("RUA_ANDROID_NATIVE_DIR")?);
    let runtime = home.parent().ok_or("app files path")?.join("runtime");
    let busy = std::process::Command::new(native.join("librua-runtime-gate.so"))
        .arg("--exclusive")
        .arg(runtime.join("use.lock"))
        .arg(runtime.join("update.pending"))
        .output()?;
    assert_eq!(
        busy.status.code(),
        Some(75),
        "Live Plugin must prevent updates"
    );
    let marker = workspace.join("plugin-call-pids");
    let _ = std::fs::remove_file(&marker);
    assert!(client.call_tool("wait", json!({})).await.is_err());
    gone(&pids(&marker).await?).await?;
    client.shutdown().await?;
    gone(&server_pids).await?;

    let _ = std::fs::remove_file(&marker);
    let client = Arc::new(PluginClient::spawn(cfg.clone()).await?);
    let caller = Arc::clone(&client);
    let pending = tokio::spawn(async move { caller.call_tool("wait", json!({})).await });
    let children = pids(&marker).await?;
    pending.abort();
    let _ = pending.await;
    gone(&children).await?;
    let mut client = Arc::try_unwrap(client).map_err(|_| "call retained client")?;
    client.shutdown().await?;

    // Same registry owner called by the resident shutdown path, including an in-flight request.
    let registry = Arc::new(ActivePluginRegistry::new());
    let mut long = cfg;
    long.call_timeout = Duration::from_secs(120);
    registry
        .insert("native-plugin".into(), PluginClient::spawn(long).await?)
        .await?;
    let server_pids = pids(&workspace.join("plugin-server-pids")).await?;
    let _ = std::fs::remove_file(&marker);
    let caller = Arc::clone(&registry);
    let pending = tokio::spawn(async move {
        caller
            .call_plugin_tool("native-plugin", "wait", json!({}))
            .await
    });
    let children = pids(&marker).await?;
    tokio::time::timeout(Duration::from_secs(5), registry.shutdown_all()).await?;
    assert!(pending.await?.is_err());
    gone(&server_pids).await?;
    gone(&children).await?;
    assert!(registry.is_empty().await);
    println!("NATIVE_PLUGIN_ALL_PASS");
    Ok(())
}
