//! On-device acceptance for real Python and explicit Shell executor cancellation/deadlines.
#[path = "native_runtime_probe/plugin.rs"]
mod plugin;

use rua_contract::Tool;
use rua_core::tool::ToolContext;
use rua_tool::{
    builtin::ExecutePythonCodeTool, executor::default_command_executor, CommandRequest,
};
use serde_json::json;
use std::{path::PathBuf, time::Duration};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::args().any(|arg| arg == "--inventory-update") {
        use std::io::Write;
        let data = PathBuf::from(std::env::var("RUA_ANDROID_DATA_DIR")?);
        let tool = rua_tool::builtin::RuntimeGetRuntimeInfoTool::new();
        let python = ExecutePythonCodeTool::new();
        for version in ["v24.17.0", "v24.18.0"] {
            if version == "v24.18.0" {
                let mut line = String::new();
                assert!(std::io::stdin().read_line(&mut line)? > 0);
            }
            let result = tool
                .call(
                    json!({}),
                    ToolContext {
                        working_dir: data.join("home"),
                        ..Default::default()
                    },
                )
                .await?;
            let value: serde_json::Value = serde_json::from_str(&result.text_content())?;
            assert_eq!(value["capabilities"]["node"]["version"], version);
            let executed = python.call(json!({
                "code": "import subprocess; print(subprocess.check_output(['node','--version'], text=True).strip())",
                "timeout_secs": 20
            }), ToolContext {
                working_dir: PathBuf::from(std::env::var("HOME")?),
                tool_result_store_dir: Some(data.join("tmp")),
                ..Default::default()
            }).await?;
            assert!(
                !executed.is_error && executed.text_content().contains(version),
                "{}",
                executed.text_content()
            );
            println!("INVENTORY_{version}_READY");
            std::io::stdout().flush()?;
        }
        return Ok(());
    }
    if std::env::args().any(|arg| arg == "--plugin") {
        return plugin::run().await;
    }
    let home = PathBuf::from(std::env::var("HOME")?);
    let store = home.join("probe-results");
    std::fs::create_dir_all(&store)?;
    let result = ExecutePythonCodeTool::new().call(json!({
        "code": "import sys, os; print('NATIVE_PROBE_PYTHON_OK'); print(sys.version); print(os.getcwd())",
        "timeout_secs": 30
    }), ToolContext {
        agent_id: "native-prototype".into(), session_id: "probe".into(),
        working_dir: home.clone(), tool_result_store_dir: Some(store.clone()),
        ..Default::default()
    }).await?;
    println!("PYTHON_RESULT {}", result.text_content());
    if result.is_error || !result.text_content().contains("NATIVE_PROBE_PYTHON_OK") {
        return Err("Python tool failed".into());
    }
    for language in ["python", "node"] {
        for cancel_run in [true, false] {
            let marker = home.join(if cancel_run {
                "native-probe-cancel-pids"
            } else {
                "native-probe-timeout-pids"
            });
            let _ = std::fs::remove_file(&marker);
            let code = format!("import os, subprocess, time; p=subprocess.Popen(['/usr/bin/python3','-c','import time; time.sleep(120)']); open({:?},'w').write(str(os.getpid())+' '+str(p.pid)); time.sleep(120)", marker.to_str().ok_or("path")?);
            let command = format!("/usr/bin/python3 -c '{}'", code.replace('\'', "'\"'\"'"));
            let command = if language == "node" {
                let code = format!("const fs=require('node:fs'); const {{spawn}}=require('node:child_process'); const child=spawn(process.execPath,['-e','setInterval(()=>{{}},1000)']); fs.writeFileSync({},process.pid+' '+child.pid); setInterval(()=>{{}},1000)", serde_json::to_string(marker.to_str().ok_or("path")?)?);
                format!("node -e '{}'", code.replace('\'', "'\"'\"'"))
            } else {
                command
            };
            let (cancel, rx) = tokio::sync::watch::channel(false);
            let executor = default_command_executor();
            let req = CommandRequest {
                command,
                working_dir: home.clone(),
                tool_result_store_dir: store.clone(),
                timeout: Duration::from_secs(if cancel_run { 40 } else { 3 }),
                env: vec![],
                env_remove: vec![],
                raw_capture_max_bytes: 65536,
                raw_capture_id: None,
                task_session: None,
            };
            let task = tokio::spawn(async move { executor.execute(req, rx).await });
            for _ in 0..200 {
                if std::fs::read_to_string(&marker).is_ok_and(|pids| {
                    pids.split_whitespace()
                        .filter(|p| p.parse::<u32>().is_ok())
                        .count()
                        == 2
                }) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            if !marker.exists() {
                cancel.send(true)?;
                let output = task.await?;
                return Err(format!("child did not start: {output:?}").into());
            }
            let pids = std::fs::read_to_string(&marker)?;
            if pids.split_whitespace().count() != 2 {
                return Err("Incomplete PID marker".into());
            }
            println!("CANCEL_STARTED {language} {pids}");
            if cancel_run {
                cancel.send(true)?;
            }
            let output = tokio::time::timeout(Duration::from_secs(15), task).await???;
            println!(
                "CANCEL_RESULT cancelled={} exit={:?}",
                output.cancelled, output.exit_code
            );
            if (cancel_run && !output.cancelled) || (!cancel_run && !output.timed_out) {
                return Err("deadline/cancellation was not observed".into());
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
            for pid in pids.split_whitespace() {
                if let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) {
                    return Err(format!("descendant remains: {stat}").into());
                }
            }
        }
    }
    println!("NATIVE_PROBE_ALL_PASS");
    Ok(())
}
