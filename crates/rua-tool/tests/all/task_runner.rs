use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use rua_contract::Tool;
use rua_core::tool::ToolContext;
use rua_task::{
    TaskInput, TaskInputCapability, TaskInputError, TaskInputSink, TaskListFilter,
    TaskLiveOutputBatch, TaskLiveOutputCursor, TaskLiveOutputError, TaskLiveOutputSource,
    TaskNotificationSink, TaskOwner, TaskRunOutput, TaskRunRequest, TaskRunner, TaskRunnerError,
    TaskRunnerKind, TaskRuntime, TaskRuntimeConfig, TaskState, TaskWatchOutputNotification,
};
use rua_tool::extended::task_tools::{
    TaskAwaitTool, TaskCancelTool, TaskCreateTool, TaskListTool, TaskReadTool, TaskRetryTool,
    TaskRunnerSessionHandle, TaskRuntimeHandle, TaskSendInputTool, TaskUnwatchTool, TaskWatchTool,
};
use rua_tool::{
    RuntimeCommandExecutor, ShellTool, ToolTaskExecution, ToolTaskExecutionRequest, ToolTaskRunner,
};
use tokio::sync::{Mutex, Notify};

struct RecordingExecution {
    completed: Notify,
}

struct LongResultExecution;

#[tokio::test]
async fn blocked_task_receipt_preserves_dependency_failure_within_budget() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = Arc::new(TaskRuntime::open(TaskRuntimeConfig::new(dir.path())).unwrap());
    runtime.register_runner(Arc::new(ToolTaskRunner::new(Arc::new(LongResultExecution))));
    let ctx = context(runtime.clone(), "agent-a", "chat-a");
    let first = budgeted_receipt(
        &TaskCreateTool,
        serde_json::json!({"runner":{"type":"tool","tool_name":"LongResult","arguments":{}}}),
        ctx.clone(),
        false,
    )
    .await;
    let upstream = first["task_id"].as_str().unwrap();
    let owner = TaskOwner::new("agent-a", "chat-a");
    wait_for_terminal(&runtime, &owner, upstream).await;
    let child = budgeted_receipt(&TaskCreateTool, serde_json::json!({"runner":{"type":"tool","tool_name":"LongResult","arguments":{}},"depends_on":[upstream]}), ctx.clone(), false).await;
    let task_id = child["task_id"].as_str().unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if runtime.get(&owner, task_id).await.unwrap().state == TaskState::Blocked {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let read = budgeted_receipt(
        &TaskReadTool,
        serde_json::json!({"task_id":task_id}),
        ctx.clone(),
        false,
    )
    .await;
    assert_eq!(read["status"], "blocked");
    assert!(read.to_string().contains(upstream));
    let dependency = budgeted_receipt(
        &TaskReadTool,
        serde_json::json!({"task_id":upstream,"max_bytes":1024}),
        ctx.clone(),
        false,
    )
    .await;
    assert_eq!(dependency["status"], "failed");
    assert!(dependency["output"].is_array());
    assert_eq!(dependency["has_more"], true);
    let list = budgeted_receipt(
        &TaskListTool,
        serde_json::json!({"state":"blocked"}),
        ctx.clone(),
        false,
    )
    .await;
    assert_eq!(list["tasks"][0]["status"], "blocked");
    assert!(list.to_string().contains(upstream));
    budgeted_receipt(
        &TaskCancelTool,
        serde_json::json!({"task_id":task_id}),
        ctx,
        false,
    )
    .await;
    runtime.shutdown().await.unwrap();
}

async fn budgeted_receipt(
    tool: &dyn Tool,
    args: serde_json::Value,
    ctx: ToolContext,
    expect_error: bool,
) -> serde_json::Value {
    let name = tool.schema().name;
    let called = tool.call(args.clone(), ctx).await;
    assert_eq!(called.is_err(), expect_error, "{name}: {called:?}");
    let mut result =
        called.unwrap_or_else(|error| agent_types::ToolResult::error(&name, error.to_string()));
    let original = result.text_content();
    let archive = tempfile::tempdir().unwrap();
    let processor = rua_tool::result_store::ToolResultOutputProcessor::new_with_limits(
        archive.path(),
        10_000,
        1024 * 1024,
        4.0,
    );
    assert!(
        !processor
            .process("receipt-budget", &name, &args, &mut result)
            .await
    );
    assert_eq!(result.text_content(), original);
    let tokens = result.metadata["tool_output"]["visible_tokens"]
        .as_u64()
        .unwrap();
    assert!(tokens <= 10_000);
    println!("{name} error={expect_error} visible_tokens={tokens}");
    if expect_error {
        assert!(result.is_error);
        assert!(!original.is_empty());
        serde_json::Value::String(original)
    } else {
        serde_json::from_str(&original).unwrap()
    }
}

#[tokio::test]
async fn six_task_tool_receipts_preserve_control_evidence_within_model_budget() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = Arc::new(TaskRuntime::open(TaskRuntimeConfig::new(dir.path())).unwrap());
    let runner = Arc::new(ProjectControlRunner {
        inputs: Arc::new(Mutex::new(Vec::new())),
        started: Notify::new(),
    });
    runtime.register_runner(runner.clone());
    let ctx = context(runtime.clone(), "agent-a", "chat-a");
    let created = budgeted_receipt(&TaskCreateTool, serde_json::json!({"instruction":"授权的局部工作".repeat(1000),"runner":{"type":"tool","tool_name":"fixture","arguments":{}}}), ctx.clone(), false).await;
    let task_id = created["task_id"].as_str().unwrap();
    assert_eq!(created["attempt_no"], 1);
    assert!(created.get("instruction").is_none());
    tokio::time::timeout(Duration::from_secs(2), runner.started.notified())
        .await
        .unwrap();
    let listed = budgeted_receipt(
        &TaskListTool,
        serde_json::json!({"limit":1}),
        ctx.clone(),
        false,
    )
    .await;
    assert_eq!(listed["tasks"][0]["task_id"], task_id);
    assert_eq!(listed["tasks"][0]["instruction_truncated"], true);
    let read = budgeted_receipt(
        &TaskReadTool,
        serde_json::json!({"task_id":task_id}),
        ctx.clone(),
        false,
    )
    .await;
    assert_eq!(read["status"], "running");
    let sent = budgeted_receipt(
        &TaskSendInputTool,
        serde_json::json!({"task_id":task_id,"text":"bounded input\n"}),
        ctx.clone(),
        false,
    )
    .await;
    assert_eq!(sent["accepted"], true);
    let eof = budgeted_receipt(
        &TaskSendInputTool,
        serde_json::json!({"task_id":task_id,"close_stdin":true}),
        ctx.clone(),
        false,
    )
    .await;
    assert_eq!(eof["accepted"], true);
    let cancelled = budgeted_receipt(
        &TaskCancelTool,
        serde_json::json!({"task_id":task_id}),
        ctx.clone(),
        false,
    )
    .await;
    assert_eq!(cancelled["status"], "cancelled");
    let terminal = budgeted_receipt(
        &TaskReadTool,
        serde_json::json!({"task_id":task_id}),
        ctx.clone(),
        false,
    )
    .await;
    assert_eq!(terminal["status"], "cancelled");
    let retried = budgeted_receipt(
        &TaskRetryTool,
        serde_json::json!({"task_id":task_id}),
        ctx.clone(),
        false,
    )
    .await;
    assert_eq!(retried["attempt_no"], 2);
    tokio::time::timeout(Duration::from_secs(2), runner.started.notified())
        .await
        .unwrap();
    for tool in [
        &TaskReadTool as &dyn Tool,
        &TaskCancelTool,
        &TaskRetryTool,
        &TaskSendInputTool,
    ] {
        budgeted_receipt(
            tool,
            serde_json::json!({"task_id":task_id,"text":"denied"}),
            context(runtime.clone(), "agent-a", "other-chat"),
            true,
        )
        .await;
    }
    budgeted_receipt(
        &TaskCreateTool,
        serde_json::json!({"runner":{"type":"agent"}}),
        ctx.clone(),
        true,
    )
    .await;
    budgeted_receipt(
        &TaskListTool,
        serde_json::json!({}),
        ToolContext::default(),
        true,
    )
    .await;
    budgeted_receipt(
        &TaskSendInputTool,
        serde_json::json!({"task_id":task_id,"text":"invalid","close_stdin":true}),
        ctx.clone(),
        true,
    )
    .await;
    budgeted_receipt(
        &TaskRetryTool,
        serde_json::json!({"task_id":task_id}),
        ctx.clone(),
        true,
    )
    .await;
    budgeted_receipt(
        &TaskCancelTool,
        serde_json::json!({"task_id":task_id}),
        ctx,
        false,
    )
    .await;
    runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn task_read_reports_irrecoverable_raw_capture_limit() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = TaskRuntimeConfig::new(dir.path());
    config.limits.max_raw_result_bytes = 128;
    let runtime = Arc::new(TaskRuntime::open(config).unwrap());
    runtime.register_runner(Arc::new(ToolTaskRunner::new(Arc::new(LongResultExecution))));
    let ctx = context(runtime.clone(), "agent-a", "chat-a");
    let created = TaskCreateTool
        .call(
            serde_json::json!({"runner":{"type":"tool","tool_name":"LongResult","arguments":{}}}),
            ctx.clone(),
        )
        .await
        .unwrap();
    let created: serde_json::Value = serde_json::from_str(&created.text_content()).unwrap();
    let task_id = created["task_id"].as_str().unwrap();
    wait_for_terminal(&runtime, &TaskOwner::new("agent-a", "chat-a"), task_id).await;
    let result = TaskReadTool
        .call(serde_json::json!({"task_id":task_id}), ctx)
        .await
        .unwrap();
    let result: serde_json::Value = serde_json::from_str(&result.text_content()).unwrap();
    assert_eq!(result["truncation"]["reason"], "raw_capture_limit");
    assert_eq!(result["truncation"]["recoverable"], false);
    assert!(result["output"].is_array());
}

