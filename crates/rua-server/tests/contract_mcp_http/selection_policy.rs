use axum::http::{Method, StatusCode};
use rua_config::types::tools::{ToolIdentity, ToolSelectionPolicy};
use rua_server::endpoints::dto::McpToolInfo;
use serde_json::{json, Value};

use super::support::{build_app, read_json, request, TestApp};

const AGENT: &str = "mcp_policy_agent";
const SOURCE: &str = "policy_source";
const TOOL: &str = "lookup";

async fn configured_app(policy: Option<ToolSelectionPolicy>) -> TestApp {
    let app = build_app();
    let workspace = app.data_dir().join("workspaces").join(AGENT);
    std::fs::create_dir_all(&workspace).unwrap();
    let tools = match policy {
        Some(policy) => json!({"selection_policy":policy,"builtin_tools":{}}),
        None => json!({"builtin_tools":{}}),
    };
    rua_config::atomic_write_json(
        &workspace.join("agent.json"),
        &json!({
            "id":AGENT,"name":AGENT,"workspace_dir":workspace,"tools":tools
        }),
    )
    .unwrap();

    let response = request(&app.app, Method::POST, &format!("/api/agents/{AGENT}/mcp"), Some(json!({
        "client_key": SOURCE,
        "client": {"name": "Policy source", "enabled": true, "transport": "stdio", "command": "/bin/echo", "args": []}
    })), None).await;
    assert_eq!(response.status(), StatusCode::CREATED);
    app.state
        .mcp_tools_cache
        .put(
            AGENT,
            SOURCE,
            vec![McpToolInfo {
                id: TOOL.into(),
                name: TOOL.into(),
                description: "Lookup".into(),
                enabled: true,
                schema: Some(json!({"type":"object"})),
                extra: Default::default(),
            }],
        )
        .await;
    app
}

async fn list(app: &TestApp) -> Value {
    let response = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{AGENT}/mcp/{SOURCE}/tools"),
        None,
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    read_json(response).await
}

async fn toggle(app: &TestApp, enabled: bool) -> axum::response::Response {
    request(
        &app.app,
        Method::PATCH,
        &format!("/api/agents/{AGENT}/mcp/{SOURCE}/tool-toggle/{TOOL}"),
        Some(json!({"enabled":enabled})),
        None,
    )
    .await
}

#[tokio::test]
async fn restricted_mcp_toggle_updates_exact_policy_and_cached_lists_reproject_it() {
    let app = configured_app(Some(ToolSelectionPolicy::restricted(vec![]))).await;
    assert_eq!(list(&app).await[0]["enabled"], false);
    let excluded_call = request(
        &app.app,
        Method::POST,
        &format!("/api/agents/{AGENT}/mcp/{SOURCE}/call"),
        Some(json!({"tool": TOOL,"arguments":{}})),
        None,
    )
    .await;
    assert_eq!(excluded_call.status(), StatusCode::FORBIDDEN);
    assert!(read_json(excluded_call).await["detail"]
        .as_str()
        .unwrap()
        .contains("selection policy"));
    assert_eq!(toggle(&app, true).await.status(), StatusCode::OK);
    assert_eq!(list(&app).await[0]["enabled"], true);
    let policy = app
        .state
        .tool_config_store
        .selection_policy(Some(AGENT))
        .await
        .unwrap();
    assert_eq!(
        policy,
        ToolSelectionPolicy::restricted(vec![ToolIdentity::mcp(SOURCE, TOOL)])
    );
    let source = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{AGENT}/mcp/{SOURCE}"),
        None,
        None,
    )
    .await;
    let source = read_json(source).await;
    assert_eq!(source["enabled"], true);
    assert_eq!(source["disabled_tools"], json!([]));
    assert_eq!(toggle(&app, false).await.status(), StatusCode::OK);
    assert_eq!(list(&app).await[0]["enabled"], false);
}

#[tokio::test]
async fn default_enabled_mcp_toggle_removes_typed_disable_without_source_dual_write() {
    let mut policy = ToolSelectionPolicy::default_enabled();
    policy.set_tool_enabled(ToolIdentity::mcp(SOURCE, TOOL), false);
    let app = configured_app(Some(policy)).await;
    assert_eq!(list(&app).await[0]["enabled"], false);
    assert_eq!(toggle(&app, true).await.status(), StatusCode::OK);
    assert_eq!(list(&app).await[0]["enabled"], true);
    assert_eq!(
        app.state
            .tool_config_store
            .selection_policy(Some(AGENT))
            .await
            .unwrap(),
        ToolSelectionPolicy::default_enabled()
    );
}

