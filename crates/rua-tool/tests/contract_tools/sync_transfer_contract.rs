use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use rua_contract::Tool;
use rua_core::tool::ToolContext;
use rua_core::RuaError;
use rua_tool::extended::sync_transfer::{
    SyncTransferRequest, SyncTransferRuntime, SyncTransferRuntimeHandle, SyncTransferTool,
};
use serde_json::{json, Value};
use tokio::sync::{watch, Notify};

#[derive(Default)]
struct Runtime {
    calls: Mutex<Vec<(SyncTransferRequest, ToolContext)>>,
    reply: Value,
}

#[async_trait]
impl SyncTransferRuntime for Runtime {
    async fn execute(
        &self,
        request: SyncTransferRequest,
        ctx: &ToolContext,
    ) -> Result<Value, String> {
        let mut captured = ctx.clone();
        captured.sync_transfer_runtime = None;
        self.calls.lock().unwrap().push((request, captured));
        Ok(self.reply.clone())
    }
}

fn context(runtime: Arc<dyn SyncTransferRuntime>) -> ToolContext {
    ToolContext {
        agent_id: "selected-agent".into(),
        session_id: "original-chat".into(),
        local_device_id: Some("desktop".into()),
        working_dir: "/unrelated-chat-cwd".into(),
        metadata: [("turn_id".into(), json!("original-turn"))].into(),
        sync_transfer_runtime: Some(Arc::new(SyncTransferRuntimeHandle::new(runtime))),
        ..Default::default()
    }
}

fn args() -> Value {
    json!({
        "source_device_id":"phone", "source_paths":["/storage/photos/a.jpg"],
        "target_device_id":"desktop", "target_dir":"/photos/incoming"
    })
}

#[tokio::test]
async fn explicit_device_paths_context_and_conflict_intent_reach_the_runtime_unchanged() {
    let reply = json!({"status":"succeeded","target_device_id":"desktop","paths":["/photos/incoming/a (1).jpg"],"files_written":1});
    let runtime = Arc::new(Runtime {
        reply: reply.clone(),
        ..Default::default()
    });
    let mut args = args();
    args["future_option"] = json!({"preserve":"unknown fields"});
    args["source_paths"] = json!([r"C:\Photos\a.jpg", "/storage/photos/b.jpg"]);
    for conflict in [None, Some("rename"), Some("overwrite"), Some("fail")] {
        if let Some(conflict) = conflict {
            args["conflict"] = json!(conflict);
        }
        let result = SyncTransferTool
            .call(args.clone(), context(runtime.clone()))
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&result.text_content()).unwrap(),
            reply
        );
        assert!(!result.metadata.contains_key("agents_instructions"));
        let calls = runtime.calls.lock().unwrap();
        let (
            SyncTransferRequest::Send {
                arguments,
                source_device_id,
                source_paths,
                target_device_id,
                target_dir,
                conflict: actual,
            },
            ctx,
        ) = calls.last().unwrap();
        assert_eq!(arguments, &args);
        assert_eq!(source_device_id, "phone");
        assert_eq!(source_paths, &[r"C:\Photos\a.jpg", "/storage/photos/b.jpg"]);
        assert_eq!(target_device_id, "desktop");
        assert_eq!(target_dir, "/photos/incoming");
        assert_eq!(actual.as_deref(), conflict);
        assert_eq!(ctx.agent_id, "selected-agent");
        assert_eq!(ctx.session_id, "original-chat");
        assert_eq!(ctx.local_device_id.as_deref(), Some("desktop"));
        assert_eq!(ctx.metadata["turn_id"], "original-turn");
    }
    let schema = SyncTransferTool.schema();
    assert_eq!(
        schema.parameters["properties"]["conflict"]["default"],
        "rename"
    );
    assert_eq!(
        schema.parameters["properties"]["source_paths"]["minItems"],
        1
    );
}