#[tokio::test]
async fn task_receipts_omit_instruction_and_list_bounds_descriptions() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = Arc::new(TaskRuntime::open(TaskRuntimeConfig::new(dir.path())).unwrap());
    runtime.register_runner(Arc::new(ToolTaskRunner::new(Arc::new(LongResultExecution))));
    let ctx = context(runtime.clone(), "agent-a", "chat-a");
    for _ in 0..2 {
        let reply = TaskCreateTool.call(serde_json::json!({"instruction":"描述".repeat(500),"runner":{"type":"tool","tool_name":"LongResult","arguments":{}}}), ctx.clone()).await.unwrap();
        let reply: serde_json::Value = serde_json::from_str(&reply.text_content()).unwrap();
        assert!(reply.get("instruction").is_none());
        assert!(reply.get("result").is_none());
        wait_for_terminal(
            &runtime,
            &TaskOwner::new("agent-a", "chat-a"),
            reply["task_id"].as_str().unwrap(),
        )
        .await;
    }
    let reply = TaskListTool
        .call(serde_json::json!({"limit":1}), ctx.clone())
        .await
        .unwrap();
    let reply: serde_json::Value = serde_json::from_str(&reply.text_content()).unwrap();
    assert_eq!(reply["has_more"], true);
    assert_eq!(reply["tasks"].as_array().unwrap().len(), 1);
    assert_eq!(
        reply["tasks"][0]["instruction"]
            .as_str()
            .unwrap()
            .chars()
            .count(),
        160
    );
    assert_eq!(reply["tasks"][0]["instruction_truncated"], true);
    let next = TaskListTool
        .call(
            serde_json::json!({"limit":1,"offset":reply["next_offset"]}),
            ctx,
        )
        .await
        .unwrap();
    let next: serde_json::Value = serde_json::from_str(&next.text_content()).unwrap();
    assert_eq!(next["has_more"], false);
    assert_ne!(next["tasks"][0]["task_id"], reply["tasks"][0]["task_id"]);
}

#[async_trait]
impl ToolTaskExecution for LongResultExecution {
    async fn execute(
        &self,
        request: ToolTaskExecutionRequest,
    ) -> Result<agent_types::ToolResult, String> {
        let mut result = agent_types::ToolResult::error(
            request.task_id,
            format!("\u{1b}[31m{}\u{1b}[0m", "长结果\t\n".repeat(3000)),
        );
        result
            .metadata
            .insert("exit_code".into(), serde_json::json!(7));
        Ok(result)
    }
}

#[tokio::test]
async fn task_read_pages_full_tool_result_not_preview_and_rejects_wrong_task_cursor() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = Arc::new(TaskRuntime::open(TaskRuntimeConfig::new(dir.path())).unwrap());
    runtime.register_runner(Arc::new(ToolTaskRunner::new(Arc::new(LongResultExecution))));
    let ctx = context(runtime.clone(), "agent-a", "chat-a");
    let spec =
        serde_json::json!({"runner":{"type":"tool","tool_name":"LongResult","arguments":{}}});
    let created = TaskCreateTool
        .call(spec.clone(), ctx.clone())
        .await
        .unwrap();
    let created: serde_json::Value = serde_json::from_str(&created.text_content()).unwrap();
    let task_id = created["task_id"].as_str().unwrap();
    wait_for_terminal(&runtime, &TaskOwner::new("agent-a", "chat-a"), task_id).await;
    let mut cursor = serde_json::Value::Null;
    let mut text = String::new();
    let archive = tempfile::tempdir().unwrap();
    let processor = rua_tool::result_store::ToolResultOutputProcessor::new_with_limits(
        archive.path(),
        10_000,
        1024 * 1024,
        4.0,
    );
    for _ in 0..100 {
        let mut reply = TaskReadTool
            .call(
                serde_json::json!({"task_id":task_id,"cursor":cursor,"max_bytes":1024}),
                ctx.clone(),
            )
            .await
            .unwrap();
        let original = reply.text_content();
        assert!(
            !reply.metadata.contains_key("task_terminal_observation"),
            "individual pages must not suppress the terminal preview"
        );
        assert!(
            !processor
                .process(
                    "task-page",
                    "task_read",
                    &serde_json::json!({"task_id":task_id,"cursor":cursor}),
                    &mut reply
                )
                .await
        );
        assert_eq!(reply.text_content(), original);
        assert!(
            reply.metadata["tool_output"]["visible_tokens"]
                .as_u64()
                .unwrap()
                <= 10_000
        );
        if cursor.is_null() {
            let replay = TaskReadTool
                .call(
                    serde_json::json!({"task_id":task_id,"max_bytes":1024}),
                    ctx.clone(),
                )
                .await
                .unwrap();
            assert_eq!(replay.text_content(), original);
            let mut limited = replay;
            let small = rua_tool::result_store::ToolResultOutputProcessor::new_with_limits(
                archive.path(),
                128,
                1024 * 1024,
                4.0,
            );
            assert!(
                small
                    .process(
                        "small-task-page",
                        "task_read",
                        &serde_json::json!({"task_id":task_id}),
                        &mut limited
                    )
                    .await
            );
            let key = limited
                .truncation
                .as_ref()
                .and_then(|meta| meta.recall_key.as_ref())
                .unwrap();
            let recalled = rua_tool::extended::recall::RecallToolResultTool
                .call(
                    serde_json::json!({"recall_key":key,"max_tokens":10_000}),
                    ToolContext {
                        tool_result_store_dir: Some(archive.path().into()),
                        ..Default::default()
                    },
                )
                .await
                .unwrap();
            assert!(!recalled.is_error);
            assert!(recalled.text_content().contains(&original));
        }
        let reply: serde_json::Value = serde_json::from_str(&reply.text_content()).unwrap();
        assert_eq!(reply["status"], "failed");
        assert_eq!(reply["exit_code"], 7);
        let mut page_bytes = 0;
        for record in reply["output"].as_array().into_iter().flatten() {
            let chunk = record["text"].as_str().unwrap();
            page_bytes += chunk.len();
            if record["source"] == "result" {
                text.push_str(chunk);
            }
        }
        assert!(page_bytes <= 1024);
        assert!(!text.contains('\u{1b}'));
        cursor = reply["cursor"].clone();
        if reply["has_more"] == false {
            break;
        }
    }
    assert_eq!(text, "长结果\t\n".repeat(3000));
    assert!(TaskReadTool
        .call(
            serde_json::json!({"task_id":task_id,"cursor":"not-a-valid-cursor"}),
            ctx.clone()
        )
        .await
        .is_err());
    let other = TaskCreateTool.call(spec, ctx.clone()).await.unwrap();
    let other: serde_json::Value = serde_json::from_str(&other.text_content()).unwrap();
    assert!(TaskReadTool
        .call(
            serde_json::json!({"task_id":other["task_id"],"cursor":cursor}),
            ctx.clone()
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("another Task"));
    wait_for_terminal(
        &runtime,
        &TaskOwner::new("agent-a", "chat-a"),
        other["task_id"].as_str().unwrap(),
    )
    .await;
}

struct ProjectControlInputSink {
    inputs: Arc<Mutex<Vec<TaskInput>>>,
    open: AtomicBool,
}

#[async_trait]
impl TaskInputSink for ProjectControlInputSink {
    fn declared_capability(&self) -> TaskInputCapability {
        TaskInputCapability::SteeringMessage
    }

    fn active_capability(&self) -> TaskInputCapability {
        if self.open.load(Ordering::SeqCst) {
            TaskInputCapability::SteeringMessage
        } else {
            TaskInputCapability::None
        }
    }

    async fn send_input(&self, input: &TaskInput) -> Result<(), TaskInputError> {
        if !self.open.load(Ordering::SeqCst) {
            return Err(TaskInputError::Unavailable);
        }
        self.inputs.lock().await.push(input.clone());
        Ok(())
    }

    async fn close(&self) {
        self.open.store(false, Ordering::SeqCst);
    }
}

#[derive(Default)]
struct ProjectControlLiveOutput {
    sealed: AtomicBool,
}

#[async_trait]
impl TaskLiveOutputSource for ProjectControlLiveOutput {
    async fn end_cursor(&self) -> Result<TaskLiveOutputCursor, TaskLiveOutputError> {
        Ok(TaskLiveOutputCursor::default())
    }

    async fn read_records(
        &self,
        cursor: &TaskLiveOutputCursor,
        _max_records: usize,
        _max_bytes: usize,
    ) -> Result<TaskLiveOutputBatch, TaskLiveOutputError> {
        Ok(TaskLiveOutputBatch {
            records: Vec::new(),
            next_cursor: cursor.clone(),
            sealed: self.sealed.load(Ordering::SeqCst),
            has_more: false,
        })
    }

    async fn seal(&self) {
        self.sealed.store(true, Ordering::SeqCst);
    }
}

struct ProjectControlRunner {
    inputs: Arc<Mutex<Vec<TaskInput>>>,
    started: Notify,
}

struct BoundedOutputSource {
    records: Vec<String>,
}

#[async_trait]
impl TaskLiveOutputSource for BoundedOutputSource {
    async fn end_cursor(&self) -> Result<TaskLiveOutputCursor, TaskLiveOutputError> {
        let mut cursor = TaskLiveOutputCursor::default();
        cursor
            .positions
            .insert("fixture".to_string(), self.records.len() as u64);
        Ok(cursor)
    }

    async fn read_records(
        &self,
        cursor: &TaskLiveOutputCursor,
        max_records: usize,
        max_bytes: usize,
    ) -> Result<TaskLiveOutputBatch, TaskLiveOutputError> {
        let start = cursor.positions.get("fixture").copied().unwrap_or(0) as usize;
        let mut records = Vec::new();
        let mut bytes = 0;
        let mut next = start;
        for text in self.records.iter().skip(start).take(max_records) {
            if !records.is_empty() && bytes + text.len() > max_bytes {
                break;
            }
            if records.is_empty() && text.len() > max_bytes {
                break;
            }
            bytes += text.len();
            records.push(rua_task::TaskLiveOutputRecord {
                source: "fixture".to_string(),
                text: text.clone(),
            });
            next += 1;
        }
        let mut next_cursor = cursor.clone();
        next_cursor
            .positions
            .insert("fixture".to_string(), next as u64);
        Ok(TaskLiveOutputBatch {
            records,
            next_cursor,
            sealed: false,
            has_more: next < self.records.len(),
        })
    }

    async fn seal(&self) {}
}

struct BoundedOutputRunner {
    started: Notify,
}

#[async_trait]
impl TaskRunner for BoundedOutputRunner {
    fn kind(&self) -> TaskRunnerKind {
        TaskRunnerKind::Tool
    }

    async fn run(&self, request: TaskRunRequest) -> Result<TaskRunOutput, TaskRunnerError> {
        request
            .session
            .activate_live_output(Arc::new(BoundedOutputSource {
                records: vec!["aaaa".into(), "bbbb".into(), "cccc".into()],
            }))
            .await
            .unwrap();
        self.started.notify_one();
        request.cancel.cancelled().await;
        let mut output = TaskRunOutput::cancelled();
        output.output = "full-output-must-not-be-inlined".to_string();
        Ok(output)
    }
}

#[async_trait]
impl TaskRunner for ProjectControlRunner {
    fn kind(&self) -> TaskRunnerKind {
        TaskRunnerKind::Tool
    }

    async fn run(&self, request: TaskRunRequest) -> Result<TaskRunOutput, TaskRunnerError> {
        request
            .session
            .activate(Arc::new(ProjectControlInputSink {
                inputs: Arc::clone(&self.inputs),
                open: AtomicBool::new(true),
            }))
            .await
            .unwrap();
        request
            .session
            .activate_live_output(Arc::new(ProjectControlLiveOutput::default()))
            .await
            .unwrap();
        self.started.notify_one();
        request.cancel.cancelled().await;
        Ok(TaskRunOutput::cancelled())
    }
}