#[tokio::test]
async fn missing_mcp_selection_policy_is_rejected_on_list_toggle_and_call() {
    let app = configured_app(None).await;
    for (method, suffix, body) in [
        (Method::GET, "tools", None),
        (
            Method::PATCH,
            "tool-toggle/lookup",
            Some(json!({"enabled": true})),
        ),
        (
            Method::POST,
            "call",
            Some(json!({"tool":TOOL,"arguments":{}})),
        ),
    ] {
        let response = request(
            &app.app,
            method,
            &format!("/api/agents/{AGENT}/mcp/{SOURCE}/{suffix}"),
            body,
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(read_json(response).await["detail"]
            .as_str()
            .unwrap()
            .contains("required"));
    }
}

struct RecordingManager(std::sync::Mutex<Option<mcp_manager::types::McpCallContext>>);

#[async_trait::async_trait]
impl mcp_manager::manager::McpManager for RecordingManager {
    async fn load_config(
        &self,
        _: mcp_manager::types::McpConfig,
    ) -> mcp_manager::error::McpResult<mcp_manager::types::McpReloadReport> {
        unreachable!()
    }
    async fn configuration(&self) -> mcp_manager::error::McpResult<mcp_manager::types::McpConfig> {
        unreachable!()
    }
    async fn list_servers(
        &self,
    ) -> mcp_manager::error::McpResult<Vec<mcp_manager::status::McpServerStatus>> {
        Ok(vec![])
    }
    async fn list_tools(
        &self,
        _: mcp_manager::types::ToolFilter,
    ) -> mcp_manager::error::McpResult<Vec<mcp_manager::types::McpToolInfo>> {
        unreachable!()
    }
    async fn call_tool(
        &self,
        _: &mcp_manager::types::McpServerId,
        _: &str,
        _: Value,
        context: mcp_manager::types::McpCallContext,
    ) -> mcp_manager::error::McpResult<mcp_manager::types::McpCallResult> {
        *self.0.lock().unwrap() = Some(context);
        Ok(mcp_manager::types::McpCallResult {
            content: vec![],
            structured_content: None,
            is_error: false,
        })
    }
    async fn reload(
        &self,
        _: mcp_manager::types::McpConfig,
    ) -> mcp_manager::error::McpResult<mcp_manager::types::McpReloadReport> {
        unreachable!()
    }
    async fn suspend_new_calls(&self) -> mcp_manager::error::McpResult<()> {
        unreachable!()
    }
    async fn invalidate_tool_cache(
        &self,
        _: &mcp_manager::types::McpServerId,
    ) -> mcp_manager::error::McpResult<()> {
        unreachable!()
    }
    async fn connect_server(
        &self,
        _: &mcp_manager::types::McpServerId,
    ) -> mcp_manager::error::McpResult<()> {
        unreachable!()
    }
    async fn disconnect_server(
        &self,
        _: &mcp_manager::types::McpServerId,
    ) -> mcp_manager::error::McpResult<()> {
        unreachable!()
    }
    async fn replace_client(
        &self,
        _: &mcp_manager::types::McpServerId,
        _: mcp_manager::types::McpServerConfig,
    ) -> mcp_manager::error::McpResult<()> {
        unreachable!()
    }
    async fn shutdown_all(
        &self,
    ) -> mcp_manager::error::McpResult<mcp_manager::types::McpShutdownReport> {
        Ok(Default::default())
    }
}

#[tokio::test]
async fn allowed_mcp_composite_call_carries_verified_policy_for_leaf_admission() {
    let policy = ToolSelectionPolicy::restricted(vec![ToolIdentity::mcp(SOURCE, "interact")]);
    let app = configured_app(Some(policy.clone())).await;
    let manager = std::sync::Arc::new(RecordingManager(Default::default()));
    app.state
        .registry
        .get_or_create(AGENT)
        .await
        .unwrap()
        .set_mcp_manager(manager.clone())
        .await;
    let response = request(
        &app.app,
        Method::POST,
        &format!("/api/agents/{AGENT}/mcp/{SOURCE}/call"),
        Some(json!({"tool":"interact","arguments":{}})),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let captured = manager
        .0
        .lock()
        .unwrap()
        .take()
        .expect("forwarded call context");
    assert_eq!(captured.agent_id, AGENT);
    assert_eq!(
        captured.metadata["chat_capability_configuration"]["selection_policy"],
        serde_json::to_value(policy).unwrap()
    );
}

#[tokio::test]
async fn agent_source_writes_reject_legacy_disabled_tools_instead_of_ignoring_intent() {
    let app = configured_app(Some(ToolSelectionPolicy::default_enabled())).await;
    for path in ["/api/mcp".to_string(), format!("/api/agents/{AGENT}/mcp")] {
        let response = request(&app.app, Method::POST, &path, Some(json!({
            "client_key":"rejected_source", "client":{"name":"Rejected", "enabled":false,"transport":"stdio","command":"/bin/echo","disabled_tools":["lookup"]}
        })), Some(AGENT)).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(read_json(response).await["detail"]
            .as_str()
            .unwrap()
            .contains("selection policy"));
    }
    for path in [
        format!("/api/mcp/{SOURCE}"),
        format!("/api/agents/{AGENT}/mcp/{SOURCE}"),
    ] {
        let response = request(
            &app.app,
            Method::PUT,
            &path,
            Some(json!({"disabled_tools":["lookup"]})),
            Some(AGENT),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
    let response = request(
        &app.app,
        Method::PUT,
        &format!("/api/agents/{AGENT}/mcp/{SOURCE}"),
        Some(json!({"disabled_tools":[]})),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
}