#[tokio::test]
async fn invalid_locations_or_conflict_are_rejected_before_any_runtime_work() {
    let runtime = Arc::new(Runtime::default());
    for (field, value) in [
        ("source_device_id", json!("")),
        ("target_device_id", json!(" desktop ")),
        ("source_paths", json!([])),
        ("source_paths", json!(["relative/photo.jpg"])),
        ("source_paths", json!(["/valid.jpg", "/bad\u{0000}path"])),
        ("source_paths", json!(["chat-media:some-id"])),
        ("target_dir", json!("~/Downloads")),
        ("target_dir", json!("")),
        ("conflict", json!("move")),
    ] {
        let mut invalid = args();
        invalid[field] = value;
        assert!(
            SyncTransferTool
                .call(invalid, context(runtime.clone()))
                .await
                .is_err(),
            "accepted invalid {field}"
        );
    }
    assert!(runtime.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn cancellation_before_dispatch_never_invokes_the_runtime() {
    let runtime = Arc::new(Runtime::default());
    let (_sender, receiver) = watch::channel(true);
    let mut ctx = context(runtime.clone());
    ctx.cancel = Some(receiver);
    assert!(matches!(
        SyncTransferTool.call(args(), ctx).await,
        Err(RuaError::Cancelled)
    ));
    assert!(runtime.calls.lock().unwrap().is_empty());
}

struct WaitingRuntime {
    started: Notify,
    release: Notify,
    wait_for_cancel: bool,
}

#[async_trait]
impl SyncTransferRuntime for WaitingRuntime {
    async fn execute(&self, _: SyncTransferRequest, ctx: &ToolContext) -> Result<Value, String> {
        self.started.notify_one();
        if self.wait_for_cancel {
            let mut cancel = ctx.cancel.clone().expect("original cancellation receiver");
            cancel
                .wait_for(|cancelled| *cancelled)
                .await
                .map_err(|e| e.to_string())?;
            Ok(json!({"status":"cancelled","files_written":0}))
        } else {
            self.release.notified().await;
            Ok(json!({"status":"succeeded","files_written":1,"paths":["/photos/incoming/a.jpg"]}))
        }
    }
}

#[tokio::test]
async fn the_tool_waits_for_runtime_completion_evidence() {
    let runtime = Arc::new(WaitingRuntime {
        started: Notify::new(),
        release: Notify::new(),
        wait_for_cancel: false,
    });
    let operation = tokio::spawn(SyncTransferTool.call(args(), context(runtime.clone())));
    tokio::time::timeout(Duration::from_secs(2), runtime.started.notified())
        .await
        .unwrap();
    assert!(!operation.is_finished());
    runtime.release.notify_one();
    let result = tokio::time::timeout(Duration::from_secs(2), operation)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let result: Value = serde_json::from_str(&result.text_content()).unwrap();
    assert_eq!(result["status"], "succeeded");
    assert_eq!(result["paths"], json!(["/photos/incoming/a.jpg"]));
}

#[tokio::test]
async fn cancellation_reaches_running_transfer_and_preserves_its_reported_outcome() {
    let runtime = Arc::new(WaitingRuntime {
        started: Notify::new(),
        release: Notify::new(),
        wait_for_cancel: true,
    });
    let (sender, receiver) = watch::channel(false);
    let mut ctx = context(runtime.clone());
    ctx.cancel = Some(receiver);
    let operation = tokio::spawn(SyncTransferTool.call(args(), ctx));
    tokio::time::timeout(Duration::from_secs(2), runtime.started.notified())
        .await
        .unwrap();
    sender.send(true).unwrap();
    let result = tokio::time::timeout(Duration::from_secs(2), operation)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&result.text_content()).unwrap()["status"],
        "cancelled"
    );
}

#[tokio::test]
async fn planned_subdirectory_instructions_return_the_existing_reconsideration_metadata() {
    let runtime = Arc::new(Runtime {
        reply: json!({"status":"not_executed","agents_instructions":"reconsider","reason":"Read target subdirectory AGENTS.md before copying."}),
        ..Default::default()
    });
    let result = SyncTransferTool
        .call(args(), context(runtime))
        .await
        .unwrap();
    assert!(!result.is_error);
    assert_eq!(result.metadata["execution_status"], "not_executed");
    assert_eq!(result.metadata["agents_instructions"], "reconsider");
    assert!(result
        .text_content()
        .contains("target subdirectory AGENTS.md"));
}
