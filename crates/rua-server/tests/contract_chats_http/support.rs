use axum::body::Body;
use axum::extract::Request;
use axum::http::Method;
use axum::response::Response;
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tempfile::TempDir;
use tower::ServiceExt;

use rua_server::endpoints::{
    acp_chat, agents, approvals, chat_search, chats, gap_fixes, sync, tasks,
};
use rua_server::middleware::agent_context_middleware;
use rua_server::{AppState, ServerProfile};

pub struct TestApp {
    pub app: Router,
    pub state: AppState,
    _tmp: TempDir,
}

pub fn build_app() -> TestApp {
    let tmp = TempDir::new().expect("tempdir");
    let state = AppState::from_tempdir(tmp.path().to_path_buf(), ServerProfile::Test);
    let api = Router::new()
        .merge(agents::router())
        .merge(approvals::router())
        .merge(chats::root_router())
        .merge(chats::agent_scoped_router())
        .merge(gap_fixes::agent_config_router())
        .merge(chat_search::root_router())
        .merge(chat_search::agent_scoped_router())
        .merge(acp_chat::router())
        .merge(tasks::router())
        .merge(sync::router());
    let app = Router::new()
        .nest("/api", api)
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            agent_context_middleware,
        ))
        .with_state(state.clone());

    TestApp {
        app,
        state,
        _tmp: tmp,
    }
}

pub async fn request(
    app: &Router,
    method: Method,
    uri: &str,
    body: Option<Value>,
    agent_id: Option<&str>,
) -> Response {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(agent_id) = agent_id {
        builder = builder.header("X-Agent-Id", agent_id);
    }
    let body = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            Body::from(serde_json::to_vec(&value).expect("request json"))
        }
        None => Body::empty(),
    };

    app.clone()
        .oneshot(builder.body(body).expect("request"))
        .await
        .expect("oneshot")
}

pub async fn read_json(resp: Response) -> Value {
    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    if bytes.is_empty() {
        return Value::Null;
    }
    serde_json::from_slice(&bytes).expect("response json")
}

pub async fn create_agent(app: &Router, id: &str, name: &str) {
    let resp = request(
        app,
        Method::POST,
        "/api/agents",
        Some(json!({
            "id": id,
            "name": name,
            "description": ""
        })),
        Some("default"),
    )
    .await;
    assert_eq!(resp.status(), axum::http::StatusCode::CREATED);
}

pub async fn delete_agent(app: &Router, id: &str) {
    let resp = request(
        app,
        Method::DELETE,
        &format!("/api/agents/{id}"),
        None,
        Some("default"),
    )
    .await;
    let status = resp.status();
    let body = read_json(resp).await;
    assert_eq!(
        status,
        axum::http::StatusCode::OK,
        "Agent cleanup failed: {body}"
    );
}

pub async fn seed_materialized_chat(
    test: &TestApp,
    uri: &str,
    agent_id: Option<&str>,
    name: &str,
) -> Value {
    let resp = request(
        &test.app,
        Method::POST,
        uri,
        Some(json!({
            "name": name,
            "user_id": format!("user-{name}"),
            "channel": "console",
            "meta": {}
        })),
        agent_id,
    )
    .await;
    assert_eq!(resp.status(), axum::http::StatusCode::OK);
    let mut draft = read_json(resp).await;
    let id = draft["id"].as_str().unwrap();
    let agent_id = agent_id
        .or_else(|| {
            uri.strip_prefix("/api/agents/")
                .and_then(|tail| tail.split('/').next())
        })
        .unwrap_or("default");
    let runtime = test.state.registry.get_or_create(agent_id).await.unwrap();
    let device =
        rua_server::services::device_context::current_sync_device_id(&test.state.sync_device)
            .unwrap();
    let address = rua_core::ChatAddress::agent(agent_id, id);
    let placement = rua_server::services::conversation_authority_placement::ConversationAuthorityPlacement {
        device_id: device.clone(), authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::next_for_device(&device, None),
        base_manifest_hash: rua_server::services::conversation_manifest::initial_conversation_manifest_hash(&address).unwrap(),
    };
    rua_server::services::console_chat::materialize_targeted_chat(
        &runtime.workspace_dir(),
        agent_id,
        id,
        name,
        &placement,
    )
    .await
    .unwrap();
    let store = rua_session::WorkspaceSessionStore::new(runtime.workspace_dir());
    let mut header = store.load_chat_header(id).await.unwrap().unwrap();
    let agent = test
        .state
        .agent_service
        .get_agent(agent_id)
        .await
        .unwrap()
        .unwrap();
    let running: rua_config::AgentsRunningConfig = agent
        .running
        .clone()
        .map(serde_json::from_value)
        .transpose()
        .unwrap()
        .unwrap_or_default();
    let mut index = rua_session::chat_index::get(&runtime.workspace_dir(), id)
        .await
        .unwrap()
        .unwrap();
    index.user_id = format!("user-{name}");
    rua_session::chat_index::upsert(&runtime.workspace_dir(), index)
        .await
        .unwrap();
    let configs: serde_json::Map<String, Value> = agent
        .tools
        .as_ref()
        .and_then(|tools| tools.get("builtin_tools"))
        .and_then(Value::as_object)
        .map(|tools| {
            tools
                .iter()
                .filter_map(|(name, entry)| {
                    entry
                        .get("config")
                        .filter(|config| config.is_object())
                        .map(|config| (name.clone(), config.clone()))
                })
                .collect()
        })
        .unwrap_or_default();
    let configuration = test
        .state
        .product_hooks
        .resolve_complete_chat_configuration(
            &address,
            rua_server::services::product_hooks::ChatConfigurationMaterialization {
                selected_agent_id: agent_id.into(),
                running_config: rua_config::ChatRunningConfig::from_agent_defaults(&running),
                security_config: test.state.security.config_store().load(agent_id).unwrap(),
                agent_assignment: Some(
                    rua_server::services::console_chat::turn_agent_assignment_from_agent(&agent)
                        .unwrap(),
                ),
                prompt_file_content: None,
                tool_config_snapshot: rua_tool::tool_config_secret_io::ToolConfigSnapshot::capture(
                    &Value::Object(configs),
                ),
                capability_configuration: Some(
                    runtime.capability_configuration(None).await.unwrap(),
                ),
                model: draft
                    .get("model")
                    .filter(|value| !value.is_null())
                    .cloned()
                    .map(serde_json::from_value)
                    .transpose()
                    .unwrap(),
                thinking: None,
                collaboration_mode: json!("default"),
                harness_configuration: None,
                authority_device_id: device,
            },
        )
        .await
        .unwrap();
    header.extra.insert(
        "hook_configuration".into(),
        serde_json::to_value(configuration).unwrap(),
    );
    store.save_chat_header(&header).await.unwrap();
    let header = store.load_chat_header(id).await.unwrap().unwrap();
    draft["meta"] = serde_json::to_value(header.extra).unwrap();
    draft
}
