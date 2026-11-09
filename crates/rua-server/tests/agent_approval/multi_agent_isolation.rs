use std::sync::Arc;
use std::time::Duration;

use rua_contract::config::AppConfig;
use rua_server::profiles::ServerProfile;
use rua_server::AppState;
use tempfile::TempDir;

const FIXTURE_DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/multi_agent_isolation"
);

fn setup_two_agent_state() -> (TempDir, AppState) {
    let tmp = TempDir::new().expect("tempdir");
    let data_dir = tmp.path().to_path_buf();
    let secret_dir = tmp.path().join("secret");
    std::fs::create_dir_all(&secret_dir).expect("mkdir secret");

    std::fs::write(
        data_dir.join("config.json"),
        serde_json::json!({
            "agents": { "active_agent": "a" }
        })
        .to_string(),
    )
    .expect("write config.json");

    for agent in ["a", "b"] {
        let ws = data_dir.join("workspaces").join(agent);
        std::fs::create_dir_all(&ws).expect("mkdir workspace");
        let src = std::path::Path::new(FIXTURE_DIR)
            .join(agent)
            .join("agent.json");
        std::fs::copy(&src, ws.join("agent.json")).unwrap_or_else(|e| {
            panic!("copy fixture {}: {e}", src.display());
        });
    }

    let state = AppState::load(
        AppConfig::default(),
        data_dir,
        secret_dir,
        ServerProfile::Test,
    )
    .expect("valid test configuration");
    (tmp, state)
}

