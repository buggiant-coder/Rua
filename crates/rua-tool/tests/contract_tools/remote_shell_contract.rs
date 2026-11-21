use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rua_contract::Tool;
use rua_core::tool::{ToolContext, ToolResult};
use rua_tool::core::shell::{RemoteCommandRuntime, RemoteCommandRuntimeHandle};
use rua_tool::{MockCommandExecutor, ShellTool};
use serde_json::{json, Value};

#[derive(Default)]
struct Remote {
    calls: Mutex<Vec<(Value, String, String)>>,
}

#[async_trait]
impl RemoteCommandRuntime for Remote {
    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult, String> {
        self.calls
            .lock()
            .unwrap()
            .push((args, ctx.agent_id.clone(), ctx.session_id.clone()));
        Ok(ToolResult::text("remote", "remote result"))
    }
}

fn context(remote: Arc<Remote>) -> ToolContext {
    ToolContext {
        local_device_id: Some("desktop".into()),
        agent_id: "chosen-agent".into(),
        session_id: "original-chat".into(),
        working_dir: "/not/a/local/directory".into(),
        remote_command_runtime: Some(Arc::new(RemoteCommandRuntimeHandle::new(remote))),
        ..Default::default()
    }
}

#[tokio::test]
async fn remote_shell_routes_before_local_paths_and_preserves_exact_arguments_and_chat() {
    let local = Arc::new(MockCommandExecutor::new());
    let remote = Arc::new(Remote::default());
    let args = json!({"command":"printf 'first\n第二行'", "device_id":"phone", "cwd":"/root",
        "yield_seconds":1, "future_option":{"keep":true}});
    let result = ShellTool::new(local.clone())
        .call(args.clone(), context(remote.clone()))
        .await
        .unwrap();
    assert_eq!(result.text_content(), "remote result");
    assert!(local.history().is_empty());
    assert_eq!(
        *remote.calls.lock().unwrap(),
        vec![(args, "chosen-agent".into(), "original-chat".into())]
    );
}

#[tokio::test]
async fn unavailable_or_invalid_remote_requests_never_fall_back_to_local_execution() {
    let dir = tempfile::tempdir().unwrap();
    let local = Arc::new(MockCommandExecutor::new());
    let remote = Arc::new(Remote::default());
    let tool = ShellTool::new(local.clone());
    let mut ctx = context(remote.clone());
    ctx.working_dir = dir.path().into();
    ctx.remote_command_runtime = None;
    let error = tool
        .call(
            json!({"command":"echo should-not-run","device_id":"phone"}),
            ctx.clone(),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("unavailable"));
    ctx.remote_command_runtime = context(remote.clone()).remote_command_runtime;
    for args in [
        json!({"command":"echo nope","device_id":""}),
        json!({"command":"echo nope","device_id":17}),
        json!({"command":"echo nope","device_id":"phone","cwd":false}),
        json!({"command":"echo nope","device_id":"phone","cwd":"/a\0b"}),
        json!({"command":"echo nope","device_id":"phone","yield_seconds":"2"}),
    ] {
        assert!(tool.call(args, ctx.clone()).await.is_err());
    }
    assert!(local.history().is_empty());
    assert!(remote.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn precancelled_remote_command_does_not_dispatch() {
    let remote = Arc::new(Remote::default());
    let local = Arc::new(MockCommandExecutor::new());
    let mut ctx = context(remote.clone());
    let (_tx, rx) = tokio::sync::watch::channel(true);
    ctx.cancel = Some(rx);
    assert!(matches!(
        ShellTool::new(local.clone())
            .call(json!({"command":"run", "device_id":"phone"}), ctx)
            .await,
        Err(rua_core::RuaError::Cancelled)
    ));
    assert!(local.history().is_empty());
    assert!(remote.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn explicit_local_device_keeps_local_executor_and_records_resolved_directory() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("work")).unwrap();
    let local = Arc::new(MockCommandExecutor::new());
    let remote = Arc::new(Remote::default());
    let mut ctx = context(remote.clone());
    ctx.working_dir = dir.path().into();
    ctx.tool_result_store_dir = Some(dir.path().join("results"));
    let result = ShellTool::new(local.clone())
        .call(
            json!({"command":"pwd", "device_id":"desktop", "cwd":"work"}),
            ctx,
        )
        .await
        .unwrap();
    let expected = dir.path().join("work").canonicalize().unwrap();
    assert_eq!(local.history()[0].working_dir, expected);
    assert_eq!(
        result.metadata["execution_location"],
        json!({"device_id":"desktop","cwd":expected})
    );
    assert!(remote.calls.lock().unwrap().is_empty());
}