#[derive(Default)]
struct RecordingWatchSink {
    notifications: Mutex<Vec<TaskWatchOutputNotification>>,
    changed: Notify,
}

#[async_trait]
impl TaskNotificationSink for RecordingWatchSink {
    async fn watch_output(
        &self,
        notification: TaskWatchOutputNotification,
    ) -> Result<(), TaskRunnerError> {
        self.notifications.lock().await.push(notification);
        self.changed.notify_one();
        Ok(())
    }

    async fn terminal(
        &self,
        _notification: rua_task::TaskCompletionNotification,
    ) -> Result<(), TaskRunnerError> {
        Ok(())
    }
}

#[async_trait]
impl ToolTaskExecution for RecordingExecution {
    async fn execute(
        &self,
        request: ToolTaskExecutionRequest,
    ) -> Result<agent_types::ToolResult, String> {
        assert_eq!(request.owner.agent_id(), Some("agent-a"));
        assert_eq!(request.owner.chat_id(), "chat-a");
        assert_eq!(request.tool_name, "Fixture_Read");
        assert_eq!(request.arguments["path"], "fixture.txt");
        self.completed.notify_waiters();
        Ok(agent_types::ToolResult::text(
            format!("{}:{}", request.task_id, request.attempt_no),
            "fixture contents",
        ))
    }
}

fn context(runtime: Arc<TaskRuntime>, agent_id: &str, chat_id: &str) -> ToolContext {
    ToolContext {
        agent_id: agent_id.to_string(),
        session_id: chat_id.to_string(),
        task_runtime: Some(Arc::new(TaskRuntimeHandle::new(runtime))),
        ..Default::default()
    }
}

fn project_context(
    runtime: Arc<TaskRuntime>,
    agent_id: &str,
    project_id: &str,
    chat_id: &str,
    tool_use_id: &str,
) -> ToolContext {
    let mut ctx = context(runtime, agent_id, chat_id);
    ctx.metadata.insert(
        "security_chat_address".to_string(),
        serde_json::to_value(rua_core::ChatAddress::project(project_id, chat_id)).unwrap(),
    );
    ctx.metadata.insert(
        "tool_use_id".to_string(),
        serde_json::Value::String(tool_use_id.to_string()),
    );
    ctx
}

#[tokio::test]
async fn task_read_is_cursor_based_bounded_and_pages_terminal_output() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = Arc::new(TaskRuntime::open(TaskRuntimeConfig::new(dir.path())).unwrap());
    let runner = Arc::new(BoundedOutputRunner {
        started: Notify::new(),
    });
    runtime.register_runner(runner.clone());
    let created = TaskCreateTool
        .call(
            serde_json::json!({
                "runner": {
                    "type": "tool",
                    "tool_name": "Fixture_Read",
                    "arguments": {"path": "fixture.txt"}
                }
            }),
            context(runtime.clone(), "agent-a", "chat-a"),
        )
        .await
        .unwrap();
    let created: serde_json::Value = serde_json::from_str(&created.text_content()).unwrap();
    let task_id = created["task_id"].as_str().unwrap();
    tokio::time::timeout(Duration::from_secs(2), runner.started.notified())
        .await
        .unwrap();

    let first = TaskReadTool
        .call(
            serde_json::json!({"task_id": task_id, "max_bytes": 5}),
            context(runtime.clone(), "agent-a", "chat-a"),
        )
        .await
        .unwrap();
    let first: serde_json::Value = serde_json::from_str(&first.text_content()).unwrap();
    assert_eq!(first["output"][0]["text"], "aaaa");
    assert_eq!(first["output"].as_array().unwrap().len(), 1);
    assert_eq!(first["has_more"], true);

    let second = TaskReadTool
        .call(
            serde_json::json!({
                "task_id": task_id,
                "cursor": first["cursor"],
                "max_bytes": 16
            }),
            context(runtime.clone(), "agent-a", "chat-a"),
        )
        .await
        .unwrap();
    let second: serde_json::Value = serde_json::from_str(&second.text_content()).unwrap();
    assert_eq!(second["output"][0]["text"], "bbbbcccc");
    assert!(second["cursor"].is_string());
    assert!(second.get("instruction").is_none());
    assert!(second.get("result").is_none());
    assert_eq!(second["has_more"], false);

    TaskCancelTool
        .call(
            serde_json::json!({"task_id": task_id}),
            context(runtime.clone(), "agent-a", "chat-a"),
        )
        .await
        .unwrap();
    wait_for_terminal(&runtime, &TaskOwner::new("agent-a", "chat-a"), task_id).await;
    let terminal = TaskReadTool
        .call(
            serde_json::json!({"task_id": task_id, "cursor": second["cursor"], "max_bytes": 8}),
            context(runtime.clone(), "agent-a", "chat-a"),
        )
        .await
        .unwrap();
    let text = terminal.text_content();
    assert!(!text.contains("full-output-must-not-be-inlined"));
    let terminal: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(terminal["has_more"], true);
    let mut cursor = terminal["cursor"].clone();
    let mut restored = terminal["output"][0]["text"].as_str().unwrap().to_string();
    let mut error_text = String::new();
    loop {
        let page = TaskReadTool
            .call(
                serde_json::json!({"task_id":task_id,"cursor":cursor,"max_bytes":8}),
                context(runtime.clone(), "agent-a", "chat-a"),
            )
            .await
            .unwrap();
        let page: serde_json::Value = serde_json::from_str(&page.text_content()).unwrap();
        if let Some(records) = page["output"].as_array() {
            for record in records {
                if record["source"] == "error" {
                    error_text.push_str(record["text"].as_str().unwrap());
                } else {
                    restored.push_str(record["text"].as_str().unwrap());
                }
            }
        }
        cursor = page["cursor"].clone();
        if page["has_more"] == false {
            break;
        }
    }
    assert_eq!(restored, "full-output-must-not-be-inlined");
    assert_eq!(error_text, "task cancelled");
    TaskRetryTool
        .call(
            serde_json::json!({"task_id":task_id}),
            context(runtime.clone(), "agent-a", "chat-a"),
        )
        .await
        .unwrap();
    assert!(TaskReadTool
        .call(
            serde_json::json!({"task_id":task_id,"cursor":cursor}),
            context(runtime.clone(), "agent-a", "chat-a")
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("Attempt"));
    TaskCancelTool
        .call(
            serde_json::json!({"task_id":task_id}),
            context(runtime.clone(), "agent-a", "chat-a"),
        )
        .await
        .unwrap();
}

