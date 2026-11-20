use axum::http::{Method, StatusCode};
use serde_json::json;

use super::support::{build_app, create_agent, delete_agent, read_json, request};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_missing_job_returns_fastapi_detail_404() {
    let app = build_app();

    let resp = request(
        &app.app,
        Method::POST,
        "/api/cron/jobs/missing-cron-job/run",
        None,
        Some("default"),
    )
    .await;

    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    assert!(read_json(resp).await["detail"]
        .as_str()
        .expect("detail")
        .contains("missing-cron-job"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_text_job_returns_started_without_waiting_for_completion() {
    let app = build_app();
    let agent_id = "integ_cron_run_contract";
    create_agent(&app.app, agent_id, "Cron run contract").await;

    let create = request(
        &app.app,
        Method::POST,
        "/api/cron/jobs",
        Some(json!({
            "name": "contract cron manual run",
            "enabled": true,
            "schedule": {
                "type": "cron",
                "cron": "0 0 * * *",
                "timezone": "UTC"
            },
            "task_type": "text",
            "text": "manual cron text",
            "dispatch": {
                "type": "channel",
                "channel": "console",
                "target": {
                    "user_id": "cron-run-contract-user",
                    "session_id": "console:cron-run-contract-session"
                },
                "mode": "stream"
            }
        })),
        Some(agent_id),
    )
    .await;
    assert_eq!(create.status(), StatusCode::CREATED);
    let job_id = read_json(create).await["id"]
        .as_str()
        .expect("job id")
        .to_owned();

    let run = request(
        &app.app,
        Method::POST,
        &format!("/api/cron/jobs/{job_id}/run"),
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(run.status(), StatusCode::OK);
    assert_eq!(read_json(run).await, json!({ "started": true }));

    let delete = request(
        &app.app,
        Method::DELETE,
        &format!("/api/cron/jobs/{job_id}"),
        None,
        Some(agent_id),
    )
    .await;
    assert_eq!(delete.status(), StatusCode::OK);
    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cron_bound_agent_heartbeat_continue_selected_chat_without_changing_dispatch() {
    use axum::Router;
    use serde_json::Value;
    use std::time::Duration;
    use wiremock::{matchers::method, Mock, MockServer, ResponseTemplate};

    async fn call(router: &Router, method: Method, path: &str, body: Option<Value>) -> Value {
        let response = request(router, method, path, body, None).await;
        let status = response.status();
        let value = read_json(response).await;
        assert!(status.is_success(), "{path}: {status} {value}");
        value
    }
    async fn wait_for_history(router: &Router, path: &str, needle: &str) -> Value {
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let history = call(router, Method::GET, path, None).await;
                if history["messages"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|m| m.to_string().contains(needle))
                {
                    break history;
                }
                tokio::time::sleep(Duration::from_millis(30)).await;
            }
        })
        .await
        .expect("scheduled input in saved Chat history")
    }

    async fn wait_for_runs(app: &Router, job_path: &str, count: usize) -> Value {
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let reports = call(app, Method::GET, &format!("{job_path}/history"), None).await;
                if reports.as_array().unwrap().len() == count {
                    break reports;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("Cron terminal dispatch")
    }

    let test = build_app();
    let app = rua_server::router::build_app(test.state.clone(), None);
    let model = MockServer::start().await;
    let body = [
        json!({"content":"fixture reply","role":"assistant"}),
        json!({}),
    ].iter().enumerate().map(|(index, delta)| {
        let frame = json!({"id":"fixture", "model":"photo-model", "object":"chat.completion.chunk",
            "created": 1, "choices":[{"index":0,"delta":delta,"finish_reason":if index == 0 {None} else {Some("stop")}}]});
        format!("data: {frame}\n\n")
    }).collect::<String>() + "data: [DONE]\n\n";
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(body),
        )
        .mount(&model)
        .await;
    call(&app, Method::POST, "/api/models/custom-providers", Some(json!({
        "id":"cronfixture","name":"Isolated Cron fixture","api_key":"test", "base_url":format!("{}/v1", model.uri()),
        "chat_model":"OpenAIChatModel","models":[{"id":"photo-model","name":"Photo model","context_window":65536}]
    }))).await;
    call(
        &app,
        Method::PUT,
        "/api/models/providers/active",
        Some(json!({"scope":"global","provider_id":"cronfixture","model":"photo-model"})),
    )
    .await;
    let root = test.state.rua_root.join("photos");
    std::fs::create_dir(&root).unwrap();
    let device = hex::encode(
        test.state
            .sync_device
            .read()
            .unwrap()
            .as_ref()
            .unwrap()
            .device_id_short(),
    );
    let project = call(
        &app,
        Method::POST,
        "/api/projects",
        Some(json!({
            "resource_device_id":device, "resource_root":root,"display_name":"Photos"
        })),
    )
    .await;
    let id = "00000000-0000-4000-8000-000000000650";
    let path = format!("/api/chats/{id}");
    call(&app, Method::POST, &format!("{path}/console/submit"), Some(json!({
        "message":"remember these photographs", "project_id":project["project"]["project_id"], "agent_id":"default"
    }))).await;
    wait_for_history(&app, &path, "fixture reply").await;
    let target = json!({"user_id": "original-recipient","session_id":"platform-thread","conversation_id":"platform-room","chat_id":id});
    let job = call(&app, Method::POST, "/api/agents/default/cron/jobs", Some(json!({
        "name":"Fixture follow-up","enabled":false,"schedule":{"type":"cron","cron":"0 8 * * *"},
        "task_type":"agent","request":{"input":"cron follow-up"},
        "dispatch":{"channel":"console","mode":"final","target":target},"runtime":{"share_session":true}
    }))).await;
    assert_eq!(job["dispatch"]["target"], target);
    let job_path = format!(
        "/api/agents/default/cron/jobs/{}",
        job["id"].as_str().unwrap()
    );
    create_agent(&app, "cron-photo", "Photo agent").await;
    call(
        &app,
        Method::PATCH,
        &format!("{path}/agent-selection"),
        Some(json!({"agent_id":"cron-photo","expected_agent_id":"default"})),
    )
    .await;
    call(&app, Method::POST, &format!("{job_path}/run"), None).await;
    let reports = wait_for_runs(&app, &job_path, 1).await;
    assert_eq!(reports[0]["success"], true, "{reports}");
    assert_eq!(reports[0]["session_id"], "platform-thread");
    assert_eq!(reports[0]["conversation_id"], "platform-room");
    assert_eq!(reports[0]["chat_id"], id);
    assert!(test
        .state
        .push_message_store
        .list()
        .await
        .iter()
        .any(|message| {
            message.extra["chat_id"] == id
                && message.extra["session_id"] == "platform-thread"
                && message.extra["conversation_id"] == "platform-room"
                && message.content == "fixture reply"
        }));
    let history = wait_for_history(&app, &path, "cron follow-up").await;
    assert_eq!(history["agent_id"], "cron-photo");
    let turns = rua_session::WorkspaceSessionStore::new(test.state.chat_workspace().as_path())
        .read_turn_range(id, 1, 20)
        .await
        .unwrap();
    assert_eq!(turns.len(), 2);
    assert_eq!(
        turns[0].metadata["turn_agent_assignment"]["agent_id"],
        "default"
    );
    assert_eq!(
        turns[1].metadata["turn_agent_assignment"]["agent_id"],
        "cron-photo"
    );
    let upstream = model.received_requests().await.unwrap();
    assert!(
        upstream.iter().any(|r| {
            let body = String::from_utf8_lossy(&r.body);
            body.contains("remember these photographs") && body.contains("cron follow-up")
        }),
        "model saw saved history plus the scheduled input"
    );

    let workspace = test
        .state
        .registry
        .get_or_create("default")
        .await
        .unwrap()
        .workspace_dir();
    std::fs::write(workspace.join("HEARTBEAT.md"), "heartbeat follow-up").unwrap();
    test.state
        .agent_cells
        .section::<Value>("default", "last_dispatch")
        .write(json!({
            "channel":"console", "user_id":"original-recipient","session_id":"platform-thread",
            "conversation_id":"platform-room","chat_id":id
        }))
        .await
        .unwrap();
    call(
        &app,
        Method::PUT,
        "/api/agents/default/config/heartbeat",
        Some(json!({"enabled":false,"every":"6h","target":"last"})),
    )
    .await;
    call(
        &app,
        Method::POST,
        "/api/agents/default/config/heartbeat/run",
        None,
    )
    .await;
    let history = wait_for_history(&app, &path, "heartbeat follow-up").await;
    assert_eq!(history["agent_id"], "cron-photo");
    assert!(history["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m.to_string().contains("remember these photographs")));
    // A dedicated run never submits to the saved Chat, even if the delivery target keeps its link.
    call(
        &app,
        Method::PUT,
        &job_path,
        Some(json!({"runtime":{"share_session":false}})),
    )
    .await;
    call(
        &app,
        Method::POST,
        &format!("{job_path}/run"),
        Some(json!({"message":"isolated control"})),
    )
    .await;
    let reports = wait_for_runs(&app, &job_path, 2).await;
    assert_eq!(reports[1]["success"], true, "{reports}");
    let history = call(&app, Method::GET, &path, None).await;
    assert!(!history["messages"].to_string().contains("isolated control"));
    let requests = model.received_requests().await.unwrap();
    assert!(
        requests.iter().any(|req| {
            let body = String::from_utf8_lossy(&req.body);
            body.contains("isolated control") && !body.contains("remember these photographs")
        }),
        "the isolated run reached the model without the saved Chat history"
    );
    call(
        &app,
        Method::PUT,
        &job_path,
        Some(json!({"runtime":{"share_session":true}})),
    )
    .await;
    let address = rua_core::ChatAddress::new(id);
    let authority = &test.state.conversation_authority;
    tokio::time::timeout(Duration::from_secs(30), async {
        while authority.active_turn(&address).await.is_some()
            || !authority.queue_snapshot(&address).await.is_empty()
        {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("heartbeat turn and any queued work finish before ordinary deletion");
    let deleted = call(&app, Method::DELETE, &path, None).await;
    assert_eq!(deleted["deleted"], true);
    assert_eq!(deleted["was_busy"], false);
    call(&app, Method::POST, &format!("{job_path}/run"), None).await;
    let reports = wait_for_runs(&app, &job_path, 3).await;
    assert_eq!(reports[2]["success"], false, "{reports}");
    assert!(reports[2]["error"]
        .as_str()
        .unwrap()
        .contains("Scheduled Chat not found"));
    let heartbeat = request(
        &app,
        Method::POST,
        "/api/agents/default/config/heartbeat/run",
        None,
        None,
    )
    .await;
    assert_eq!(heartbeat.status(), StatusCode::NOT_FOUND);
    assert!(
        rua_session::WorkspaceSessionStore::new(test.state.chat_workspace().as_path())
            .load_chat_header(id)
            .await
            .unwrap()
            .is_none()
    );
}
