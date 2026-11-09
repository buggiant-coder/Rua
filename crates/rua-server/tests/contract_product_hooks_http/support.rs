use axum::body::Body;
use axum::extract::Request;
use axum::http::Method;
use axum::response::Response;
use axum::Router;
use http_body_util::BodyExt;
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;

use rua_server::endpoints::{agents, chats, plugins, product_hooks};
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
        .merge(chats::agent_scoped_router())
        .merge(plugins::router())
        .merge(product_hooks::router());
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

pub async fn request(app: &Router, method: Method, uri: &str, body: Option<Value>) -> Response {
    let mut builder = Request::builder().method(method).uri(uri);
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

pub async fn read_json(response: Response) -> Value {
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    serde_json::from_slice(&bytes).expect("response json")
}

pub async fn seed_materialized_chat(test: &TestApp, agent_id: &str, name: &str) -> Value {
    let response = request(
        &test.app,
        Method::POST,
        &format!("/api/agents/{agent_id}/chats"),
        Some(serde_json::json!({"name":name})),
    )
    .await;
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let mut draft = read_json(response).await;
    let id = draft["id"].as_str().unwrap().to_string();
    let runtime = test.state.registry.get_or_create(agent_id).await.unwrap();
    let address = rua_core::ChatAddress::agent(agent_id, &id);
    let device =
        rua_server::services::device_context::current_sync_device_id(&test.state.sync_device)
            .unwrap();
    let placement = rua_server::services::conversation_authority_placement::ConversationAuthorityPlacement {
        device_id: device.clone(), authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::next_for_device(&device, None),
        base_manifest_hash: rua_server::services::conversation_manifest::initial_conversation_manifest_hash(&address).unwrap(),
    };
    rua_server::services::console_chat::materialize_targeted_chat(
        &runtime.workspace_dir(),
        agent_id,
        &id,
        name,
        &placement,
    )
    .await
    .unwrap();
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
                        .filter(|value| value.is_object())
                        .map(|value| (name.clone(), value.clone()))
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
                collaboration_mode: serde_json::json!("default"),
                harness_configuration: None,
                authority_device_id: device,
            },
        )
        .await
        .unwrap();
    let store = rua_session::WorkspaceSessionStore::new(runtime.workspace_dir());
    store
        .update_chat_header(&id, |header| {
            header.extra.insert(
                "hook_configuration".into(),
                serde_json::to_value(&configuration).unwrap(),
            );
        })
        .await
        .unwrap();
    test.state
        .product_hooks
        .adopt_chat_configuration(&address, configuration)
        .await;
    draft["meta"] =
        serde_json::to_value(store.load_chat_header(&id).await.unwrap().unwrap().extra).unwrap();
    draft
}