async fn wait_for_terminal(runtime: &TaskRuntime, owner: &TaskOwner, task_id: &str) {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if runtime
                .get(owner, task_id)
                .await
                .is_ok_and(|record| record.state.is_terminal())
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

async fn wait_for_input_capability(
    runtime: &TaskRuntime,
    owner: &TaskOwner,
    task_id: &str,
    capability: rua_task::TaskInputCapability,
) {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if runtime
                .get(owner, task_id)
                .await
                .is_ok_and(|record| record.input_capability == capability)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn task_create_uses_typed_tool_runner_and_returns_immediately() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = Arc::new(TaskRuntime::open(TaskRuntimeConfig::new(dir.path())).unwrap());
    let execution = Arc::new(RecordingExecution {
        completed: Notify::new(),
    });
    runtime.register_runner(Arc::new(ToolTaskRunner::new(execution)));

    let result = TaskCreateTool
        .call(
            serde_json::json!({
                "instruction": "read fixture",
                "runner": {
                    "type": "tool",
                    "tool_name": "Fixture_Read",
                    "arguments": {"path": "fixture.txt"}
                }
            }),
            context(runtime.clone(), "agent-a", "chat-a"),
        )
        .await
        .unwrap();
    let created: serde_json::Value = serde_json::from_str(&result.text_content()).unwrap();
    let task_id = created["task_id"].as_str().unwrap();
    assert!(matches!(
        created["status"].as_str(),
        Some("accepted" | "waiting" | "running" | "completed")
    ));

    let owner = TaskOwner::new("agent-a", "chat-a");
    wait_for_terminal(&runtime, &owner, task_id).await;
    let record = runtime.get(&owner, task_id).await.unwrap();
    assert_eq!(record.state, TaskState::Completed);
    let output = &record.result.unwrap().output;
    assert!(output.contains("completed"));
    assert!(output.contains("fixture contents"));
    assert!(!output.contains("Remaining Work"));
}

#[tokio::test]
async fn task_create_rejects_disabled_tool_before_acceptance() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = Arc::new(TaskRuntime::open(TaskRuntimeConfig::new(dir.path())).unwrap());
    let mut ctx = context(runtime.clone(), "agent-a", "chat-a");
    ctx.metadata
        .insert("chat_tool_allowlist".into(), serde_json::json!(["read"]));
    let result = TaskCreateTool.call(serde_json::json!({"runner": {"type": "tool", "tool_name": "bash", "arguments": {"command": "echo denied"}}}), ctx).await;
    assert!(result.is_err());
    assert!(runtime
        .list(&TaskOwner::new("agent-a", "chat-a"), Default::default())
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn task_create_preserves_project_chat_ownership_from_tool_context() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = Arc::new(TaskRuntime::open(TaskRuntimeConfig::new(dir.path())).unwrap());
    let execution = Arc::new(BlockingCreateExecution {
        release: Notify::new(),
    });
    runtime.register_runner(Arc::new(ToolTaskRunner::new(execution.clone())));
    let address = rua_core::ChatAddress::project("project-a", "chat-a");
    let project_owner = TaskOwner::from_address(address.clone());
    let mut ctx = context(runtime.clone(), "agent-a", "chat-a");
    ctx.metadata.insert(
        "security_chat_address".to_string(),
        serde_json::to_value(address).unwrap(),
    );

    let created = TaskCreateTool
        .call(
            serde_json::json!({
                "runner": {
                    "type": "tool",
                    "tool_name": "Fixture_Blocking",
                    "arguments": {}
                }
            }),
            ctx,
        )
        .await
        .unwrap();
    let created: serde_json::Value = serde_json::from_str(&created.text_content()).unwrap();
    let task_id = created["task_id"].as_str().unwrap();

    assert!(runtime.get(&project_owner, task_id).await.is_ok());
    assert!(runtime
        .get(&TaskOwner::new("agent-a", "chat-a"), task_id)
        .await
        .is_err());

    execution.release.notify_one();
    wait_for_terminal(&runtime, &project_owner, task_id).await;
}

#[tokio::test]
async fn project_chat_agent_switch_preserves_all_task_controls_and_attempt_authorization() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = Arc::new(TaskRuntime::open(TaskRuntimeConfig::new(dir.path())).unwrap());
    let runner = Arc::new(ProjectControlRunner {
        inputs: Arc::new(Mutex::new(Vec::new())),
        started: Notify::new(),
    });
    runtime.register_runner(runner.clone());
    let owner = TaskOwner::from_address(rua_core::ChatAddress::project("project-a", "chat-a"));
    let task_id = runtime
        .create(
            rua_task::TaskSpec {
                owner: owner.clone(),
                instruction: "remain controllable after Agent switch".to_string(),
                runner: rua_task::TaskRunnerSpec::Tool {
                    tool_name: "Fixture_ProjectControl".to_string(),
                    arguments: serde_json::json!({}),
                },
                context_refs: Vec::new(),
                depends_on: Vec::new(),
                origin: Default::default(),
            },
            rua_task::TaskAttemptAuthorization::new("agent-a").unwrap(),
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), runner.started.notified())
        .await
        .unwrap();

    let get = TaskReadTool
        .call(
            serde_json::json!({"task_id": task_id}),
            project_context(
                runtime.clone(),
                "agent-b",
                "project-a",
                "chat-a",
                "get-after-switch",
            ),
        )
        .await
        .unwrap();
    let get: serde_json::Value = serde_json::from_str(&get.text_content()).unwrap();
    assert_eq!(get["status"], "running");
    assert!(get.get("attempts").is_none());
    assert!(get.get("owner").is_none());
    assert!(get.get("runner").is_none());

    let list = TaskListTool
        .call(
            serde_json::json!({}),
            project_context(
                runtime.clone(),
                "agent-b",
                "project-a",
                "chat-a",
                "list-after-switch",
            ),
        )
        .await
        .unwrap();
    let list: serde_json::Value = serde_json::from_str(&list.text_content()).unwrap();
    assert_eq!(list["tasks"].as_array().unwrap().len(), 1);
    assert_eq!(list["tasks"][0]["task_id"], task_id);
    assert_eq!(list["tasks"][0]["status"], "running");
    assert!(list["tasks"][0].get("attempts").is_none());
    assert!(list["tasks"][0].get("owner").is_none());

    let waiting = TaskReadTool
        .call(
            serde_json::json!({"task_id": task_id, "wait_seconds": 1}),
            project_context(
                runtime.clone(),
                "agent-b",
                "project-a",
                "chat-a",
                "read-wait-after-switch",
            ),
        )
        .await
        .unwrap();
    let waiting = serde_json::from_str::<serde_json::Value>(&waiting.text_content()).unwrap();
    assert_eq!(waiting["timed_out"], true);
    assert_eq!(waiting["status"], "running");
    assert!(waiting.get("attempts").is_none());
    TaskSendInputTool
        .call(
            serde_json::json!({"task_id": task_id, "text": "new direction"}),
            project_context(
                runtime.clone(),
                "agent-b",
                "project-a",
                "chat-a",
                "input-after-switch",
            ),
        )
        .await
        .unwrap();
    assert_eq!(
        runner.inputs.lock().await.as_slice(),
        &[TaskInput::Text {
            text: "new direction".to_string()
        }]
    );
    for (project_id, chat_id) in [("project-b", "chat-a"), ("project-a", "chat-b")] {
        assert!(TaskReadTool
            .call(
                serde_json::json!({"task_id": task_id}),
                project_context(
                    runtime.clone(),
                    "agent-b",
                    project_id,
                    chat_id,
                    "cross-chat-get",
                ),
            )
            .await
            .is_err());
    }

    let cancelled = TaskCancelTool
        .call(
            serde_json::json!({"task_id": task_id}),
            project_context(
                runtime.clone(),
                "agent-b",
                "project-a",
                "chat-a",
                "cancel-after-switch",
            ),
        )
        .await
        .unwrap();
    let cancelled: serde_json::Value = serde_json::from_str(&cancelled.text_content()).unwrap();
    assert_eq!(cancelled["status"], "cancelled");
    assert!(cancelled.get("attempts").is_none());

    let terminal = TaskReadTool
        .call(
            serde_json::json!({"task_id": task_id, "wait_seconds": 1}),
            project_context(
                runtime.clone(),
                "agent-b",
                "project-a",
                "chat-a",
                "read-terminal-after-switch",
            ),
        )
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&terminal.text_content())
            .unwrap()
            .get("timed_out"),
        None
    );

    let retried = TaskRetryTool
        .call(
            serde_json::json!({"task_id": task_id}),
            project_context(
                runtime.clone(),
                "agent-b",
                "project-a",
                "chat-a",
                "retry-after-switch",
            ),
        )
        .await
        .unwrap();
    let retried: serde_json::Value = serde_json::from_str(&retried.text_content()).unwrap();
    assert_eq!(retried["status"], "accepted");
    assert_eq!(retried["attempt_no"], 2);
    assert!(retried.get("attempts").is_none());
    tokio::time::timeout(Duration::from_secs(2), runner.started.notified())
        .await
        .unwrap();
    TaskCancelTool
        .call(
            serde_json::json!({"task_id": task_id}),
            project_context(
                runtime,
                "agent-b",
                "project-a",
                "chat-a",
                "cancel-retry-after-switch",
            ),
        )
        .await
        .unwrap();
}

struct BlockingCreateExecution {
    release: Notify,
}

#[async_trait]
impl ToolTaskExecution for BlockingCreateExecution {
    async fn execute(
        &self,
        _request: ToolTaskExecutionRequest,
    ) -> Result<agent_types::ToolResult, String> {
        self.release.notified().await;
        Ok(agent_types::ToolResult::text("blocking-create", "released"))
    }
}

#[tokio::test]
async fn task_create_returns_before_blocking_runner_completes() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = Arc::new(TaskRuntime::open(TaskRuntimeConfig::new(dir.path())).unwrap());
    let execution = Arc::new(BlockingCreateExecution {
        release: Notify::new(),
    });
    runtime.register_runner(Arc::new(ToolTaskRunner::new(execution.clone())));

    let created = tokio::time::timeout(
        Duration::from_secs(1),
        TaskCreateTool.call(
            serde_json::json!({
                "runner": {
                    "type": "tool",
                    "tool_name": "Fixture_Blocking",
                    "arguments": {}
                }
            }),
            context(runtime.clone(), "agent-a", "chat-a"),
        ),
    )
    .await
    .expect("task_create must not wait for Runner completion")
    .unwrap();
    let created: serde_json::Value = serde_json::from_str(&created.text_content()).unwrap();
    let task_id = created["task_id"].as_str().unwrap();
    let owner = TaskOwner::new("agent-a", "chat-a");
    assert!(!runtime
        .get(&owner, task_id)
        .await
        .unwrap()
        .state
        .is_terminal());

    execution.release.notify_one();
    wait_for_terminal(&runtime, &owner, task_id).await;
}

struct RealShellTaskExecution {
    working_dir: std::path::PathBuf,
}

#[cfg(unix)]
#[tokio::test]
async fn task_read_reopens_persisted_shell_observation_after_runtime_restart() {
    let dir = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let runtime = Arc::new(TaskRuntime::open(TaskRuntimeConfig::new(dir.path())).unwrap());
    runtime.register_runner(Arc::new(ToolTaskRunner::new(Arc::new(
        RealShellTaskExecution {
            working_dir: work.path().into(),
        },
    ))));
    let created = TaskCreateTool.call(serde_json::json!({"runner":{"type":"tool","tool_name":"bash","arguments":{"command":"printf 'abcdefghijklmno'"}}}), context(runtime.clone(), "agent-a", "chat-a")).await.unwrap();
    let created: serde_json::Value = serde_json::from_str(&created.text_content()).unwrap();
    let task_id = created["task_id"].as_str().unwrap();
    wait_for_terminal(&runtime, &TaskOwner::new("agent-a", "chat-a"), task_id).await;
    let first = TaskReadTool
        .call(
            serde_json::json!({"task_id":task_id,"max_bytes":5}),
            context(runtime.clone(), "agent-a", "chat-a"),
        )
        .await
        .unwrap();
    let first: serde_json::Value = serde_json::from_str(&first.text_content()).unwrap();
    assert_eq!(first["output"][0]["text"], "abcde");
    let owner = TaskOwner::new("agent-a", "chat-a");
    let raw: serde_json::Value =
        serde_json::from_slice(&runtime.read_full_result(&owner, task_id).await.unwrap()).unwrap();
    let observation =
        std::path::PathBuf::from(raw["metadata"]["live_output_path"].as_str().unwrap());
    let attempt_dir = TaskRuntimeConfig::new(dir.path()).attempt_dir(&owner, task_id, 1);
    assert!(observation.starts_with(&attempt_dir));
    let unrelated = work.path().join("unrelated.txt");
    tokio::fs::write(&unrelated, b"private fixture")
        .await
        .unwrap();
    assert!(runtime
        .resolve_output_file(&owner, task_id, 1, &unrelated)
        .await
        .unwrap_err()
        .to_string()
        .contains("escapes"));
    #[cfg(unix)]
    {
        let link = attempt_dir.join("output/escape");
        std::os::unix::fs::symlink(&unrelated, &link).unwrap();
        assert!(runtime
            .resolve_output_file(&owner, task_id, 1, &link)
            .await
            .is_err());
    }
    rua_tool::ToolResultStore::cleanup(
        &rua_tool::ToolCallArchive::new(work.path().join("tool_call_logs")),
        rua_tool::result_store::CleanupPolicy {
            max_age: Duration::ZERO,
            max_total_bytes: 0,
        },
    )
    .await
    .unwrap();
    assert!(observation.exists());
    drop(runtime);
    let reopened = Arc::new(TaskRuntime::open(TaskRuntimeConfig::new(dir.path())).unwrap());
    let mut cursor = first["cursor"].clone();
    let mut text = String::from("abcde");
    for _ in 0..100 {
        let page = TaskReadTool
            .call(
                serde_json::json!({"task_id":task_id,"cursor":cursor,"max_bytes":5}),
                context(reopened.clone(), "agent-a", "chat-a"),
            )
            .await
            .unwrap();
        let page: serde_json::Value = serde_json::from_str(&page.text_content()).unwrap();
        for record in page["output"].as_array().into_iter().flatten() {
            if record["source"] == "stdout" {
                text.push_str(record["text"].as_str().unwrap());
            }
            assert_ne!(
                record["source"], "result",
                "shell output must not be repeated in terminal text"
            );
        }
        cursor = page["cursor"].clone();
        if page["has_more"] == false {
            break;
        }
    }
    assert_eq!(text, "abcdefghijklmno");
    reopened.remove_owner(&owner).await.unwrap();
    assert!(
        !observation.exists(),
        "Task removal must clean its owned observation"
    );
}

