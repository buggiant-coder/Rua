use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::http::{Method, StatusCode};
use futures::StreamExt;
use rua_contract::Tool;
use rua_core::tool::ToolContext;
use rua_session::{
    NewConversationMessage, NewConversationTurn, RuntimeNote, RuntimeNotePayload,
    TaskAttemptTerminalRuntimeNote, TurnLifecycle, WorkspaceSessionStore,
};
use rua_task::{
    TaskArtifact, TaskInput, TaskInputCapability, TaskInputError, TaskInputSink, TaskOwner,
    TaskResourceRef, TaskRunOutput, TaskRunRequest, TaskRunner, TaskRunnerError, TaskRunnerKind,
    TaskRunnerSpec, TaskSpec, TaskState,
};
use rua_tool::extended::task_tools::{TaskCreateTool, TaskRuntimeHandle};
use tokio::sync::{Mutex, Notify};

use super::support::{build_app, create_agent, read_json, request, seed_materialized_chat};

async fn wait_for_terminal(
    state: &rua_server::AppState,
    owner: &TaskOwner,
    task_id: &str,
) -> rua_task::TaskRecord {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(record) = state.task_runtime.get(owner, task_id).await {
                if record.state.is_terminal() {
                    break record;
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("Task should reach terminal state")
}

async fn chat_configuration_epoch(workspace: &std::path::Path, chat_id: &str) -> String {
    WorkspaceSessionStore::new(workspace)
        .load_chat_header(chat_id)
        .await
        .unwrap()
        .unwrap()
        .extra["hook_configuration"]["epoch"]
        .as_str()
        .unwrap()
        .to_string()
}

async fn enable_agent_tool(state: &rua_server::AppState, agent_id: &str, tool_name: &str) {
    let mut tools = rua_config::types::tools::default_builtin_tools();
    for (name, config) in &mut tools {
        config["enabled"] = serde_json::Value::Bool(name == tool_name);
    }
    state
        .agent_tools_cells
        .cell(agent_id)
        .write_value(serde_json::json!({
            "selection_policy": rua_config::types::tools::ToolSelectionPolicy::restricted(vec![rua_config::types::tools::ToolIdentity::builtin(tool_name)]),
            "builtin_tools": tools
        }))
        .await
        .unwrap();
}

struct ShutdownOwnershipRunner;
#[async_trait]
impl TaskRunner for ShutdownOwnershipRunner {
    fn kind(&self) -> TaskRunnerKind {
        TaskRunnerKind::Tool
    }
    async fn run(&self, request: TaskRunRequest) -> Result<TaskRunOutput, TaskRunnerError> {
        request.cancel.cancelled().await;
        Ok(TaskRunOutput::cancelled())
    }
}

#[tokio::test]
async fn workspace_shutdown_cancels_only_the_authorized_agent_tasks_in_a_project_chat() {
    use rua_workspace::AgentWorkAuthority;
    let app = build_app();
    let temp = tempfile::tempdir().unwrap();
    let runtime = Arc::new(
        rua_task::TaskRuntime::open(rua_task::TaskRuntimeConfig::new(temp.path())).unwrap(),
    );
    runtime.register_runner(Arc::new(ShutdownOwnershipRunner));
    let owner = TaskOwner::from_address(rua_core::ChatAddress::project(
        "project-task-only",
        "chat-task-only",
    ));
    let make_spec = || TaskSpec {
        owner: owner.clone(),
        instruction: "wait for cancellation".to_string(),
        runner: TaskRunnerSpec::Tool {
            tool_name: "fixture".to_string(),
            arguments: serde_json::json!({}),
        },
        context_refs: Vec::new(),
        depends_on: Vec::new(),
        origin: Default::default(),
    };
    let a = runtime
        .create(
            make_spec(),
            rua_task::TaskAttemptAuthorization::new("agent-a").unwrap(),
        )
        .await
        .unwrap();
    let b = runtime
        .create(
            make_spec(),
            rua_task::TaskAttemptAuthorization::new("agent-b").unwrap(),
        )
        .await
        .unwrap();
    let authority = rua_server::services::workspace_work_lifecycle::WorkspaceWorkAuthority::new(
        app.state.conversation_authority.clone(),
        app.state.streams.clone(),
        runtime.clone(),
        rua_workspace::RuaRoot::new(temp.path()),
        None,
    );
    authority.begin_shutdown("agent-a").await.unwrap();
    let before = authority.outstanding_work("agent-a").await.unwrap();
    assert_eq!(before.units.len(), 1);
    assert_eq!(before.units[0].id, a);
    authority.cancel_work("agent-a").await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while !runtime.get(&owner, &a).await.unwrap().state.is_terminal() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(!runtime.get(&owner, &b).await.unwrap().state.is_terminal());
    assert_eq!(
        authority
            .outstanding_work("agent-b")
            .await
            .unwrap()
            .units
            .len(),
        1
    );
    runtime.shutdown().await.unwrap();
}

struct HttpTaskInputSink {
    inputs: Mutex<Vec<TaskInput>>,
}

#[async_trait]
impl TaskInputSink for HttpTaskInputSink {
    fn declared_capability(&self) -> TaskInputCapability {
        TaskInputCapability::Stdin
    }

    fn active_capability(&self) -> TaskInputCapability {
        TaskInputCapability::Stdin
    }

    async fn send_input(&self, input: &TaskInput) -> Result<(), TaskInputError> {
        self.inputs.lock().await.push(input.clone());
        Ok(())
    }

    async fn close(&self) {}
}

struct HttpTaskInputRunner {
    sink: Arc<HttpTaskInputSink>,
    started: Notify,
    release: Notify,
}

#[async_trait]
impl TaskRunner for HttpTaskInputRunner {
    fn kind(&self) -> TaskRunnerKind {
        TaskRunnerKind::Tool
    }

    async fn run(&self, request: TaskRunRequest) -> Result<TaskRunOutput, TaskRunnerError> {
        request.session.activate(self.sink.clone()).await.unwrap();
        self.started.notify_one();
        self.release.notified().await;
        Ok(TaskRunOutput::completed(
            "Outcome\nDone\n\nKey Results\ninput accepted\n\nVerification\npassed\n\nRemaining Work\nNone",
        ))
    }
}

#[tokio::test]
async fn task_input_http_routes_by_chat_and_preserves_submission_identity() {
    let test = build_app();
    create_agent(&test.app, "task-input-agent", "Task Input Agent").await;
    let chat = seed_materialized_chat(
        &test,
        "/api/agents/task-input-agent/chats",
        Some("task-input-agent"),
        "task-input-chat",
    )
    .await;
    let chat_id = chat["id"].as_str().unwrap().to_string();
    let owner = TaskOwner::new("task-input-agent", &chat_id);
    let runner = Arc::new(HttpTaskInputRunner {
        sink: Arc::new(HttpTaskInputSink {
            inputs: Mutex::new(Vec::new()),
        }),
        started: Notify::new(),
        release: Notify::new(),
    });
    test.state.task_runtime.register_runner(runner.clone());
    let task_id = test
        .state
        .task_runtime
        .create(
            TaskSpec {
                owner: owner.clone(),
                instruction: "accept HTTP input".to_string(),
                runner: TaskRunnerSpec::Tool {
                    tool_name: "Fixture_HttpInput".to_string(),
                    arguments: serde_json::json!({}),
                },
                context_refs: Vec::new(),
                depends_on: Vec::new(),
                origin: Default::default(),
            },
            rua_task::TaskAttemptAuthorization::new("task-input-agent").unwrap(),
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), runner.started.notified())
        .await
        .unwrap();

    let detail = request(
        &test.app,
        Method::GET,
        &format!("/api/agents/task-input-agent/chats/{chat_id}/tasks/{task_id}"),
        None,
        Some("task-input-agent"),
    )
    .await;
    assert_eq!(detail.status(), StatusCode::OK);
    assert_eq!(read_json(detail).await["input_capability"], "stdin");

    let input_uri = format!("/api/agents/task-input-agent/chats/{chat_id}/tasks/{task_id}/input");
    let body = serde_json::json!({
        "submission_id": "http-submission-a",
        "input": {"type": "text", "text": "exact"}
    });
    let first = request(
        &test.app,
        Method::POST,
        &input_uri,
        Some(body.clone()),
        Some("task-input-agent"),
    )
    .await;
    assert_eq!(first.status(), StatusCode::OK);
    let first = read_json(first).await;
    assert_eq!(first["accepted"], true);
    assert_eq!(first["attempt_no"], 1);
    assert_eq!(first["input_seq"], 1);

    let duplicate = request(
        &test.app,
        Method::POST,
        &input_uri,
        Some(body),
        Some("task-input-agent"),
    )
    .await;
    assert_eq!(duplicate.status(), StatusCode::OK);
    assert_eq!(read_json(duplicate).await, first);
    assert_eq!(runner.sink.inputs.lock().await.len(), 1);

    let conflict = request(
        &test.app,
        Method::POST,
        &input_uri,
        Some(serde_json::json!({
            "submission_id": "http-submission-a",
            "input": {"type": "text", "text": "different"}
        })),
        Some("task-input-agent"),
    )
    .await;
    assert_eq!(conflict.status(), StatusCode::CONFLICT);

    let wrong_chat = request(
        &test.app,
        Method::POST,
        &format!("/api/agents/task-input-agent/chats/another-chat/tasks/{task_id}/input"),
        Some(serde_json::json!({
            "submission_id": "wrong-chat",
            "input": {"type": "text", "text": "no"}
        })),
        Some("task-input-agent"),
    )
    .await;
    assert_eq!(wrong_chat.status(), StatusCode::NOT_FOUND);

    let binary = request(
        &test.app,
        Method::POST,
        &input_uri,
        Some(serde_json::json!({
            "submission_id": "binary",
            "input": {"type": "binary", "bytes": "AA=="}
        })),
        Some("task-input-agent"),
    )
    .await;
    assert!(binary.status().is_client_error());

    runner.release.notify_one();
    let terminal = wait_for_terminal(&test.state, &owner, &task_id).await;
    assert_eq!(terminal.input_capability, TaskInputCapability::None);
}

#[cfg(unix)]
#[tokio::test]
async fn watched_shell_output_starts_runtime_only_notification_turn() {
    let test = build_app();
    create_agent(&test.app, "task-watch-agent", "Task Watch Agent").await;
    enable_agent_tool(&test.state, "task-watch-agent", "bash").await;
    let chat = seed_materialized_chat(
        &test,
        "/api/agents/task-watch-agent/chats",
        Some("task-watch-agent"),
        "task-watch-chat",
    )
    .await;
    let chat_id = chat["id"].as_str().unwrap().to_string();
    let workspace = test
        .state
        .registry
        .get_or_create("task-watch-agent")
        .await
        .expect("task watch workspace runtime")
        .workspace_dir();
    let tool_result_store_dir = WorkspaceSessionStore::new(&workspace)
        .chat_dir(&chat_id)
        .join("tool-results");
    std::fs::create_dir_all(&tool_result_store_dir).unwrap();
    for marker in ["watch-ready", "watch-release"] {
        let path = workspace.join(marker);
        if path.exists() {
            std::fs::remove_file(path).unwrap();
        }
    }
    let owner = TaskOwner::new("task-watch-agent", &chat_id);
    let configuration_epoch = chat_configuration_epoch(&workspace, &chat_id).await;
    let created = TaskCreateTool
        .call(
            serde_json::json!({
                "runner": {
                    "type": "tool",
                    "tool_name": "bash",
                    "arguments": {
                        "command": "printf 'historical\\n'; sleep 0.1; : > watch-ready; while [ ! -f watch-release ]; do sleep 0.01; done; printf 'watched-line\\n'; printf final-watch-tail"
                    }
                }
            }),
            ToolContext {
                agent_id: "task-watch-agent".to_string(),
                session_id: chat_id.clone(),
                working_dir: workspace.clone(),
                tool_result_store_dir: Some(tool_result_store_dir),
                metadata: std::collections::HashMap::from([(
                    "chat_configuration_epoch".to_string(),
                    serde_json::Value::String(configuration_epoch),
                )]),
                task_runtime: Some(Arc::new(TaskRuntimeHandle::new(Arc::clone(
                    &test.state.task_runtime,
                )))),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let created: serde_json::Value = serde_json::from_str(&created.text_content()).unwrap();
    let task_id = created["task_id"].as_str().unwrap().to_string();
    tokio::time::timeout(Duration::from_secs(3), async {
        while !workspace.join("watch-ready").is_file() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let watch = request(
        &test.app,
        Method::POST,
        &format!("/api/agents/task-watch-agent/chats/{chat_id}/tasks/{task_id}/watch"),
        None,
        Some("task-watch-agent"),
    )
    .await;
    assert_eq!(watch.status(), StatusCode::OK);
    let watch_body = read_json(watch).await;
    assert_eq!(watch_body["active"], true);
    std::fs::write(workspace.join("watch-release"), "").unwrap();

    let store = WorkspaceSessionStore::new(&workspace);
    let notifications = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let messages = store
                .read_message_range(&chat_id, 1, usize::MAX)
                .await
                .unwrap();
            let notifications = messages
                .iter()
                .filter_map(|message| {
                    let note = RuntimeNote::from_message_log_entry(message)?;
                    matches!(
                        note.payload,
                        RuntimeNotePayload::TaskWatchOutput(ref output)
                            if output.task_id == task_id
                    )
                    .then(|| (note, message.turn_id.clone()))
                })
                .collect::<Vec<_>>();
            if notifications.iter().any(|(note, _)| {
                matches!(
                    &note.payload,
                    RuntimeNotePayload::TaskWatchOutput(output)
                        if output.records.iter().map(|record| record.text.as_str()).collect::<String>().contains("final-watch-tail")
                )
            }) {
                break notifications;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("Task Watch notification Runtime Note");
    let records = notifications
        .iter()
        .flat_map(|(note, _)| match &note.payload {
            RuntimeNotePayload::TaskWatchOutput(output) => output.records.iter(),
            _ => unreachable!(),
        })
        .collect::<Vec<_>>();
    let text = records
        .iter()
        .map(|record| record.text.as_str())
        .collect::<String>();
    assert!(text.contains("watched-line\n"));
    assert!(text.contains("final-watch-tail"));
    assert!(!text.contains("historical"));

    let notification_turn_ids = notifications
        .iter()
        .map(|(_, turn_id)| turn_id.as_str())
        .collect::<std::collections::HashSet<_>>();
    let notification_turn_messages = store
        .read_message_range(&chat_id, 1, usize::MAX)
        .await
        .unwrap()
        .into_iter()
        .filter(|message| notification_turn_ids.contains(message.turn_id.as_str()))
        .collect::<Vec<_>>();
    assert!(notification_turn_messages
        .iter()
        .all(|message| message.role != "user"));
    assert!(notification_turn_messages.iter().any(|message| {
        RuntimeNote::from_message_log_entry(message)
            .is_some_and(|note| matches!(note.payload, RuntimeNotePayload::TaskWatchOutput(_)))
    }));

    let terminal = wait_for_terminal(&test.state, &owner, &task_id).await;
    assert_eq!(terminal.state, TaskState::Completed);
}

async fn seed_runtime_note_turn(store: &WorkspaceSessionStore, chat_id: &str, turn_id: &str) {
    store
        .append_conversation_turn(
            chat_id,
            NewConversationTurn {
                turn_id: turn_id.to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: chrono::Utc::now().to_rfc3339(),
                    assistant_started_at: Some(chrono::Utc::now().to_rfc3339()),
                    assistant_completed_at: Some(chrono::Utc::now().to_rfc3339()),
                    status: "completed".to_string(),
                },
                selected_variant_id: None,
                messages: vec![NewConversationMessage {
                    message_id: format!("user-{turn_id}"),
                    role: "user".to_string(),
                    created_at: chrono::Utc::now().to_rfc3339(),
                    status: "completed".to_string(),
                    content: serde_json::json!([{"type": "text", "text": "start task"}]),
                    provider: None,
                    tool: None,
                    metadata: serde_json::Map::new(),
                }],
            },
        )
        .await
        .expect("seed Runtime Note turn");
}

async fn wait_for_task_runtime_notes(
    store: &WorkspaceSessionStore,
    chat_id: &str,
    task_id: &str,
    expected: usize,
) -> Vec<RuntimeNote> {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let notes = store
                .read_message_range(chat_id, 1, usize::MAX)
                .await
                .expect("read Message Log")
                .iter()
                .filter_map(RuntimeNote::from_message_log_entry)
                .filter(|note| {
                    matches!(
                        note.payload,
                        RuntimeNotePayload::TaskAttemptTerminal(ref terminal)
                            if terminal.task_id == task_id
                    )
                })
                .collect::<Vec<_>>();
            if notes.len() == expected {
                break notes;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("Task Runtime Notes")
}

fn task_attempt_note(note: &RuntimeNote) -> &TaskAttemptTerminalRuntimeNote {
    let RuntimeNotePayload::TaskAttemptTerminal(note) = &note.payload else {
        panic!("expected Task Attempt Terminal Runtime Note");
    };
    note
}

#[tokio::test]
async fn scoped_task_http_reads_real_tool_task_and_hides_cross_chat_access() {
    let test = build_app();
    create_agent(&test.app, "task-agent", "Task Agent").await;
    enable_agent_tool(&test.state, "task-agent", "time_now").await;
    let config_path = test.state.rua_root.join("config.json");
    let mut config = rua_config::load_config(&config_path, &test.state.rua_root).unwrap();
    config.user_timezone = Some("Pacific/Honolulu".to_string());
    rua_config::save_config(&config, &config_path).unwrap();
    let chat = seed_materialized_chat(
        &test,
        "/api/agents/task-agent/chats",
        Some("task-agent"),
        "task-chat",
    )
    .await;
    let chat_id = chat["id"].as_str().expect("chat id").to_string();
    let owner = TaskOwner::new("task-agent", &chat_id);
    let workspace_dir = test.state.rua_root.join("workspaces").join("task-agent");
    let session_store = WorkspaceSessionStore::new(&workspace_dir);
    seed_runtime_note_turn(&session_store, &chat_id, "turn-task-http").await;
    let configuration_epoch = chat_configuration_epoch(&workspace_dir, &chat_id).await;

    let created = TaskCreateTool
        .call(
            serde_json::json!({
                "instruction": "Report current time",
                "runner": {
                    "type": "tool",
                    "tool_name": "time_now",
                    "arguments": {}
                }
            }),
            ToolContext {
                agent_id: "task-agent".to_string(),
                session_id: chat_id.clone(),
                working_dir: workspace_dir,
                metadata: std::collections::HashMap::from([
                    (
                        "turn_id".to_string(),
                        serde_json::Value::String("turn-task-http".to_string()),
                    ),
                    (
                        "tool_use_id".to_string(),
                        serde_json::Value::String("call-task-http".to_string()),
                    ),
                    (
                        "chat_configuration_epoch".to_string(),
                        serde_json::Value::String(configuration_epoch),
                    ),
                ]),
                task_runtime: Some(Arc::new(TaskRuntimeHandle::new(Arc::clone(
                    &test.state.task_runtime,
                )))),
                ..Default::default()
            },
        )
        .await
        .expect("task_create");
    let created: serde_json::Value =
        serde_json::from_str(&created.text_content()).expect("task_create JSON result");
    let task_id = created["task_id"].as_str().expect("task id").to_string();
    let record = wait_for_terminal(&test.state, &owner, &task_id).await;
    assert_eq!(
        record.state,
        TaskState::Completed,
        "Task record: {record:#?}"
    );
    assert!(record
        .result
        .as_ref()
        .unwrap()
        .output
        .contains(" Pacific/Honolulu ("));
    let local_device_id = test
        .state
        .sync_device
        .read()
        .unwrap()
        .as_ref()
        .map(|device| hex::encode(device.device_id_short()))
        .unwrap();
    assert_eq!(
        record.attempts[0].device.device_id.as_deref(),
        Some(local_device_id.as_str())
    );
    assert!(record.attempts[0].device.authority_epoch.is_some());
    assert_eq!(
        record.attempts[0].device.host_profile.as_deref(),
        Some("test")
    );
    assert!(record.attempts[0]
        .device
        .device_name
        .as_deref()
        .is_some_and(|name| !name.is_empty()));
    assert_eq!(
        record.result.as_ref().unwrap().device,
        record.attempts[0].device
    );

    let notes = wait_for_task_runtime_notes(&session_store, &chat_id, &task_id, 1).await;
    let note = task_attempt_note(&notes[0]);
    assert_eq!(note.runner_name, "time_now");
    assert_eq!(note.attempt_no, 1);
    assert!(note.output.contains("completed"));
    let reopened = WorkspaceSessionStore::new(test.state.rua_root.join("workspaces/task-agent"));
    let replayed = wait_for_task_runtime_notes(&reopened, &chat_id, &task_id, 1).await;
    assert_eq!(
        replayed.len(),
        1,
        "reopening the Chat must retain one automatic terminal observation"
    );

    let list = request(
        &test.app,
        Method::GET,
        &format!("/api/agents/task-agent/chats/{chat_id}/tasks"),
        None,
        Some("task-agent"),
    )
    .await;
    assert_eq!(list.status(), StatusCode::OK);
    let list_body = read_json(list).await;
    assert_eq!(list_body.as_array().unwrap().len(), 1);
    assert_eq!(list_body[0]["task_id"], task_id);

    let get = request(
        &test.app,
        Method::GET,
        &format!("/api/agents/task-agent/chats/{chat_id}/tasks/{task_id}"),
        None,
        Some("task-agent"),
    )
    .await;
    assert_eq!(get.status(), StatusCode::OK);
    let get_body = read_json(get).await;
    assert_eq!(get_body["state"], "completed");
    assert!(get_body["result"]["output"]
        .as_str()
        .unwrap()
        .contains("completed"));

    let result = request(
        &test.app,
        Method::GET,
        &format!("/api/agents/task-agent/chats/{chat_id}/tasks/{task_id}/result"),
        None,
        Some("task-agent"),
    )
    .await;
    assert_eq!(result.status(), StatusCode::OK);
    let result_body = read_json(result).await;
    assert_eq!(result_body["name"], "time_now");

    *test.state.sync_device.write().unwrap() =
        Some(Arc::new(rua_sync::identity::DeviceIdentity::generate()));
    let unavailable = request(
        &test.app,
        Method::GET,
        &format!("/api/agents/task-agent/chats/{chat_id}/tasks/{task_id}/result"),
        None,
        Some("task-agent"),
    )
    .await;
    assert_eq!(unavailable.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(read_json(unavailable).await["detail"]
        .as_str()
        .unwrap()
        .contains(&local_device_id));

    let cross_chat = request(
        &test.app,
        Method::GET,
        &format!("/api/agents/task-agent/chats/another-chat/tasks/{task_id}"),
        None,
        Some("task-agent"),
    )
    .await;
    assert_eq!(cross_chat.status(), StatusCode::NOT_FOUND);
    assert_eq!(read_json(cross_chat).await["detail"], "Task not found");
}

struct ScopedResultRunner;

#[async_trait]
impl TaskRunner for ScopedResultRunner {
    fn kind(&self) -> TaskRunnerKind {
        TaskRunnerKind::Tool
    }

    async fn run(&self, request: TaskRunRequest) -> Result<TaskRunOutput, TaskRunnerError> {
        let scope = format!(
            "{}:{}:{}",
            request.spec.owner.agent_id().unwrap_or("project"),
            request.spec.owner.chat_id(),
            request.task_id
        );
        Ok(TaskRunOutput {
            outcome: rua_task::TaskRunOutcome::Completed,
            output: format!(
                "Outcome\nCompleted.\n\nKey Results\n{scope}\n\nVerification\nScoped.\n\nRemaining Work\nNone"
            ),
            raw_output: Some(
                serde_json::to_vec(&serde_json::json!({"scope": scope.clone()})).unwrap(),
            ),
            artifacts: vec![TaskArtifact {
                id: "scope-artifact".to_string(),
                name: "scope.txt".to_string(),
                media_type: "text/plain".to_string(),
                resource: TaskResourceRef {
                    uri: format!(
                        "task://{}/attempts/{}/scope.txt",
                        request.task_id, request.attempt_no
                    ),
                    ref_type: "task_artifact".to_string(),
                    source_device_id: None,
                },
                metadata: serde_json::json!({"scope": scope}),
            }],
            artifact_payloads: Vec::new(),
            error: None,
        })
    }
}

#[tokio::test]
async fn process_task_runtime_isolates_two_agents_and_multiple_chats() {
    let test = build_app();
    test.state
        .task_runtime
        .register_runner(Arc::new(ScopedResultRunner));
    let mut scopes = Vec::new();
    for agent_id in ["scope-agent-a", "scope-agent-b"] {
        create_agent(&test.app, agent_id, agent_id).await;
        for chat_name in ["chat-one", "chat-two"] {
            let chat = seed_materialized_chat(
                &test,
                &format!("/api/agents/{agent_id}/chats"),
                Some(agent_id),
                chat_name,
            )
            .await;
            scopes.push((
                agent_id.to_string(),
                chat["id"].as_str().unwrap().to_string(),
            ));
        }
    }

    let mut tasks = Vec::new();
    for (agent_id, chat_id) in &scopes {
        let created = TaskCreateTool
            .call(
                serde_json::json!({
                    "instruction": format!("run {agent_id}:{chat_id}"),
                    "runner": {
                        "type": "tool",
                        "tool_name": "Fixture_Scoped",
                        "arguments": {}
                    }
                }),
                ToolContext {
                    agent_id: agent_id.clone(),
                    session_id: chat_id.clone(),
                    working_dir: test.state.rua_root.join("workspaces").join(agent_id),
                    task_runtime: Some(Arc::new(TaskRuntimeHandle::new(Arc::clone(
                        &test.state.task_runtime,
                    )))),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let created: serde_json::Value = serde_json::from_str(&created.text_content()).unwrap();
        tasks.push((
            agent_id.clone(),
            chat_id.clone(),
            created["task_id"].as_str().unwrap().to_string(),
        ));
    }
    for (agent_id, chat_id, task_id) in &tasks {
        let completed =
            wait_for_terminal(&test.state, &TaskOwner::new(agent_id, chat_id), task_id).await;
        assert_eq!(completed.state, TaskState::Completed);
    }

    for (agent_id, chat_id, task_id) in &tasks {
        let list = request(
            &test.app,
            Method::GET,
            &format!("/api/agents/{agent_id}/chats/{chat_id}/tasks"),
            None,
            Some(agent_id),
        )
        .await;
        assert_eq!(list.status(), StatusCode::OK);
        let list = read_json(list).await;
        assert_eq!(list.as_array().unwrap().len(), 1);
        assert_eq!(list[0]["task_id"], task_id.as_str());

        let detail = request(
            &test.app,
            Method::GET,
            &format!("/api/agents/{agent_id}/chats/{chat_id}/tasks/{task_id}"),
            None,
            Some(agent_id),
        )
        .await;
        assert_eq!(detail.status(), StatusCode::OK);
        let detail = read_json(detail).await;
        assert_eq!(
            detail["result"]["artifacts"][0]["metadata"]["scope"],
            format!("{agent_id}:{chat_id}:{task_id}")
        );

        let result = request(
            &test.app,
            Method::GET,
            &format!("/api/agents/{agent_id}/chats/{chat_id}/tasks/{task_id}/result"),
            None,
            Some(agent_id),
        )
        .await;
        assert_eq!(result.status(), StatusCode::OK);
        assert_eq!(
            read_json(result).await["scope"],
            format!("{agent_id}:{chat_id}:{task_id}")
        );

        let activity = request(
            &test.app,
            Method::GET,
            &format!("/api/agents/{agent_id}/chats/{chat_id}/tasks/activity"),
            None,
            Some(agent_id),
        )
        .await;
        let mut stream = activity.into_body().into_data_stream();
        let snapshot = tokio::time::timeout(Duration::from_secs(2), stream.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let snapshot = String::from_utf8(snapshot.to_vec()).unwrap();
        assert!(snapshot.contains(task_id));
        for (_, _, other_task_id) in &tasks {
            if other_task_id != task_id {
                assert!(!snapshot.contains(other_task_id));
            }
        }
    }

    let (owner_agent, owner_chat, task_id) = &tasks[0];
    let (other_agent, other_chat, _) = &tasks[3];
    for (method, suffix) in [
        (Method::GET, ""),
        (Method::GET, "/result"),
        (Method::POST, "/cancel"),
        (Method::POST, "/retry"),
    ] {
        let response = request(
            &test.app,
            method,
            &format!("/api/agents/{other_agent}/chats/{other_chat}/tasks/{task_id}{suffix}"),
            None,
            Some(other_agent),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(read_json(response).await["detail"], "Task not found");
    }
    assert_ne!(owner_agent, other_agent);
    assert_ne!(owner_chat, other_chat);
}

#[tokio::test]
async fn tool_task_preserves_real_name_and_arguments_for_tool_guard() {
    let test = build_app();
    create_agent(&test.app, "task-guard-agent", "Task Guard Agent").await;
    enable_agent_tool(&test.state, "task-guard-agent", "bash").await;
    let guarded_command = "printf task-guard-fixture";
    let rule = request(
        &test.app,
        Method::POST,
        "/api/agents/task-guard-agent/permission-rules",
        Some(serde_json::json!({
            "effect": "deny",
            "enabled": true,
            "label": "Block exact Task Tool call",
            "matcher": {
                "type": "tool_call",
                "tool_name": "bash",
                "input_params": guarded_command
            }
        })),
        Some("task-guard-agent"),
    )
    .await;
    assert_eq!(rule.status(), StatusCode::CREATED);
    let chat = seed_materialized_chat(
        &test,
        "/api/agents/task-guard-agent/chats",
        Some("task-guard-agent"),
        "task-guard-chat",
    )
    .await;
    let chat_id = chat["id"].as_str().expect("chat id").to_string();
    let owner = TaskOwner::new("task-guard-agent", &chat_id);
    let workspace_dir = test
        .state
        .rua_root
        .join("workspaces")
        .join("task-guard-agent");
    let configuration_epoch = chat_configuration_epoch(&workspace_dir, &chat_id).await;
    let created = TaskCreateTool
        .call(
            serde_json::json!({
                "instruction": "Exercise the guarded Tool call",
                "runner": {
                    "type": "tool",
                    "tool_name": "bash",
                    "arguments": {
                        "command": guarded_command
                    }
                }
            }),
            ToolContext {
                agent_id: "task-guard-agent".to_string(),
                session_id: chat_id,
                working_dir: workspace_dir,
                metadata: std::collections::HashMap::from([(
                    "chat_configuration_epoch".to_string(),
                    serde_json::Value::String(configuration_epoch),
                )]),
                task_runtime: Some(Arc::new(TaskRuntimeHandle::new(Arc::clone(
                    &test.state.task_runtime,
                )))),
                ..Default::default()
            },
        )
        .await
        .expect("task_create");
    let created: serde_json::Value =
        serde_json::from_str(&created.text_content()).expect("task_create JSON result");
    let task_id = created["task_id"].as_str().expect("task id");
    let record = wait_for_terminal(&test.state, &owner, task_id).await;

    assert_eq!(record.state, TaskState::Failed);
    let error = record
        .result
        .and_then(|result| result.error)
        .expect("Tool Guard failure");
    assert!(error.message.contains("Block exact Task Tool call"));
    assert!(error.message.contains("bash"));
}

struct CancellableRunner {
    started: tokio::sync::Notify,
}

#[async_trait]
impl TaskRunner for CancellableRunner {
    fn kind(&self) -> TaskRunnerKind {
        TaskRunnerKind::Tool
    }

    async fn run(&self, request: TaskRunRequest) -> Result<TaskRunOutput, TaskRunnerError> {
        self.started.notify_one();
        request.cancel.cancelled().await;
        tokio::time::sleep(Duration::from_millis(20)).await;
        Ok(TaskRunOutput::completed("cleanup complete"))
    }
}

#[tokio::test]
async fn task_creation_is_rejected_when_the_local_device_is_not_chat_authority() {
    let test = build_app();
    create_agent(
        &test.app,
        "remote-authority-agent",
        "Remote Authority Agent",
    )
    .await;
    let chat = seed_materialized_chat(
        &test,
        "/api/agents/remote-authority-agent/chats",
        Some("remote-authority-agent"),
        "remote-authority-chat",
    )
    .await;
    let chat_id = chat["id"].as_str().unwrap().to_string();
    let workspace_dir = test
        .state
        .registry
        .get_or_create("remote-authority-agent")
        .await
        .unwrap()
        .workspace_dir();
    rua_server::services::chat_metadata_store::update_authority_placement(
        Some(&test.state),
        &workspace_dir,
        &rua_core::ChatAddress::agent("remote-authority-agent", &chat_id),
        rua_server::services::conversation_authority_placement::ConversationAuthorityPlacement {
            device_id: "remote-device".to_string(),
            authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(2, 0, [0; 8]),
            base_manifest_hash: format!("sha256:{}", "2".repeat(64)),
        },
    )
    .await
    .unwrap();

    let error = TaskCreateTool
        .call(
            serde_json::json!({
                "runner": {
                    "type": "tool",
                    "tool_name": "time_now",
                    "arguments": {}
                }
            }),
            ToolContext {
                agent_id: "remote-authority-agent".to_string(),
                session_id: chat_id.clone(),
                task_runtime: Some(Arc::new(TaskRuntimeHandle::new(Arc::clone(
                    &test.state.task_runtime,
                )))),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("Chat authority is Device 'remote-device'"));
    assert!(test
        .state
        .task_runtime
        .list(
            &TaskOwner::new("remote-authority-agent", chat_id),
            rua_task::TaskListFilter::default(),
        )
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn remote_task_runtime_note_remains_readable_without_local_task_details() {
    let test = build_app();
    create_agent(&test.app, "remote-note-agent", "Remote Note Agent").await;
    let chat = seed_materialized_chat(
        &test,
        "/api/agents/remote-note-agent/chats",
        Some("remote-note-agent"),
        "remote-note-chat",
    )
    .await;
    let chat_id = chat["id"].as_str().unwrap().to_string();
    let workspace_dir = test
        .state
        .registry
        .get_or_create("remote-note-agent")
        .await
        .unwrap()
        .workspace_dir();
    let task_id = "remote-task-1";
    let store = WorkspaceSessionStore::new(&workspace_dir);
    store
        .append_conversation_turn(
            &chat_id,
            NewConversationTurn {
                turn_id: "remote-turn-1".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: chrono::Utc::now().to_rfc3339(),
                    assistant_started_at: Some(chrono::Utc::now().to_rfc3339()),
                    assistant_completed_at: Some(chrono::Utc::now().to_rfc3339()),
                    status: "completed".to_string(),
                },
                selected_variant_id: Some("remote-variant-1".to_string()),
                messages: vec![NewConversationMessage {
                    message_id: "remote-user-1".to_string(),
                    role: "user".to_string(),
                    created_at: chrono::Utc::now().to_rfc3339(),
                    status: "completed".to_string(),
                    content: serde_json::json!([{"type": "text", "text": "inspect remotely"}]),
                    provider: None,
                    tool: None,
                    metadata: serde_json::Map::new(),
                }],
            },
        )
        .await
        .unwrap();
    store
        .append_runtime_note(
            &chat_id,
            "remote-turn-1",
            RuntimeNote {
                runtime_note_id: "task:remote-task-1:attempt:1".to_string(),
                created_at: chrono::Utc::now().to_rfc3339(),
                payload: RuntimeNotePayload::TaskAttemptTerminal(
                    TaskAttemptTerminalRuntimeNote {
                        task_id: task_id.to_string(),
                        attempt_no: 1,
                        terminal_status: "completed".to_string(),
                        output: "Outcome\nRemote Task completed.\n\nKey Results\nBounded output is synchronized.\n\nVerification\nRuntime Note is readable.\n\nRemaining Work\nNone".to_string(),
                        runner_name: "Remote_Inspect".to_string(),
                        tool_call_id: Some("remote-tool-call".to_string()),
                        artifacts: serde_json::json!([{
                    "id": "remote-artifact",
                    "name": "handoff.md",
                    "media_type": "text/markdown",
                    "resource": {
                        "uri": "task://remote-task-1/attempts/1/handoff/handoff.md",
                        "ref_type": "task_handoff",
                        "source_device_id": "remote-device"
                    }
                        }]),
                        device: serde_json::json!({
                    "device_id": "remote-device",
                    "device_name": "Remote Desktop",
                    "host_profile": "desktop",
                    "authority_epoch": "0000000000000004000000000000000000000000"
                        }),
                        full_result_ref: Some(serde_json::json!({
                    "uri": "task://remote-task-1/attempts/1/raw-output.json",
                    "ref_type": "task_raw_output",
                    "source_device_id": "remote-device"
                        })),
                    },
                ),
            },
        )
        .await
        .unwrap();

    let history = request(
        &test.app,
        Method::GET,
        &format!("/api/chats/{chat_id}"),
        None,
        Some("remote-note-agent"),
    )
    .await;
    assert_eq!(history.status(), StatusCode::OK);
    let history = read_json(history).await;
    let note = history["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["type"] == "runtime_note")
        .expect("Runtime Note message");
    assert_eq!(note["runtime_note"]["kind"], "task_attempt_terminal");
    assert_eq!(note["runtime_note"]["payload"]["task_id"], task_id);
    assert!(note["runtime_note"]["payload"]["output"]
        .as_str()
        .unwrap()
        .contains("Bounded output is synchronized"));
    assert_eq!(
        note["runtime_note"]["payload"]["device"]["device_id"],
        "remote-device"
    );
    assert_eq!(
        note["runtime_note"]["payload"]["full_result_ref"]["source_device_id"],
        "remote-device"
    );

    let details = request(
        &test.app,
        Method::GET,
        &format!("/api/agents/remote-note-agent/chats/{chat_id}/tasks/{task_id}"),
        None,
        Some("remote-note-agent"),
    )
    .await;
    assert_eq!(details.status(), StatusCode::NOT_FOUND);
    assert_eq!(read_json(details).await["detail"], "Task not found");
}

#[tokio::test]
async fn authority_transfer_interrupts_tasks_and_commits_the_runtime_note_before_handoff() {
    let test = build_app();
    create_agent(&test.app, "transfer-task-agent", "Transfer Task Agent").await;
    let chat = seed_materialized_chat(
        &test,
        "/api/agents/transfer-task-agent/chats",
        Some("transfer-task-agent"),
        "transfer-task-chat",
    )
    .await;
    let chat_id = chat["id"].as_str().unwrap().to_string();
    let owner = TaskOwner::new("transfer-task-agent", &chat_id);
    let workspace_dir = test
        .state
        .registry
        .get_or_create("transfer-task-agent")
        .await
        .unwrap()
        .workspace_dir();
    let store = WorkspaceSessionStore::new(&workspace_dir);
    seed_runtime_note_turn(&store, &chat_id, "turn-transfer-task").await;
    let runner = Arc::new(CancellableRunner {
        started: tokio::sync::Notify::new(),
    });
    test.state.task_runtime.register_runner(runner.clone());
    let created = TaskCreateTool
        .call(
            serde_json::json!({
                "runner": {
                    "type": "tool",
                    "tool_name": "Fixture_Block",
                    "arguments": {}
                }
            }),
            ToolContext {
                agent_id: "transfer-task-agent".to_string(),
                session_id: chat_id.clone(),
                metadata: std::collections::HashMap::from([(
                    "turn_id".to_string(),
                    serde_json::Value::String("turn-transfer-task".to_string()),
                )]),
                task_runtime: Some(Arc::new(TaskRuntimeHandle::new(Arc::clone(
                    &test.state.task_runtime,
                )))),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let created: serde_json::Value = serde_json::from_str(&created.text_content()).unwrap();
    let task_id = created["task_id"].as_str().unwrap().to_string();
    tokio::time::timeout(Duration::from_secs(2), runner.started.notified())
        .await
        .unwrap();

    test.state
        .conversation_authority
        .switch_authority(
            Some(&test.state),
            &workspace_dir,
            &rua_core::ChatAddress::agent("transfer-task-agent", &chat_id),
            rua_server::services::conversation_authority_placement::ConversationAuthorityPlacement {
                device_id: "next-authority-device".to_string(),
                authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(2, 0, [0; 8]),
                base_manifest_hash: format!("sha256:{}", "3".repeat(64)),
            },
        )
        .await
        .unwrap();

    let interrupted = test.state.task_runtime.get(&owner, &task_id).await.unwrap();
    assert_eq!(interrupted.state, TaskState::Interrupted);
    assert_eq!(
        interrupted.result.as_ref().unwrap().device.authority_epoch,
        interrupted.attempts[0].device.authority_epoch
    );
    let notes = wait_for_task_runtime_notes(&store, &chat_id, &task_id, 1).await;
    let note = task_attempt_note(&notes[0]);
    assert_eq!(note.terminal_status, "interrupted");
    assert_eq!(
        note.device["authority_epoch"].as_str(),
        interrupted.attempts[0].device.authority_epoch.as_deref()
    );
}

#[tokio::test]
async fn task_activity_reconnects_with_snapshot_and_chat_delete_removes_tasks() {
    let test = build_app();
    create_agent(&test.app, "task-lifecycle-agent", "Task Lifecycle Agent").await;
    let chat = seed_materialized_chat(
        &test,
        "/api/agents/task-lifecycle-agent/chats",
        Some("task-lifecycle-agent"),
        "task-lifecycle-chat",
    )
    .await;
    let chat_id = chat["id"].as_str().expect("chat id").to_string();
    let runner = Arc::new(CancellableRunner {
        started: tokio::sync::Notify::new(),
    });
    test.state.task_runtime.register_runner(runner.clone());

    let created = TaskCreateTool
        .call(
            serde_json::json!({
                "runner": {
                    "type": "tool",
                    "tool_name": "Fixture_Block",
                    "arguments": {}
                }
            }),
            ToolContext {
                agent_id: "task-lifecycle-agent".to_string(),
                session_id: chat_id.clone(),
                task_runtime: Some(Arc::new(TaskRuntimeHandle::new(Arc::clone(
                    &test.state.task_runtime,
                )))),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let value: serde_json::Value = serde_json::from_str(&created.text_content()).unwrap();
    let task_id = value["task_id"].as_str().unwrap().to_string();
    tokio::time::timeout(Duration::from_secs(2), runner.started.notified())
        .await
        .unwrap();

    let activity = request(
        &test.app,
        Method::GET,
        &format!("/api/agents/task-lifecycle-agent/chats/{chat_id}/tasks/activity"),
        None,
        Some("task-lifecycle-agent"),
    )
    .await;
    assert_eq!(activity.status(), StatusCode::OK);
    let mut stream = activity.into_body().into_data_stream();
    let first = tokio::time::timeout(Duration::from_secs(2), stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let first = String::from_utf8(first.to_vec()).unwrap();
    assert!(first.contains("event: snapshot"));
    assert!(first.contains(&task_id));
    drop(stream);
    assert!(!test
        .state
        .task_runtime
        .get(&TaskOwner::new("task-lifecycle-agent", &chat_id), &task_id)
        .await
        .unwrap()
        .state
        .is_terminal());

    let cancel = request(
        &test.app,
        Method::POST,
        &format!("/api/agents/task-lifecycle-agent/chats/{chat_id}/tasks/{task_id}/cancel"),
        None,
        Some("task-lifecycle-agent"),
    )
    .await;
    assert_eq!(cancel.status(), StatusCode::OK);
    assert_eq!(read_json(cancel).await["state"], "cancelled");
    let reconnected = request(
        &test.app,
        Method::GET,
        &format!("/api/agents/task-lifecycle-agent/chats/{chat_id}/tasks/activity"),
        None,
        Some("task-lifecycle-agent"),
    )
    .await;
    assert_eq!(reconnected.status(), StatusCode::OK);
    let mut reconnected_stream = reconnected.into_body().into_data_stream();
    let snapshot = tokio::time::timeout(Duration::from_secs(2), reconnected_stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let snapshot = String::from_utf8(snapshot.to_vec()).unwrap();
    assert!(snapshot.contains("event: snapshot"));
    assert!(snapshot.contains(&task_id));
    assert!(snapshot.contains("cancelled"));
    drop(reconnected_stream);

    let second = TaskCreateTool
        .call(
            serde_json::json!({
                "runner": {
                    "type": "tool",
                    "tool_name": "Fixture_Block",
                    "arguments": {}
                }
            }),
            ToolContext {
                agent_id: "task-lifecycle-agent".to_string(),
                session_id: chat_id.clone(),
                task_runtime: Some(Arc::new(TaskRuntimeHandle::new(Arc::clone(
                    &test.state.task_runtime,
                )))),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let second: serde_json::Value = serde_json::from_str(&second.text_content()).unwrap();
    let second_id = second["task_id"].as_str().unwrap().to_string();
    tokio::time::timeout(Duration::from_secs(2), runner.started.notified())
        .await
        .unwrap();

    let delete = request(
        &test.app,
        Method::DELETE,
        &format!("/api/agents/task-lifecycle-agent/chats/{chat_id}"),
        None,
        Some("task-lifecycle-agent"),
    )
    .await;
    assert_eq!(delete.status(), StatusCode::OK);

    let missing = request(
        &test.app,
        Method::GET,
        &format!("/api/agents/task-lifecycle-agent/chats/{chat_id}/tasks/{second_id}"),
        None,
        Some("task-lifecycle-agent"),
    )
    .await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
}

struct FailThenSucceedTaskRunner {
    calls: std::sync::atomic::AtomicUsize,
}

#[async_trait]
impl TaskRunner for FailThenSucceedTaskRunner {
    fn kind(&self) -> TaskRunnerKind {
        TaskRunnerKind::Tool
    }

    async fn run(&self, _request: TaskRunRequest) -> Result<TaskRunOutput, TaskRunnerError> {
        let call = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if call == 0 {
            Ok(TaskRunOutput::failed(
                "Outcome\nfailed\n\nKey Results\nfirst failure\n\nVerification\nfailed\n\nRemaining Work\nRetry",
                "first failure",
            ))
        } else {
            Ok(TaskRunOutput::completed(
                "Outcome\ncompleted\n\nKey Results\nretry success\n\nVerification\npassed\n\nRemaining Work\nNone",
            ))
        }
    }
}

#[tokio::test]
async fn task_retry_http_preserves_attempt_history_under_same_id() {
    let test = build_app();
    create_agent(&test.app, "task-retry-agent", "Task Retry Agent").await;
    let chat = seed_materialized_chat(
        &test,
        "/api/agents/task-retry-agent/chats",
        Some("task-retry-agent"),
        "task-retry-chat",
    )
    .await;
    let chat_id = chat["id"].as_str().unwrap().to_string();
    let owner = TaskOwner::new("task-retry-agent", &chat_id);
    let session_store = WorkspaceSessionStore::new(
        test.state
            .rua_root
            .join("workspaces")
            .join("task-retry-agent"),
    );
    seed_runtime_note_turn(&session_store, &chat_id, "turn-task-retry").await;
    test.state
        .task_runtime
        .register_runner(Arc::new(FailThenSucceedTaskRunner {
            calls: std::sync::atomic::AtomicUsize::new(0),
        }));

    let created = TaskCreateTool
        .call(
            serde_json::json!({
                "runner": {
                    "type": "tool",
                    "tool_name": "Fixture_Retry",
                    "arguments": {}
                }
            }),
            ToolContext {
                agent_id: "task-retry-agent".to_string(),
                session_id: chat_id.clone(),
                metadata: std::collections::HashMap::from([(
                    "turn_id".to_string(),
                    serde_json::Value::String("turn-task-retry".to_string()),
                )]),
                task_runtime: Some(Arc::new(TaskRuntimeHandle::new(Arc::clone(
                    &test.state.task_runtime,
                )))),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let created: serde_json::Value = serde_json::from_str(&created.text_content()).unwrap();
    let task_id = created["task_id"].as_str().unwrap().to_string();
    assert_eq!(
        wait_for_terminal(&test.state, &owner, &task_id).await.state,
        TaskState::Failed
    );

    let retry = request(
        &test.app,
        Method::POST,
        &format!("/api/agents/task-retry-agent/chats/{chat_id}/tasks/{task_id}/retry"),
        None,
        Some("task-retry-agent"),
    )
    .await;
    assert_eq!(retry.status(), StatusCode::OK);
    let retry = read_json(retry).await;
    assert_eq!(retry["task_id"], task_id);
    assert_eq!(retry["attempt_no"], 2);

    let completed = wait_for_terminal(&test.state, &owner, &task_id).await;
    assert_eq!(completed.state, TaskState::Completed);
    assert_eq!(completed.attempts.len(), 2);
    assert_eq!(completed.attempts[0].state, TaskState::Failed);
    assert_eq!(completed.attempts[1].state, TaskState::Completed);

    let mut notes = wait_for_task_runtime_notes(&session_store, &chat_id, &task_id, 2).await;
    notes.sort_by_key(|note| task_attempt_note(note).attempt_no);
    assert_eq!(
        notes[0].runtime_note_id,
        format!("task:{task_id}:attempt:1")
    );
    assert_eq!(task_attempt_note(&notes[0]).attempt_no, 1);
    assert_eq!(task_attempt_note(&notes[0]).terminal_status, "failed");
    assert_eq!(
        notes[1].runtime_note_id,
        format!("task:{task_id}:attempt:2")
    );
    assert_eq!(task_attempt_note(&notes[1]).attempt_no, 2);
    assert_eq!(task_attempt_note(&notes[1]).terminal_status, "completed");
}

#[tokio::test]
async fn task_http_exposes_derived_blocked_dependency_state() {
    let test = build_app();
    create_agent(&test.app, "task-graph-agent", "Task Graph Agent").await;
    let chat = seed_materialized_chat(
        &test,
        "/api/agents/task-graph-agent/chats",
        Some("task-graph-agent"),
        "task-graph-chat",
    )
    .await;
    let chat_id = chat["id"].as_str().unwrap().to_string();
    let owner = TaskOwner::new("task-graph-agent", &chat_id);
    test.state
        .task_runtime
        .register_runner(Arc::new(FailThenSucceedTaskRunner {
            calls: std::sync::atomic::AtomicUsize::new(0),
        }));
    let ctx = ToolContext {
        agent_id: "task-graph-agent".to_string(),
        session_id: chat_id.clone(),
        task_runtime: Some(Arc::new(TaskRuntimeHandle::new(Arc::clone(
            &test.state.task_runtime,
        )))),
        ..Default::default()
    };

    let upstream = TaskCreateTool
        .call(
            serde_json::json!({
                "instruction": "fail upstream",
                "runner": {
                    "type": "tool",
                    "tool_name": "Fixture_GraphUpstream",
                    "arguments": {}
                }
            }),
            ctx.clone(),
        )
        .await
        .unwrap();
    let upstream: serde_json::Value = serde_json::from_str(&upstream.text_content()).unwrap();
    let upstream_id = upstream["task_id"].as_str().unwrap().to_string();
    assert_eq!(
        wait_for_terminal(&test.state, &owner, &upstream_id)
            .await
            .state,
        TaskState::Failed
    );

    let downstream = TaskCreateTool
        .call(
            serde_json::json!({
                "instruction": "wait for upstream",
                "depends_on": [upstream_id],
                "runner": {
                    "type": "tool",
                    "tool_name": "Fixture_GraphDownstream",
                    "arguments": {}
                }
            }),
            ctx,
        )
        .await
        .unwrap();
    let downstream: serde_json::Value = serde_json::from_str(&downstream.text_content()).unwrap();
    let downstream_id = downstream["task_id"].as_str().unwrap().to_string();

    let get = request(
        &test.app,
        Method::GET,
        &format!("/api/agents/task-graph-agent/chats/{chat_id}/tasks/{downstream_id}"),
        None,
        Some("task-graph-agent"),
    )
    .await;
    assert_eq!(get.status(), StatusCode::OK);
    let body = read_json(get).await;
    assert_eq!(body["state"], "blocked");
    assert_eq!(body["spec"]["depends_on"][0], upstream_id);
    assert_eq!(body["blocked_by"][0], upstream_id);
    assert!(body["waiting_on"].as_array().unwrap().is_empty());

    let list = request(
        &test.app,
        Method::GET,
        &format!("/api/agents/task-graph-agent/chats/{chat_id}/tasks?state=blocked"),
        None,
        Some("task-graph-agent"),
    )
    .await;
    assert_eq!(list.status(), StatusCode::OK);
    let body = read_json(list).await;
    assert_eq!(body.as_array().unwrap().len(), 1);
    assert_eq!(body[0]["task_id"], downstream_id);
}