async fn get_runtimes_with_runtime_init(
    state: &AppState,
) -> (
    Arc<rua_workspace::WorkspaceRuntime>,
    Arc<rua_workspace::WorkspaceRuntime>,
) {
    let rt_a = state.registry.get_or_create("a").await.expect("runtime a");
    let rt_b = state.registry.get_or_create("b").await.expect("runtime b");
    rt_a.initialize_runtime().await.expect("init runtime a");
    rt_b.initialize_runtime().await.expect("init runtime b");
    (rt_a, rt_b)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_manager_isolation() {
    let (_tmp, state) = setup_two_agent_state();
    let (rt_a, rt_b) = get_runtimes_with_runtime_init(&state).await;

    let mgr_a = rt_a.mcp_manager().await;
    let mgr_b = rt_b.mcp_manager().await;

    assert!(mgr_a.is_some(), "agent a should have an mcp_manager");
    assert!(mgr_b.is_some(), "agent b should have an mcp_manager");

    let ptr_a = Arc::as_ptr(mgr_a.as_ref().unwrap()) as *const () as usize;
    let ptr_b = Arc::as_ptr(mgr_b.as_ref().unwrap()) as *const () as usize;
    assert_ne!(
        ptr_a, ptr_b,
        "mcp_managers must be different instances per agent"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn memory_vec_db_path_isolation() {
    let (_tmp, state) = setup_two_agent_state();

    let rt_a = state.registry.get_or_create("a").await.expect("runtime a");
    let rt_b = state.registry.get_or_create("b").await.expect("runtime b");

    let ws_a = rt_a.workspace_dir();
    let ws_b = rt_b.workspace_dir();
    assert_ne!(ws_a, ws_b, "workspace dirs must differ per agent");

    let vec_db_a = ws_a.join("memory_vec.db");
    let vec_db_b = ws_b.join("memory_vec.db");
    assert_ne!(
        vec_db_a, vec_db_b,
        "memory_vec.db paths must differ per agent"
    );

    assert!(
        vec_db_a.to_string_lossy().contains("/a/"),
        "agent a vec.db should be under /a/"
    );
    assert!(
        vec_db_b.to_string_lossy().contains("/b/"),
        "agent b vec.db should be under /b/"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn channel_manager_isolation() {
    let (_tmp, state) = setup_two_agent_state();

    let cm_a = state.channel_manager_for("a").await.expect("cm a");
    let cm_b = state.channel_manager_for("b").await.expect("cm b");

    assert!(
        !Arc::ptr_eq(&cm_a, &cm_b),
        "channel_managers must be different instances per agent"
    );

    let target = rua_channel::core::ChannelTarget {
        agent_id: "a".into(),
        conversation_id: "conv-1".into(),
        sender_id: None,
        reply_to: None,
        metadata: serde_json::Value::Null,
    };
    cm_a.register_session_target("session-a", target).await;

    assert!(
        cm_a.target_for_session("session-a").await.is_some(),
        "agent a should see its own session target"
    );
    assert!(
        cm_b.target_for_session("session-a").await.is_none(),
        "agent b must NOT see agent a's session target"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn approval_authority_unified_with_data_isolation() {
    use rua_security::{ApprovalRequestInput, ApprovalTrigger, ToolCallSnapshot};

    let (_tmp, state) = setup_two_agent_state();

    let authority = state.security.approval_authority();
    let req = authority
        .create_request(ApprovalRequestInput {
            agent_id: "a".into(),
            owner: rua_security::ApprovalOwner::chat(rua_core::ChatAddress::agent(
                "a",
                "session-a",
            )),
            turn_id: "session-a".into(),
            tool_call_id: "call-a".into(),
            tool_name: "tool_x".into(),
            trigger: ApprovalTrigger::approval_mode("tool_x requires approval"),
            tool_call: ToolCallSnapshot {
                display: "tool_x".into(),
                args: serde_json::json!({}),
            },
        })
        .await;

    let a_pending = authority.list_pending_for_agent("a").await;
    assert_eq!(a_pending.len(), 1, "agent a should have 1 pending");
    assert_eq!(a_pending[0].id, req.id);

    let b_pending = authority.list_pending_for_agent("b").await;
    assert!(
        b_pending.is_empty(),
        "querying under agent b should be empty"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn embedding_config_isolation() {
    let (_tmp, state) = setup_two_agent_state();
    let (rt_a, rt_b) = get_runtimes_with_runtime_init(&state).await;

    let mm_a = rt_a.memory_manager().await;
    let mm_b = rt_b.memory_manager().await;

    assert!(mm_a.is_some(), "agent a should have a memory_manager");
    assert!(mm_b.is_some(), "agent b should have a memory_manager");

    let ptr_a = Arc::as_ptr(mm_a.as_ref().unwrap()) as *const () as usize;
    let ptr_b = Arc::as_ptr(mm_b.as_ref().unwrap()) as *const () as usize;
    assert_ne!(
        ptr_a, ptr_b,
        "memory_managers must be different instances per agent"
    );

    let cfg_a: rua_config::AgentProfileConfig =
        serde_json::from_slice(&std::fs::read(rt_a.workspace_dir().join("agent.json")).unwrap())
            .unwrap();
    let cfg_b: rua_config::AgentProfileConfig =
        serde_json::from_slice(&std::fs::read(rt_b.workspace_dir().join("agent.json")).unwrap())
            .unwrap();

    assert_ne!(
        cfg_a.running.embedding_config.base_url, cfg_b.running.embedding_config.base_url,
        "embedding base_urls must differ per agent"
    );
    assert_eq!(
        cfg_a.running.embedding_config.base_url,
        "http://localhost:11434"
    );
    assert_eq!(
        cfg_b.running.embedding_config.base_url,
        "http://localhost:11435"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn same_name_mcp_server_no_first_wins_drop() {
    let (_tmp, state) = setup_two_agent_state();
    let (rt_a, rt_b) = get_runtimes_with_runtime_init(&state).await;

    let mgr_a = rt_a.mcp_manager().await.expect("mgr a");
    let mgr_b = rt_b.mcp_manager().await.expect("mgr b");

    let servers_a = wait_for_mcp_servers(&*mgr_a, Duration::from_secs(5)).await;
    let servers_b = wait_for_mcp_servers(&*mgr_b, Duration::from_secs(5)).await;

    let has_fs = |servers: &[mcp_manager::status::McpServerStatus]| -> bool {
        servers.iter().any(|s| s.key == "fs")
    };

    assert!(
        has_fs(&servers_a),
        "agent a must still have MCP server 'fs', got: {servers_a:?}"
    );
    assert!(
        has_fs(&servers_b),
        "agent b must still have MCP server 'fs', got: {servers_b:?}"
    );
}

async fn wait_for_mcp_servers(
    mgr: &dyn mcp_manager::manager::McpManager,
    timeout: Duration,
) -> Vec<mcp_manager::status::McpServerStatus> {
    let start = tokio::time::Instant::now();
    loop {
        if let Ok(servers) = mgr.list_servers().await {
            if !servers.is_empty() {
                return servers;
            }
        }
        if start.elapsed() > timeout {
            return vec![];
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