#[async_trait]
impl ToolTaskExecution for RealShellTaskExecution {
    async fn execute(
        &self,
        request: ToolTaskExecutionRequest,
    ) -> Result<agent_types::ToolResult, String> {
        let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
        let cancel = request.cancel.clone();
        let cancel_bridge = tokio::spawn(async move {
            cancel.cancelled().await;
            let _ = cancel_tx.send(true);
        });
        let tool = ShellTool::new(Arc::new(RuntimeCommandExecutor::new()));
        let tool_result_store_dir = self.working_dir.join("tool-results");
        std::fs::create_dir_all(&tool_result_store_dir).map_err(|error| error.to_string())?;
        let result = tool
            .call(
                request.arguments,
                ToolContext {
                    agent_id: request.authorization_agent_id,
                    session_id: request.owner.chat_id().to_string(),
                    working_dir: self.working_dir.clone(),
                    tool_result_store_dir: Some(tool_result_store_dir),
                    cancel: Some(cancel_rx),
                    task_session: Some(Arc::new(TaskRunnerSessionHandle::new(request.session))),
                    ..Default::default()
                },
            )
            .await
            .map_err(|error| error.to_string());
        cancel_bridge.abort();
        result
    }
}

struct DelayedShellResultExecution {
    working_dir: std::path::PathBuf,
    shell_finished: Notify,
    release_result: Notify,
}

#[async_trait]
impl ToolTaskExecution for DelayedShellResultExecution {
    async fn execute(
        &self,
        request: ToolTaskExecutionRequest,
    ) -> Result<agent_types::ToolResult, String> {
        let (_cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
        let tool = ShellTool::new(Arc::new(RuntimeCommandExecutor::new()));
        let tool_result_store_dir = self.working_dir.join("tool-results");
        std::fs::create_dir_all(&tool_result_store_dir).map_err(|error| error.to_string())?;
        let result = tool
            .call(
                request.arguments,
                ToolContext {
                    agent_id: request.authorization_agent_id,
                    session_id: request.owner.chat_id().to_string(),
                    working_dir: self.working_dir.clone(),
                    tool_result_store_dir: Some(tool_result_store_dir),
                    cancel: Some(cancel_rx),
                    task_session: Some(Arc::new(TaskRunnerSessionHandle::new(request.session))),
                    ..Default::default()
                },
            )
            .await
            .map_err(|error| error.to_string())?;
        self.shell_finished.notify_one();
        self.release_result.notified().await;
        Ok(result)
    }
}

#[cfg(unix)]
#[tokio::test]
async fn shell_task_multiline_scripts_reach_the_correct_terminal_state() {
    let dir = tempfile::tempdir().unwrap();
    let working_dir = tempfile::tempdir().unwrap();
    let runtime = Arc::new(TaskRuntime::open(TaskRuntimeConfig::new(dir.path())).unwrap());
    runtime.register_runner(Arc::new(ToolTaskRunner::new(Arc::new(
        RealShellTaskExecution {
            working_dir: working_dir.path().to_path_buf(),
        },
    ))));
    let owner = TaskOwner::new("agent-a", "chat-a");
    for (command, expected_state, expected_output) in [
        (
            "PATCH() { read -r line; }\ncat <<'PATCH'\npatch-body\nPATCH",
            TaskState::Completed,
            "patch-body",
        ),
        (
            "cat <<'RUA_EOF'\n中文 heredoc\nRUA_EOF\nprintf 'done'",
            TaskState::Completed,
            "中文 heredoc",
        ),
        (
            "# comment must not swallow the command\nprintf 'failure-marker' >&2\nexit 7",
            TaskState::Failed,
            "failure-marker",
        ),
        (
            "cat <<'RUA_EOF'\nerror input\nRUA_EOF\nprintf 'heredoc-failed' >&2\nexit 9",
            TaskState::Failed,
            "heredoc-failed",
        ),
        (
            "printf 'warning-only' >&2\nprintf 'success-marker'",
            TaskState::Completed,
            "success-marker",
        ),
    ] {
        let created = TaskCreateTool.call(
            serde_json::json!({"runner": {"type": "tool", "tool_name": "bash", "arguments": {"command": command}}}),
            context(runtime.clone(), "agent-a", "chat-a"),
        ).await.unwrap();
        let created: serde_json::Value = serde_json::from_str(&created.text_content()).unwrap();
        let task_id = created["task_id"].as_str().unwrap();
        let terminal = runtime
            .await_terminal(&owner, task_id, Duration::from_secs(5))
            .await
            .unwrap();
        if !terminal.state.is_terminal() {
            runtime.cancel(&owner, task_id).await.unwrap();
        }
        assert_eq!(terminal.state, expected_state, "command: {command}");
        assert_eq!(terminal.input_capability, TaskInputCapability::None);
        assert!(
            terminal.result.unwrap().output.contains(expected_output),
            "command: {command}"
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn task_send_input_drives_direct_shell_task_once_and_delivers_eof() {
    let dir = tempfile::tempdir().unwrap();
    let working_dir = tempfile::tempdir().unwrap();
    let runtime = Arc::new(TaskRuntime::open(TaskRuntimeConfig::new(dir.path())).unwrap());
    runtime.register_runner(Arc::new(ToolTaskRunner::new(Arc::new(
        RealShellTaskExecution {
            working_dir: working_dir.path().to_path_buf(),
        },
    ))));
    let owner = TaskOwner::new("agent-a", "chat-a");
    let created = TaskCreateTool
        .call(
            serde_json::json!({
                "runner": {
                    "type": "tool",
                    "tool_name": "bash",
                    "arguments": {"command": "cat"}
                }
            }),
            context(runtime.clone(), "agent-a", "chat-a"),
        )
        .await
        .unwrap();
    let created: serde_json::Value = serde_json::from_str(&created.text_content()).unwrap();
    let task_id = created["task_id"].as_str().unwrap().to_string();
    wait_for_input_capability(
        &runtime,
        &owner,
        &task_id,
        rua_task::TaskInputCapability::Stdin,
    )
    .await;

    let mut text_context = context(runtime.clone(), "agent-a", "chat-a");
    text_context.metadata.insert(
        "tool_use_id".to_string(),
        serde_json::json!("send-direct-text"),
    );
    let first = TaskSendInputTool
        .call(
            serde_json::json!({"task_id": task_id, "text": "direct-input"}),
            text_context.clone(),
        )
        .await
        .unwrap();
    let duplicate = TaskSendInputTool
        .call(
            serde_json::json!({"task_id": task_id, "text": "direct-input"}),
            text_context,
        )
        .await
        .unwrap();
    assert_eq!(duplicate.text_content(), first.text_content());

    let mut close_context = context(runtime.clone(), "agent-a", "chat-a");
    close_context.metadata.insert(
        "tool_use_id".to_string(),
        serde_json::json!("send-direct-close"),
    );
    TaskSendInputTool
        .call(
            serde_json::json!({"task_id": task_id, "close_stdin": true}),
            close_context,
        )
        .await
        .unwrap();

    let terminal = runtime
        .await_terminal(&owner, &task_id, Duration::from_secs(2))
        .await
        .unwrap();
    assert_eq!(terminal.state, TaskState::Completed);
    assert_eq!(
        terminal.input_capability,
        rua_task::TaskInputCapability::None
    );
    assert!(terminal.result.unwrap().output.contains("direct-input"));
}

#[cfg(unix)]
#[tokio::test]
async fn direct_shell_task_has_no_foreground_wait_timer() {
    let dir = tempfile::tempdir().unwrap();
    let working_dir = tempfile::tempdir().unwrap();
    let runtime = Arc::new(TaskRuntime::open(TaskRuntimeConfig::new(dir.path())).unwrap());
    runtime.register_runner(Arc::new(ToolTaskRunner::new(Arc::new(
        RealShellTaskExecution {
            working_dir: working_dir.path().to_path_buf(),
        },
    ))));
    let owner = TaskOwner::new("agent-a", "chat-a");
    let created = TaskCreateTool
        .call(
            serde_json::json!({
                "runner": {
                    "type": "tool",
                    "tool_name": "bash",
                    "arguments": {
                        "command": "sleep 0.1; printf direct-finished",
                        "yield_seconds": 1
                    }
                }
            }),
            context(runtime.clone(), "agent-a", "chat-a"),
        )
        .await
        .unwrap();
    let created: serde_json::Value = serde_json::from_str(&created.text_content()).unwrap();
    let task_id = created["task_id"].as_str().unwrap();

    let terminal = runtime
        .await_terminal(&owner, task_id, Duration::from_secs(2))
        .await
        .unwrap();
    assert_eq!(terminal.state, TaskState::Completed);
    assert!(terminal.result.unwrap().output.contains("direct-finished"));
}

#[cfg(unix)]
#[tokio::test]
async fn task_send_input_rejects_closed_shell_while_task_record_is_still_running() {
    let dir = tempfile::tempdir().unwrap();
    let working_dir = tempfile::tempdir().unwrap();
    let runtime = Arc::new(TaskRuntime::open(TaskRuntimeConfig::new(dir.path())).unwrap());
    let execution = Arc::new(DelayedShellResultExecution {
        working_dir: working_dir.path().to_path_buf(),
        shell_finished: Notify::new(),
        release_result: Notify::new(),
    });
    runtime.register_runner(Arc::new(ToolTaskRunner::new(execution.clone())));
    let owner = TaskOwner::new("agent-a", "chat-a");
    let created = TaskCreateTool
        .call(
            serde_json::json!({
                "runner": {
                    "type": "tool",
                    "tool_name": "bash",
                    "arguments": {"command": "printf finished"}
                }
            }),
            context(runtime.clone(), "agent-a", "chat-a"),
        )
        .await
        .unwrap();
    let created: serde_json::Value = serde_json::from_str(&created.text_content()).unwrap();
    let task_id = created["task_id"].as_str().unwrap().to_string();
    tokio::time::timeout(Duration::from_secs(2), execution.shell_finished.notified())
        .await
        .unwrap();

    let running = runtime.get(&owner, &task_id).await.unwrap();
    assert_eq!(running.state, TaskState::Running);
    assert_eq!(
        running.input_capability,
        rua_task::TaskInputCapability::None
    );
    let mut input_context = context(runtime.clone(), "agent-a", "chat-a");
    input_context.metadata.insert(
        "tool_use_id".to_string(),
        serde_json::json!("closed-shell-input"),
    );
    let error = TaskSendInputTool
        .call(
            serde_json::json!({"task_id": task_id, "text": "too-late"}),
            input_context,
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("input is unavailable"));

    execution.release_result.notify_one();
    let terminal = runtime
        .await_terminal(&owner, &task_id, Duration::from_secs(2))
        .await
        .unwrap();
    assert_eq!(terminal.state, TaskState::Completed);
}

#[cfg(unix)]
#[tokio::test]
async fn task_watch_reads_future_shell_lines_and_final_unterminated_suffix() {
    let dir = tempfile::tempdir().unwrap();
    let working_dir = tempfile::tempdir().unwrap();
    let sink = Arc::new(RecordingWatchSink::default());
    let runtime = Arc::new(
        TaskRuntime::open_with_sink(TaskRuntimeConfig::new(dir.path()), sink.clone()).unwrap(),
    );
    runtime.register_runner(Arc::new(ToolTaskRunner::new(Arc::new(
        RealShellTaskExecution {
            working_dir: working_dir.path().to_path_buf(),
        },
    ))));
    let owner = TaskOwner::new("agent-a", "chat-a");
    let created = TaskCreateTool
        .call(
            serde_json::json!({
                "runner": {
                    "type": "tool",
                    "tool_name": "bash",
                    "arguments": {
                        "command": "printf 'historical\\n'; : > watch-ready; while [ ! -f watch-release ]; do sleep 0.01; done; printf 'future-out\\n'; printf 'future-err\\n' >&2; printf final-tail"
                    }
                }
            }),
            context(runtime.clone(), "agent-a", "chat-a"),
        )
        .await
        .unwrap();
    let created: serde_json::Value = serde_json::from_str(&created.text_content()).unwrap();
    let task_id = created["task_id"].as_str().unwrap().to_string();
    tokio::time::timeout(Duration::from_secs(2), async {
        while !working_dir.path().join("watch-ready").is_file() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        loop {
            let observed = TaskReadTool
                .call(
                    serde_json::json!({"task_id":task_id}),
                    context(runtime.clone(), "agent-a", "chat-a"),
                )
                .await
                .unwrap();
            let observed: serde_json::Value =
                serde_json::from_str(&observed.text_content()).unwrap();
            if observed["output"].as_array().is_some_and(|records| {
                records.iter().any(|record| {
                    record["text"]
                        .as_str()
                        .is_some_and(|text| text.contains("historical\n"))
                })
            }) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();

    let watched = TaskWatchTool
        .call(
            serde_json::json!({"task_id": task_id}),
            context(runtime.clone(), "agent-a", "chat-a"),
        )
        .await
        .unwrap();
    let duplicate = TaskWatchTool
        .call(
            serde_json::json!({"task_id": task_id}),
            context(runtime.clone(), "agent-a", "chat-a"),
        )
        .await
        .unwrap();
    assert_eq!(duplicate.text_content(), watched.text_content());
    std::fs::write(working_dir.path().join("watch-release"), "").unwrap();

    let terminal = runtime
        .await_terminal(&owner, &task_id, Duration::from_secs(3))
        .await
        .unwrap();
    assert_eq!(terminal.state, TaskState::Completed);
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let records = sink
                .notifications
                .lock()
                .await
                .iter()
                .flat_map(|notification| notification.records.iter())
                .cloned()
                .collect::<Vec<_>>();
            if records
                .iter()
                .filter(|record| record.source == "stdout")
                .map(|record| record.text.as_str())
                .collect::<String>()
                .ends_with("final-tail")
            {
                break records;
            }
            sink.changed.notified().await;
        }
    })
    .await
    .unwrap();
    let records = sink
        .notifications
        .lock()
        .await
        .iter()
        .flat_map(|notification| notification.records.iter())
        .cloned()
        .collect::<Vec<_>>();
    let stdout = records
        .iter()
        .filter(|r| r.source == "stdout")
        .map(|r| r.text.as_str())
        .collect::<String>();
    let stderr = records
        .iter()
        .filter(|r| r.source == "stderr")
        .map(|r| r.text.as_str())
        .collect::<String>();
    assert_eq!(stdout, "future-out\nfinal-tail");
    assert_eq!(stderr, "future-err\n");

    let unwatched = TaskUnwatchTool
        .call(
            serde_json::json!({"task_id": task_id}),
            context(runtime, "agent-a", "chat-a"),
        )
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&unwatched.text_content()).unwrap()["active"],
        false
    );
}

#[cfg(unix)]
#[tokio::test]
async fn task_watch_tail_f_delivers_appended_lines_without_task_polling() {
    let dir = tempfile::tempdir().unwrap();
    let working_dir = tempfile::tempdir().unwrap();
    std::fs::write(working_dir.path().join("watched.log"), "tail-opened\n").unwrap();
    let sink = Arc::new(RecordingWatchSink::default());
    let runtime = Arc::new(
        TaskRuntime::open_with_sink(TaskRuntimeConfig::new(dir.path()), sink.clone()).unwrap(),
    );
    runtime.register_runner(Arc::new(ToolTaskRunner::new(Arc::new(
        RealShellTaskExecution {
            working_dir: working_dir.path().to_path_buf(),
        },
    ))));
    let owner = TaskOwner::new("agent-a", "chat-a");
    let created = TaskCreateTool
        .call(
            serde_json::json!({
                "runner": {
                    "type": "tool",
                    "tool_name": "bash",
                    "arguments": {
                        "command": ": > tail-ready; while [ ! -f tail-start ]; do sleep 0.01; done; exec tail -n 1 -f watched.log"
                    }
                }
            }),
            context(runtime.clone(), "agent-a", "chat-a"),
        )
        .await
        .unwrap();
    let created: serde_json::Value = serde_json::from_str(&created.text_content()).unwrap();
    let task_id = created["task_id"].as_str().unwrap().to_string();
    wait_for_input_capability(
        &runtime,
        &owner,
        &task_id,
        rua_task::TaskInputCapability::Stdin,
    )
    .await;
    TaskWatchTool
        .call(
            serde_json::json!({"task_id": task_id}),
            context(runtime.clone(), "agent-a", "chat-a"),
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while !working_dir.path().join("tail-ready").is_file() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    std::fs::write(working_dir.path().join("tail-start"), "").unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if sink
                .notifications
                .lock()
                .await
                .iter()
                .flat_map(|notification| notification.records.iter())
                .any(|record| record.text == "tail-opened\n")
            {
                break;
            }
            sink.changed.notified().await;
        }
    })
    .await
    .unwrap();

