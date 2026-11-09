use std::sync::Arc;

use rua_core::tool::ToolContext;
use rua_session::WorkspaceSessionStore;
use rua_tool::{
    register_all_builtin_tools, register_extended_tools, MockCommandExecutor, ToolProfile,
    ToolRegistry,
};

fn full_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    register_all_builtin_tools(&mut registry, Arc::new(MockCommandExecutor::new()), None)
        .expect("builtin tools");
    register_extended_tools(&mut registry, ToolProfile::ServerFull).expect("extended tools");
    registry
}

#[tokio::test]
async fn update_plan_accepts_a_complete_checklist_snapshot() {
    let registry = full_registry();
    let tool = registry.get("update_plan").expect("update_plan tool");

    let result = tool
        .call(
            serde_json::json!({
                "explanation": "Investigation is complete; implementation starts next.",
                "plan": [
                    {"step": "Locate the behavior", "status": "completed"},
                    {"step": "Implement the fix", "status": "in_progress"},
                    {"step": "Verify the result", "status": "pending"}
                ]
            }),
            ToolContext::default(),
        )
        .await
        .expect("valid checklist");

    assert_eq!(result.text_content(), "Plan updated");
    assert_eq!(
        result.metadata["work_checklist"]["plan"][1]["status"],
        "in_progress"
    );
}

#[tokio::test]
async fn update_plan_directly_overwrites_the_chat_snapshot() {
    let storage = tempfile::tempdir().expect("storage");
    let registry = full_registry();
    let tool = registry.get("update_plan").expect("update_plan tool");
    let context = ToolContext {
        session_id: "chat-1".to_string(),
        conversation_store_dir: Some(storage.path().to_path_buf()),
        metadata: std::collections::HashMap::from([(
            "turn_id".to_string(),
            serde_json::json!("turn-7"),
        )]),
        ..Default::default()
    };

    tool.call(
        serde_json::json!({
            "plan": [{"step": "First plan", "status": "in_progress"}]
        }),
        context.clone(),
    )
    .await
    .expect("first update");
    tool.call(
        serde_json::json!({
            "explanation": "The second call replaces the first.",
            "plan": [{"step": "Second plan", "status": "completed"}]
        }),
        context,
    )
    .await
    .expect("second update");

    let snapshot = WorkspaceSessionStore::new(storage.path())
        .load_active_work_plan("chat-1")
        .await
        .expect("load snapshot")
        .expect("active work plan");
    assert_eq!(snapshot.source_turn_id.as_deref(), Some("turn-7"));
    assert_eq!(snapshot.source_sequence, None);
    assert_eq!(snapshot.last_injected_sequence, None);
    assert_eq!(
        snapshot.explanation.as_deref(),
        Some("The second call replaces the first.")
    );
    assert_eq!(snapshot.plan.len(), 1);
    assert_eq!(snapshot.plan[0].step, "Second plan");
}

#[tokio::test]
async fn update_plan_rejects_multiple_in_progress_steps() {
    let registry = full_registry();
    let tool = registry.get("update_plan").expect("update_plan tool");

    let error = tool
        .call(
            serde_json::json!({
                "plan": [
                    {"step": "First", "status": "in_progress"},
                    {"step": "Second", "status": "in_progress"}
                ]
            }),
            ToolContext::default(),
        )
        .await
        .expect_err("multiple in-progress steps must fail");

    assert!(error.to_string().contains("at most one"));
}

#[tokio::test]
async fn update_plan_rejects_empty_step_text() {
    let registry = full_registry();
    let tool = registry.get("update_plan").expect("update_plan tool");

    let error = tool
        .call(
            serde_json::json!({
                "plan": [{"step": "  ", "status": "pending"}]
            }),
            ToolContext::default(),
        )
        .await
        .expect_err("empty step text must fail");

    assert!(error.to_string().contains("must not be empty"));
}
