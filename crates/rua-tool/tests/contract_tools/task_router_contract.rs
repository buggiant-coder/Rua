use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rua_contract::Tool;
use rua_core::tool::{ToolContext, ToolResult};
use rua_task::{TaskListFilter, TaskOwner, TaskRuntime, TaskRuntimeConfig};
use rua_tool::extended::task_tools::{
    TaskAwaitTool, TaskCancelTool, TaskCreateTool, TaskListTool, TaskReadTool, TaskRetryTool,
    TaskRuntimeHandle, TaskSendInputTool, TaskToolRouter, TaskUnwatchTool, TaskWatchTool,
};
use serde_json::{json, Value};

type RoutedCall = (String, Value, String, String, bool);

#[derive(Default)]
struct Router {
    calls: Mutex<Vec<RoutedCall>>,
    decline: bool,
    fail: bool,
}

#[async_trait]
impl TaskToolRouter for Router {
    async fn execute(
        &self,
        name: &str,
        args: Value,
        ctx: &ToolContext,
    ) -> Result<Option<ToolResult>, String> {
        self.calls.lock().unwrap().push((
            name.into(),
            args,
            ctx.session_id.clone(),
            ctx.agent_id.clone(),
            ctx.cancel.as_ref().is_some_and(|cancel| *cancel.borrow()),
        ));
        if self.fail {
            return Err("execution Device is offline".into());
        }
        Ok((!self.decline).then(|| {
            let mut result = ToolResult::text("original-call", "routed Task output");
            result
                .metadata
                .insert("cursor".into(), json!("target-owned-cursor"));
            result
        }))
    }
}

fn context(runtime: Arc<TaskRuntime>, router: Arc<Router>) -> ToolContext {
    ToolContext {
        session_id: "owning-chat".into(),
        agent_id: "selected-agent".into(),
        task_runtime: Some(Arc::new(
            TaskRuntimeHandle::new(runtime).with_router(router),
        )),
        ..Default::default()
    }
}

#[tokio::test]
async fn every_task_control_routes_exact_arguments_and_keeps_the_original_context() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = Arc::new(TaskRuntime::open(TaskRuntimeConfig::new(dir.path())).unwrap());
    let router = Arc::new(Router::default());
    let mut ctx = context(runtime.clone(), router.clone());
    let (_cancel, receiver) = tokio::sync::watch::channel(true);
    ctx.cancel = Some(receiver);
    let tools: Vec<Box<dyn Tool>> = vec![
        Box::new(TaskCreateTool),
        Box::new(TaskSendInputTool),
        Box::new(TaskReadTool),
        Box::new(TaskListTool),
        Box::new(TaskRetryTool),
        Box::new(TaskCancelTool),
        Box::new(TaskWatchTool),
        Box::new(TaskUnwatchTool),
        Box::new(TaskAwaitTool),
    ];
    let args = json!({"task_id":"remote-task","cursor":"opaque-target-cursor","future_option":{"preserve":true}});
    for tool in tools {
        let result = tool.call(args.clone(), ctx.clone()).await.unwrap();
        assert_eq!(result.call_id, "original-call");
        assert_eq!(result.text_content(), "routed Task output");
        assert_eq!(result.metadata["cursor"], "target-owned-cursor");
        let calls = router.calls.lock().unwrap();
        assert_eq!(
            calls.last().unwrap(),
            &(
                tool.name().into(),
                args.clone(),
                "owning-chat".into(),
                "selected-agent".into(),
                true
            )
        );
    }
    assert_eq!(router.calls.lock().unwrap().len(), 9);
    assert!(runtime
        .list(&TaskOwner::new("owning-chat"), TaskListFilter::default())
        .await
        .unwrap()
        .is_empty());
    runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn local_decline_keeps_existing_list_behavior_but_remote_failure_never_falls_back() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = Arc::new(TaskRuntime::open(TaskRuntimeConfig::new(dir.path())).unwrap());
    let local = Arc::new(Router {
        decline: true,
        ..Default::default()
    });
    let result = TaskListTool
        .call(json!({}), context(runtime.clone(), local.clone()))
        .await
        .unwrap();
    let body: Value = serde_json::from_str(&result.text_content()).unwrap();
    assert_eq!(body["tasks"], json!([]));
    assert_eq!(body["has_more"], false);
    assert_eq!(local.calls.lock().unwrap().len(), 1);

    let remote = Arc::new(Router {
        fail: true,
        ..Default::default()
    });
    let error = TaskCreateTool.call(
        json!({"runner":{"type":"tool","tool_name":"bash","arguments":{"device_id":"phone","command":"echo should-not-run"}}}),
        context(runtime.clone(), remote),
    ).await.unwrap_err();
    assert!(error.to_string().contains("execution Device is offline"));
    assert!(runtime
        .list(&TaskOwner::new("owning-chat"), TaskListFilter::default())
        .await
        .unwrap()
        .is_empty());
    runtime.shutdown().await.unwrap();
}