    use std::io::Write;
    {
        let mut log = std::fs::OpenOptions::new()
            .append(true)
            .open(working_dir.path().join("watched.log"))
            .unwrap();
        writeln!(log, "line-one").unwrap();
        log.sync_all().unwrap();
    }
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if sink
                .notifications
                .lock()
                .await
                .iter()
                .flat_map(|notification| notification.records.iter())
                .any(|record| record.text == "line-one\n")
            {
                break;
            }
            sink.changed.notified().await;
        }
    })
    .await
    .unwrap();
    {
        let mut log = std::fs::OpenOptions::new()
            .append(true)
            .open(working_dir.path().join("watched.log"))
            .unwrap();
        writeln!(log, "line-two").unwrap();
        log.sync_all().unwrap();
    }
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if sink
                .notifications
                .lock()
                .await
                .iter()
                .flat_map(|notification| notification.records.iter())
                .any(|record| record.text == "line-two\n")
            {
                break;
            }
            sink.changed.notified().await;
        }
    })
    .await
    .unwrap();

    let cancelled = runtime.cancel(&owner, &task_id).await.unwrap();
    assert_eq!(cancelled.state, TaskState::Cancelled);
    let records = sink
        .notifications
        .lock()
        .await
        .iter()
        .flat_map(|notification| notification.records.iter())
        .map(|record| record.text.clone())
        .collect::<Vec<_>>();
    assert!(!records.contains(&"before-watch".to_string()));
    assert!(records.contains(&"line-one\n".to_string()));
    assert!(records.contains(&"line-two\n".to_string()));
}

#[cfg(unix)]
#[tokio::test]
async fn task_watch_bounds_one_unterminated_shell_record_to_batch_limit() {
    let dir = tempfile::tempdir().unwrap();
    let working_dir = tempfile::tempdir().unwrap();
    let sink = Arc::new(RecordingWatchSink::default());
    let runtime = Arc::new(
        TaskRuntime::open_with_sink(TaskRuntimeConfig::new(dir.path()), sink.clone()).unwrap(),
    );
    runtime.register_runner(Arc::new(ToolTaskRunner::new(Arc::new(
        RealShellTaskExecution {
            working_dir: working_dir.path().to_path_buf(),
        },
    ))));
    let owner = TaskOwner::new("agent-a", "chat-a");
    let created = TaskCreateTool
        .call(
            serde_json::json!({
                "runner": {
                    "type": "tool",
                    "tool_name": "bash",
                    "arguments": {
                        "command": ": > watch-ready; while [ ! -f watch-release ]; do sleep 0.01; done; head -c 200000 /dev/zero | tr '\\0' x"
                    }
                }
            }),
            context(runtime.clone(), "agent-a", "chat-a"),
        )
        .await
        .unwrap();
    let created: serde_json::Value = serde_json::from_str(&created.text_content()).unwrap();
    let task_id = created["task_id"].as_str().unwrap().to_string();
    tokio::time::timeout(Duration::from_secs(2), async {
        while !working_dir.path().join("watch-ready").is_file() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();

    TaskWatchTool
        .call(
            serde_json::json!({"task_id": task_id}),
            context(runtime.clone(), "agent-a", "chat-a"),
        )
        .await
        .unwrap();
    std::fs::write(working_dir.path().join("watch-release"), "").unwrap();
    let terminal = runtime
        .await_terminal(&owner, &task_id, Duration::from_secs(3))
        .await
        .unwrap();
    assert_eq!(terminal.state, TaskState::Completed);

    let record = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Some(record) = sink
                .notifications
                .lock()
                .await
                .iter()
                .flat_map(|notification| notification.records.iter())
                .find(|record| record.source == "stdout")
                .cloned()
            {
                break record;
            }
            sink.changed.notified().await;
        }
    })
    .await
    .unwrap();
    assert!(record.text.len() <= 64 * 1024);
    assert!(record.text.chars().all(|c| c == 'x'));
    let notifications = sink.notifications.lock().await;
    let records = notifications
        .iter()
        .flat_map(|n| &n.records)
        .collect::<Vec<_>>();
    assert!(records.iter().any(|r| r.text.contains("bytes omitted")));
    assert!(records.iter().map(|r| r.text.len()).sum::<usize>() <= 64 * 1024);
    drop(notifications);
    let mut cursor = TaskLiveOutputCursor::default();
    let mut total = 0;
    loop {
        let page = runtime
            .read(&owner, &task_id, cursor, 128, 8192, Duration::ZERO)
            .await
            .unwrap();
        total += page.records.iter().map(|r| r.text.len()).sum::<usize>();
        cursor = page.next_cursor;
        if !page.has_more {
            break;
        }
    }
    assert_eq!(total, 200000);
}

struct AttachmentExecution;

#[test]
fn task_read_schema_uses_one_optional_cursor() {
    let schema = TaskReadTool::spec().parameters;
    assert_eq!(schema["required"], serde_json::json!(["task_id"]));
    assert!(schema["properties"].get("artifact").is_none());
    assert_eq!(
        schema["properties"]["cursor"]["type"],
        serde_json::json!(["string", "null"])
    );
    assert_eq!(schema["properties"].as_object().unwrap().len(), 4);
}

#[async_trait]
impl ToolTaskExecution for AttachmentExecution {
    async fn execute(
        &self,
        request: ToolTaskExecutionRequest,
    ) -> Result<agent_types::ToolResult, String> {
        let mut result = agent_types::ToolResult::text(request.task_id, "attachment ready");
        result.name = Some(request.tool_name);
        result.attachments = Some(vec![agent_types::ToolAttachment {
            name: "../report.txt".to_string(),
            mime_type: "text/plain".to_string(),
            content: "artifact-body-".repeat(128),
        }]);
        Ok(result)
    }
}

#[tokio::test]
async fn tool_attachments_are_externalized_when_raw_result_is_bounded() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = TaskRuntimeConfig::new(dir.path());
    config.limits.max_raw_result_bytes = 128;
    let runtime = Arc::new(TaskRuntime::open(config).unwrap());
    runtime.register_runner(Arc::new(ToolTaskRunner::new(Arc::new(AttachmentExecution))));
    let owner = TaskOwner::new("agent-a", "chat-a");

    let created = TaskCreateTool
        .call(
            serde_json::json!({
                "runner": {
                    "type": "tool",
                    "tool_name": "Fixture_Attachment",
                    "arguments": {}
                }
            }),
            context(runtime.clone(), "agent-a", "chat-a"),
        )
        .await
        .unwrap();
    let created: serde_json::Value = serde_json::from_str(&created.text_content()).unwrap();
    let task_id = created["task_id"].as_str().unwrap();
    wait_for_terminal(&runtime, &owner, task_id).await;

    let record = runtime.get(&owner, task_id).await.unwrap();
    let result = record.result.expect("Task Result");
    assert_eq!(result.artifacts.len(), 1);
    let artifact = &result.artifacts[0];
    assert_eq!(artifact.resource.ref_type, "task_artifact");
    assert_eq!(artifact.metadata["content_externalized"], true);
    assert!(!artifact.resource.uri.contains(".."));
    let file_name = artifact
        .resource
        .uri
        .rsplit('/')
        .next()
        .expect("artifact file name");
    let artifact_path = dir
        .path()
        .join("workspaces/agent-a/sessions/chat-a/tasks")
        .join(task_id)
        .join("attempts/1/artifacts")
        .join(file_name);
    assert_eq!(
        std::fs::read_to_string(artifact_path).unwrap(),
        "artifact-body-".repeat(128)
    );

    let raw = runtime.read_full_result(&owner, task_id).await.unwrap();
    let raw: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    assert_eq!(raw["bounded"], true);
    use base64::Engine;
    let initial = TaskReadTool
        .call(
            serde_json::json!({"task_id":task_id,"cursor":null}),
            context(runtime.clone(), "agent-a", "chat-a"),
        )
        .await
        .unwrap();
    let initial: serde_json::Value = serde_json::from_str(&initial.text_content()).unwrap();
    let mut cursor = initial["artifacts"][0]["cursor"].clone();
    assert!(cursor.is_string());
    assert!(TaskReadTool
        .call(
            serde_json::json!({"task_id":"another-task","cursor":cursor}),
            context(runtime.clone(), "agent-a", "chat-a")
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("another Task"));
    let mut restored = Vec::new();
    loop {
        let page = TaskReadTool
            .call(
                serde_json::json!({"task_id":task_id,"cursor":cursor,"wait_seconds":30,"max_bytes":128}),
                context(runtime.clone(), "agent-a", "chat-a"),
            )
            .await
            .unwrap();
        let page: serde_json::Value = serde_json::from_str(&page.text_content()).unwrap();
        let encoded = page["artifact"]["data"].as_str().unwrap();
        assert!(encoded.len() <= 128);
        restored.extend(
            base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .unwrap(),
        );
        if page["has_more"] == false {
            break;
        }
        assert!(page.get("next_artifact").is_none());
        cursor = page["cursor"].clone();
    }
    assert_eq!(restored, "artifact-body-".repeat(128).as_bytes());
    assert!(TaskReadTool
        .call(
            serde_json::json!({"task_id":task_id,"cursor":cursor}),
            context(runtime.clone(), "agent-a", "other-chat")
        )
        .await
        .is_err());
    assert!(TaskReadTool
        .call(
            serde_json::json!({"task_id":task_id,"cursor":"invalid"}),
            context(runtime.clone(), "agent-a", "chat-a")
        )
        .await
        .is_err());
    assert!(runtime
        .read_artifact(&owner, task_id, record.attempt_no + 1, &artifact.id, 0, 128)
        .await
        .is_err());
    assert!(runtime
        .read_artifact(&owner, task_id, record.attempt_no, "../outside", 0, 128)
        .await
        .is_err());
    assert!(runtime
        .read_artifact(
            &owner,
            task_id,
            record.attempt_no,
            &artifact.id,
            u64::MAX,
            128
        )
        .await
        .is_err());
}

#[tokio::test]
async fn task_get_does_not_disclose_cross_chat_records() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = Arc::new(TaskRuntime::open(TaskRuntimeConfig::new(dir.path())).unwrap());
    runtime.register_runner(Arc::new(ToolTaskRunner::new(Arc::new(
        RecordingExecution {
            completed: Notify::new(),
        },
    ))));

    let created = TaskCreateTool
        .call(
            serde_json::json!({
                "runner": {
                    "type": "tool",
                    "tool_name": "Fixture_Read",
                    "arguments": {"path": "fixture.txt"}
                }
            }),
            context(runtime.clone(), "agent-a", "chat-a"),
        )
        .await
        .unwrap();
    let value: serde_json::Value = serde_json::from_str(&created.text_content()).unwrap();
    let task_id = value["task_id"].as_str().unwrap();

    let error = TaskReadTool
        .call(
            serde_json::json!({"task_id": task_id}),
            context(runtime.clone(), "agent-a", "chat-b"),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("not found"));
    assert!(runtime
        .list(
            &TaskOwner::new("agent-a", "chat-b"),
            TaskListFilter::default()
        )
        .await
        .unwrap()
        .is_empty());

    let own_list = TaskListTool
        .call(
            serde_json::json!({}),
            context(runtime.clone(), "agent-a", "chat-a"),
        )
        .await
        .unwrap();
    let own_list: serde_json::Value = serde_json::from_str(&own_list.text_content()).unwrap();
    assert_eq!(own_list["tasks"].as_array().unwrap().len(), 1);

    let cross_chat_list = TaskListTool
        .call(
            serde_json::json!({}),
            context(runtime.clone(), "agent-a", "chat-b"),
        )
        .await
        .unwrap();
    let cross_chat_list: serde_json::Value =
        serde_json::from_str(&cross_chat_list.text_content()).unwrap();
    assert!(cross_chat_list["tasks"].as_array().unwrap().is_empty());

    let invalid_state = TaskListTool
        .call(
            serde_json::json!({"state": "pending"}),
            context(runtime, "agent-a", "chat-a"),
        )
        .await
        .unwrap_err();
    assert!(invalid_state
        .to_string()
        .contains("unknown Task state 'pending'"));
}

#[test]
fn task_runner_contract_is_object_safe() {
    fn assert_runner(_: &dyn TaskRunner) {}
    fn assert_sink(_: &dyn TaskNotificationSink) {}

    let runner = ToolTaskRunner::new(Arc::new(RecordingExecution {
        completed: Notify::new(),
    }));
    assert_runner(&runner);
    assert_sink(&rua_task::NoopTaskNotificationSink);
}

#[tokio::test]
async fn task_await_accepts_short_exact_timeout_and_returns_terminal_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = Arc::new(TaskRuntime::open(TaskRuntimeConfig::new(dir.path())).unwrap());
    runtime.register_runner(Arc::new(ToolTaskRunner::new(Arc::new(
        RecordingExecution {
            completed: Notify::new(),
        },
    ))));
    let ctx = context(runtime.clone(), "agent-a", "chat-a");
    let created = TaskCreateTool
        .call(
            serde_json::json!({
                "runner": {
                    "type": "tool",
                    "tool_name": "Fixture_Read",
                    "arguments": {"path": "fixture.txt"}
                }
            }),
            ctx.clone(),
        )
        .await
        .unwrap();
    let value: serde_json::Value = serde_json::from_str(&created.text_content()).unwrap();
    let task_id = value["task_id"].as_str().unwrap();

    let awaited = TaskAwaitTool
        .call(
            serde_json::json!({"task_ids": [task_id], "timeout": 1}),
            ctx,
        )
        .await
        .unwrap();
    let value: serde_json::Value = serde_json::from_str(&awaited.text_content()).unwrap();
    assert_eq!(value["timed_out"], false);
    assert_eq!(value["tasks"][0]["status"], "completed");
}

#[test]
fn task_create_schema_has_no_total_timeout() {
    let spec = TaskCreateTool::spec();
    let parameters = spec.parameters;
    assert!(parameters["properties"].get("timeout_secs").is_none());
    assert!(spec.description.contains("background Tool"));
    assert!(spec.description.contains("task_read"));
    let variants = parameters["properties"]["runner"]["oneOf"]
        .as_array()
        .unwrap();
    assert_eq!(variants.len(), 1);
    assert_eq!(variants[0]["properties"]["type"]["enum"][0], "tool");
}

#[tokio::test]
async fn task_await_zero_uses_default_wait_instead_of_immediate_timeout() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = Arc::new(TaskRuntime::open(TaskRuntimeConfig::new(dir.path())).unwrap());
    let execution = Arc::new(BlockingCreateExecution {
        release: Notify::new(),
    });
    runtime.register_runner(Arc::new(ToolTaskRunner::new(execution.clone())));
    let ctx = context(runtime.clone(), "agent-a", "chat-a");
    let created = TaskCreateTool
        .call(
            serde_json::json!({
                "runner": {
                    "type": "tool",
                    "tool_name": "Fixture_Blocking",
                    "arguments": {}
                }
            }),
            ctx.clone(),
        )
        .await
        .unwrap();
    let created: serde_json::Value = serde_json::from_str(&created.text_content()).unwrap();
    let task_id = created["task_id"].as_str().unwrap().to_string();

    let mut waiter = tokio::spawn(async move {
        TaskAwaitTool
            .call(
                serde_json::json!({"task_ids": [task_id], "timeout": 0}),
                ctx,
            )
            .await
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut waiter)
            .await
            .is_err(),
        "zero should use the default wait instead of returning immediately"
    );

    execution.release.notify_one();
    let awaited = tokio::time::timeout(Duration::from_secs(2), waiter)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let value: serde_json::Value = serde_json::from_str(&awaited.text_content()).unwrap();
    assert_eq!(value["timed_out"], false);
    assert_eq!(value["tasks"][0]["status"], "completed");
}

#[tokio::test]
async fn dropping_task_await_detaches_without_cancelling_execution() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = Arc::new(TaskRuntime::open(TaskRuntimeConfig::new(dir.path())).unwrap());
    let execution = Arc::new(BlockingCreateExecution {
        release: Notify::new(),
    });
    runtime.register_runner(Arc::new(ToolTaskRunner::new(execution.clone())));
    let ctx = context(runtime.clone(), "agent-a", "chat-a");
    let created = TaskCreateTool
        .call(
            serde_json::json!({
                "runner": {
                    "type": "tool",
                    "tool_name": "Fixture_Blocking",
                    "arguments": {}
                }
            }),
            ctx.clone(),
        )
        .await
        .unwrap();
    let created: serde_json::Value = serde_json::from_str(&created.text_content()).unwrap();
    let task_id = created["task_id"].as_str().unwrap();
    let owner = TaskOwner::new("agent-a", "chat-a");

    assert!(
        tokio::time::timeout(
            Duration::from_millis(25),
            TaskAwaitTool.call(
                serde_json::json!({"task_ids": [task_id], "timeout": 10}),
                ctx,
            ),
        )
        .await
        .is_err(),
        "the foreground waiter should be detachable"
    );
    assert!(!runtime
        .get(&owner, task_id)
        .await
        .unwrap()
        .state
        .is_terminal());

    execution.release.notify_one();
    wait_for_terminal(&runtime, &owner, task_id).await;
}

struct CancelAwareExecution {
    started: Notify,
}

#[async_trait]
impl ToolTaskExecution for CancelAwareExecution {
    async fn execute(
        &self,
        request: ToolTaskExecutionRequest,
    ) -> Result<agent_types::ToolResult, String> {
        self.started.notify_one();
        request.cancel.cancelled().await;
        tokio::time::sleep(Duration::from_millis(20)).await;
        Ok(agent_types::ToolResult::text(
            format!("{}:{}", request.task_id, request.attempt_no),
            "output captured during cancellation",
        ))
    }
}

#[tokio::test]
async fn task_cancel_tool_cancels_process_runtime_task() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = Arc::new(TaskRuntime::open(TaskRuntimeConfig::new(dir.path())).unwrap());
    let execution = Arc::new(CancelAwareExecution {
        started: Notify::new(),
    });
    runtime.register_runner(Arc::new(ToolTaskRunner::new(execution.clone())));
    let ctx = context(runtime.clone(), "agent-a", "chat-a");
    let created = TaskCreateTool
        .call(
            serde_json::json!({
                "runner": {
                    "type": "tool",
                    "tool_name": "Fixture_Read",
                    "arguments": {"path": "fixture.txt"}
                }
            }),
            ctx.clone(),
        )
        .await
        .unwrap();
    let value: serde_json::Value = serde_json::from_str(&created.text_content()).unwrap();
    let task_id = value["task_id"].as_str().unwrap();
    tokio::time::timeout(Duration::from_secs(2), execution.started.notified())
        .await
        .unwrap();

    let cancelled = TaskCancelTool
        .call(serde_json::json!({"task_id": task_id}), ctx)
        .await
        .unwrap();
    let value: serde_json::Value = serde_json::from_str(&cancelled.text_content()).unwrap();
    assert_eq!(value["status"], "cancelled");
    assert!(value["result"].is_null() || value["result"]["output_available"] == true);
    assert!(!cancelled
        .text_content()
        .contains("output captured during cancellation"));
}

struct FailThenSucceedExecution {
    calls: std::sync::atomic::AtomicUsize,
}

#[async_trait]
impl ToolTaskExecution for FailThenSucceedExecution {
    async fn execute(
        &self,
        request: ToolTaskExecutionRequest,
    ) -> Result<agent_types::ToolResult, String> {
        let call = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if call == 0 {
            Ok(agent_types::ToolResult::error(
                format!("{}:{}", request.task_id, request.attempt_no),
                "first Attempt failed",
            ))
        } else {
            Ok(agent_types::ToolResult::text(
                format!("{}:{}", request.task_id, request.attempt_no),
                "retry succeeded",
            ))
        }
    }
}

#[tokio::test]
async fn task_retry_tool_keeps_task_id_and_appends_attempt() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = Arc::new(TaskRuntime::open(TaskRuntimeConfig::new(dir.path())).unwrap());
    runtime.register_runner(Arc::new(ToolTaskRunner::new(Arc::new(
        FailThenSucceedExecution {
            calls: std::sync::atomic::AtomicUsize::new(0),
        },
    ))));
    let ctx = context(runtime.clone(), "agent-a", "chat-a");
    let created = TaskCreateTool
        .call(
            serde_json::json!({
                "runner": {
                    "type": "tool",
                    "tool_name": "Fixture_Read",
                    "arguments": {"path": "fixture.txt"}
                }
            }),
            ctx.clone(),
        )
        .await
        .unwrap();
    let created: serde_json::Value = serde_json::from_str(&created.text_content()).unwrap();
    let task_id = created["task_id"].as_str().unwrap();
    let owner = TaskOwner::new("agent-a", "chat-a");
    let failed = runtime
        .await_terminal(&owner, task_id, Duration::from_secs(2))
        .await
        .unwrap();
    assert_eq!(failed.state, TaskState::Failed);

    let retried = TaskRetryTool
        .call(serde_json::json!({"task_id": task_id}), ctx)
        .await
        .unwrap();
    let retried: serde_json::Value = serde_json::from_str(&retried.text_content()).unwrap();
    assert_eq!(retried["task_id"], task_id);
    assert_eq!(retried["attempt_no"], 2);
    let completed = runtime
        .await_terminal(&owner, task_id, Duration::from_secs(2))
        .await
        .unwrap();
    assert_eq!(completed.state, TaskState::Completed);
    assert_eq!(completed.attempts.len(), 2);
}

#[derive(Default)]
struct DependencyRecordingExecution {
    dependencies: Mutex<Vec<Vec<rua_task::TaskDependencyResult>>>,
}

#[async_trait]
impl ToolTaskExecution for DependencyRecordingExecution {
    async fn execute(
        &self,
        request: ToolTaskExecutionRequest,
    ) -> Result<agent_types::ToolResult, String> {
        self.dependencies.lock().await.push(request.dependencies);
        Ok(agent_types::ToolResult::text(
            format!("{}:{}", request.task_id, request.attempt_no),
            format!("completed {}", request.tool_name),
        ))
    }
}

#[tokio::test]
async fn task_create_passes_dependency_result_envelopes_to_tool_runner() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = Arc::new(TaskRuntime::open(TaskRuntimeConfig::new(dir.path())).unwrap());
    let execution = Arc::new(DependencyRecordingExecution::default());
    runtime.register_runner(Arc::new(ToolTaskRunner::new(execution.clone())));
    let ctx = context(runtime.clone(), "agent-a", "chat-a");
    let owner = TaskOwner::new("agent-a", "chat-a");

    let upstream = TaskCreateTool
        .call(
            serde_json::json!({
                "instruction": "produce dependency",
                "runner": {
                    "type": "tool",
                    "tool_name": "Fixture_Upstream",
                    "arguments": {}
                }
            }),
            ctx.clone(),
        )
        .await
        .unwrap();
    let upstream: serde_json::Value = serde_json::from_str(&upstream.text_content()).unwrap();
    let upstream_id = upstream["task_id"].as_str().unwrap();
    wait_for_terminal(&runtime, &owner, upstream_id).await;

    let downstream = TaskCreateTool
        .call(
            serde_json::json!({
                "instruction": "consume dependency",
                "depends_on": [upstream_id],
                "runner": {
                    "type": "tool",
                    "tool_name": "Fixture_Downstream",
                    "arguments": {}
                }
            }),
            ctx,
        )
        .await
        .unwrap();
    let downstream: serde_json::Value = serde_json::from_str(&downstream.text_content()).unwrap();
    let downstream_id = downstream["task_id"].as_str().unwrap();
    wait_for_terminal(&runtime, &owner, downstream_id).await;

    let dependencies = execution.dependencies.lock().await;
    assert_eq!(dependencies.len(), 2);
    assert!(dependencies[0].is_empty());
    assert_eq!(dependencies[1].len(), 1);
    assert_eq!(dependencies[1][0].task_id, upstream_id);
    assert_eq!(dependencies[1][0].attempt_no, 1);
    assert_eq!(dependencies[1][0].status, TaskState::Completed);
    assert!(dependencies[1][0].full_result_ref.is_some());
}

#[tokio::test]
async fn legacy_task_create_shape_and_metadata_list_fallback_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = Arc::new(TaskRuntime::open(TaskRuntimeConfig::new(dir.path())).unwrap());
    let owner = TaskOwner::new("agent-a", "chat-a");

    let create_error = TaskCreateTool
        .call(
            serde_json::json!({
                "subject": "legacy task",
                "agent_id": "agent-b",
                "blocked_by": []
            }),
            context(runtime.clone(), "agent-a", "chat-a"),
        )
        .await
        .unwrap_err();
    assert!(create_error.to_string().contains("runner"));
    assert!(runtime
        .list(&owner, TaskListFilter::default())
        .await
        .unwrap()
        .is_empty());

    let mut metadata_context = ToolContext {
        agent_id: "agent-a".to_string(),
        session_id: "chat-a".to_string(),
        ..Default::default()
    };
    metadata_context.metadata.insert(
        "_task_list".to_string(),
        serde_json::json!([{"id": "legacy-task", "state": "running"}]),
    );
    let list_error = TaskListTool
        .call(serde_json::json!({}), metadata_context)
        .await
        .unwrap_err();
    assert!(list_error
        .to_string()
        .contains("Task Runtime is not available"));
}

#[test]
fn canonical_task_tool_schemas_exclude_legacy_scheduler_fields() {
    let create = TaskCreateTool::spec().parameters;
    let create_properties = create["properties"].as_object().unwrap();
    assert!(create_properties.contains_key("runner"));
    for field in ["subject", "agent_id", "blocked_by", "metadata"] {
        assert!(!create_properties.contains_key(field));
    }

    let cancel = TaskCancelTool::spec().parameters;
    assert!(!cancel["properties"]
        .as_object()
        .unwrap()
        .contains_key("cascade"));

    let list = TaskListTool::spec().parameters;
    assert!(!list["properties"]
        .as_object()
        .unwrap()
        .contains_key("agent_id"));

    let await_spec = TaskAwaitTool::spec();
    assert!(await_spec.description.contains("Wait up to"));
    assert!(await_spec.description.contains("without cancelling"));

    let send_input = TaskSendInputTool::spec().parameters;
    assert!(send_input["properties"].get("submission_id").is_none());
    assert!(send_input["properties"].get("bytes").is_none());
    assert!(send_input.get("oneOf").is_none());
    assert!(send_input["properties"]["close_stdin"]
        .get("const")
        .is_none());
    assert!(TaskSendInputTool::spec()
        .description
        .contains("exactly one of"));

    for watch in [TaskWatchTool::spec(), TaskUnwatchTool::spec()] {
        assert_eq!(watch.parameters["required"], serde_json::json!(["task_id"]));
        assert_eq!(
            watch.parameters["properties"]
                .as_object()
                .unwrap()
                .keys()
                .collect::<Vec<_>>(),
            vec!["task_id"]
        );
    }
}
