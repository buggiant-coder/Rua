use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use mcp_manager::client::McpClient;
use mcp_manager::error::{McpError, McpResult};
use mcp_manager::manager::McpManager;
use mcp_manager::status::{McpConnectionState, McpServerStatus};
use mcp_manager::types::{
    AutoConnectPolicy, McpCallContext, McpCallResult, McpConfig, McpContent, McpInitializeResult,
    McpServerConfig, McpServerId, McpToolInfo, McpTransportConfig, ToolFilter,
};
use rua_mcp::manager::{McpClientStatus, McpManagerImpl, ServerEntry};
use rua_mcp::mobile_policy::{McpMobilePolicy, PlatformProfile};
use tokio::sync::RwLock;

#[derive(Clone)]
struct RecordedCall {
    name: String,
    args: serde_json::Value,
    agent_id: String,
    session_id: String,
}

struct FakeClientState {
    fail_shutdown: std::sync::atomic::AtomicBool,
    status: Mutex<McpConnectionState>,
    connect_count: Mutex<usize>,
    calls: Mutex<Vec<RecordedCall>>,
}

impl FakeClientState {
    fn new(status: McpConnectionState) -> Arc<Self> {
        Arc::new(Self {
            fail_shutdown: std::sync::atomic::AtomicBool::new(false),
            status: Mutex::new(status),
            connect_count: Mutex::new(0),
            calls: Mutex::new(Vec::new()),
        })
    }

    fn connect_count(&self) -> usize {
        *self.connect_count.lock().expect("connect count")
    }

    fn calls(&self) -> Vec<RecordedCall> {
        self.calls.lock().expect("calls").clone()
    }
}

struct FakeClient {
    server_id: String,
    state: Arc<FakeClientState>,
    result: McpResult<McpCallResult>,
}

#[async_trait]
impl McpClient for FakeClient {
    async fn connect(&self) -> McpResult<()> {
        *self.state.connect_count.lock().expect("connect count") += 1;
        *self.state.status.lock().expect("status") = McpConnectionState::Ready;
        Ok(())
    }

    async fn initialize_result(&self) -> McpResult<McpInitializeResult> {
        Ok(McpInitializeResult {
            server_name: self.server_id.clone(),
            server_version: "test".into(),
            protocol_version: "2025-03-26".into(),
            capabilities: BTreeMap::new(),
        })
    }

    async fn list_tools(&self) -> McpResult<Vec<McpToolInfo>> {
        Ok(vec![McpToolInfo {
            server_id: McpServerId::new(&self.server_id),
            name: "search_docs".into(),
            description: "search docs".into(),
            input_schema: serde_json::json!({"type": "object"}),
            raw: serde_json::Value::Null,
        }])
    }

    async fn call_tool(
        &self,
        name: &str,
        args: serde_json::Value,
        ctx: McpCallContext,
    ) -> McpResult<McpCallResult> {
        self.state.calls.lock().expect("calls").push(RecordedCall {
            name: name.to_string(),
            args,
            agent_id: ctx.agent_id,
            session_id: ctx.session_id,
        });
        self.result.clone()
    }

    async fn invalidate_tool_cache(&self) {}

    async fn shutdown(&self) -> McpResult<()> {
        if self
            .state
            .fail_shutdown
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return Err(McpError::Internal {
                message: "controlled shutdown failure".to_string(),
            });
        }
        *self.state.status.lock().expect("status") = McpConnectionState::NotConnected;
        Ok(())
    }

    fn status(&self) -> McpServerStatus {
        McpServerStatus {
            key: self.server_id.clone(),
            name: self.server_id.clone(),
            enabled: true,
            state: *self.state.status.lock().expect("status"),
            transport_type: "stdio".into(),
            tools_count: 0,
            last_error: None,
            connected_at: None,
            mobile_unavailable_reason: None,
            config_errors: vec![],
        }
    }
}

fn server_config(enabled: bool) -> McpServerConfig {
    McpServerConfig {
        name: "fake server".into(),
        description: "fake MCP server".into(),
        enabled,
        transport: McpTransportConfig::Stdio {
            command: "fake".into(),
            args: vec![],
            env: BTreeMap::new(),
            cwd: None,
            use_proot: false,
        },
        auto_connect: Default::default(),
        call_timeout_secs: 60,
        max_concurrency: 4,
        oauth: None,
        disabled_tools: Vec::new(),
        extra: BTreeMap::new(),
    }
}

#[tokio::test]
async fn unchanged_configuration_preserves_client_but_failed_client_is_reapplied() {
    let temp = tempfile::tempdir().unwrap();
    let manager = McpManagerImpl::new(McpMobilePolicy::new(PlatformProfile::Desktop))
        .with_runtime_dirs(temp.path(), temp.path().join("cache"));
    let id = McpServerId::new("retry-fixture");
    let mut disabled = server_config(false);
    if let McpTransportConfig::Stdio { command, .. } = &mut disabled.transport {
        *command = std::env::current_exe()
            .unwrap()
            .to_string_lossy()
            .into_owned();
    }
    let config = McpConfig {
        clients: BTreeMap::from([(id.clone(), disabled)]),
    };
    assert!(manager
        .reload(config.clone())
        .await
        .unwrap()
        .errors
        .is_empty());
    let first = manager.servers.read().await[&id].status.clone();
    manager.suspend_new_calls().await.unwrap();
    assert_eq!(
        manager.list_servers().await.unwrap()[0].state,
        McpConnectionState::Failed
    );
    assert!(manager.list_tools(ToolFilter::default()).await.is_err());
    assert!(manager
        .call_tool(
            &id,
            "fixture",
            serde_json::json!({}),
            McpCallContext::default()
        )
        .await
        .is_err());
    let unchanged = manager.reload(config.clone()).await.unwrap();
    assert!(manager.list_tools(ToolFilter::default()).await.is_ok());
    assert_eq!(unchanged.unchanged, vec![id.clone()]);
    assert!(Arc::ptr_eq(
        &first,
        &manager.servers.read().await[&id].status
    ));
    *manager.servers.read().await[&id].status.write().await =
        McpClientStatus::Failed("fixture".into());
    let retried = manager.reload(config).await.unwrap();
    assert_eq!(retried.updated, vec![id.clone()]);
    assert!(retried.errors.is_empty());
    assert!(!Arc::ptr_eq(
        &first,
        &manager.servers.read().await[&id].status
    ));
    manager.shutdown_all().await.unwrap();
}

#[tokio::test]
async fn failed_removal_blocks_new_calls_and_retries_the_saved_configuration() {
    let id = McpServerId::new("failed-removal");
    let state = FakeClientState::new(McpConnectionState::Ready);
    let manager = McpManagerImpl::new(McpMobilePolicy::new(PlatformProfile::Desktop));
    let config = McpConfig {
        clients: BTreeMap::from([(id.clone(), server_config(true))]),
    };
    manager.reload(config).await.unwrap();
    let fixture = manager_with_fake_client(
        &id,
        true,
        state.clone(),
        Ok(McpCallResult {
            content: Vec::new(),
            structured_content: None,
            is_error: false,
        }),
    )
    .await;
    let entry = fixture.servers.write().await.remove(&id).unwrap();
    manager.servers.write().await.insert(id.clone(), entry);
    state
        .fail_shutdown
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let failed = manager.reload(McpConfig::default()).await.unwrap();
    assert!(failed.errors.contains_key(&id));
    assert!(manager.servers.read().await.contains_key(&id));
    assert!(manager
        .call_tool(
            &id,
            "search_docs",
            serde_json::json!({}),
            McpCallContext::default()
        )
        .await
        .is_err());
    assert!(state.calls().is_empty());
    state
        .fail_shutdown
        .store(false, std::sync::atomic::Ordering::SeqCst);
    let retry = manager.reload(McpConfig::default()).await.unwrap();
    assert!(retry.errors.is_empty());
    assert!(!manager.servers.read().await.contains_key(&id));
}

async fn manager_with_fake_client(
    server_id: &McpServerId,
    enabled: bool,
    state: Arc<FakeClientState>,
    result: McpResult<McpCallResult>,
) -> McpManagerImpl {
    let manager = McpManagerImpl::new(McpMobilePolicy::new(PlatformProfile::Desktop));
    manager.servers.write().await.insert(
        server_id.clone(),
        ServerEntry {
            stop: tokio_util::sync::CancellationToken::new(),
            config: server_config(enabled),
            client: Box::new(FakeClient {
                server_id: server_id.to_string(),
                state,
                result,
            }),
            status: Arc::new(RwLock::new(McpClientStatus::Ready)),
        },
    );
    manager
}

async fn manager_with_fake_client_status(
    server_id: &McpServerId,
    enabled: bool,
    state: Arc<FakeClientState>,
    result: McpResult<McpCallResult>,
    status: McpClientStatus,
) -> McpManagerImpl {
    let manager = McpManagerImpl::new(McpMobilePolicy::new(PlatformProfile::Desktop));
    manager.servers.write().await.insert(
        server_id.clone(),
        ServerEntry {
            stop: tokio_util::sync::CancellationToken::new(),
            config: server_config(enabled),
            client: Box::new(FakeClient {
                server_id: server_id.to_string(),
                state,
                result,
            }),
            status: Arc::new(RwLock::new(status)),
        },
    );
    manager
}

#[tokio::test]
async fn bridge_uses_chat_snapshot_and_recognizes_exact_approved_invocation() {
    use rua_contract::Tool;
    let dir = tempfile::tempdir().unwrap();
    let security = Arc::new(rua_security::SecurityModule::new(
        Arc::new(rua_security::SecurityConfigStore::new(
            dir.path().to_path_buf(),
        )),
        None,
    ));
    let server_id = McpServerId::new("snapshot");
    let state = FakeClientState::new(McpConnectionState::Ready);
    let manager = Arc::new(
        manager_with_fake_client(
            &server_id,
            true,
            state.clone(),
            Ok(McpCallResult {
                structured_content: None,
                content: Vec::new(),
                is_error: false,
            }),
        )
        .await,
    );
    let bridge = rua_mcp::McpToolBridge::new(manager).with_security(security.clone());
    let tool = bridge
        .discover_tools_from(&server_id)
        .await
        .unwrap()
        .remove(0);
    let address = rua_core::ChatAddress::agent("agent", "chat");
    let mut config = rua_config::types::security::SecurityConfig::default();
    config.tool_guard.approval_mode = "ask_first".into();
    let mut ctx = rua_core::tool::ToolContext {
        agent_id: "agent".into(),
        session_id: "chat".into(),
        ..Default::default()
    };
    ctx.metadata.insert(
        "security_chat_address".into(),
        serde_json::to_value(&address).unwrap(),
    );
    ctx.metadata
        .insert("turn_id".into(), serde_json::json!("turn"));
    ctx.metadata
        .insert("tool_call_id".into(), serde_json::json!("call"));
    ctx.metadata.insert(
        rua_security::models::EFFECTIVE_SECURITY_CONFIG_METADATA_KEY.into(),
        serde_json::to_value(config).unwrap(),
    );
    let args = serde_json::json!({});
    assert!(tool.call(args.clone(), ctx.clone()).await.is_err());
    assert_eq!(state.calls().len(), 0);
    let pending = security
        .approval_authority()
        .snapshot_at(&address)
        .await
        .pending;
    assert_eq!(pending.len(), 1);
    security
        .approval_authority()
        .decide(
            &pending[0].id,
            rua_security::ApprovalDecision::RunOnce {
                decided_by: "user".into(),
            },
        )
        .await
        .unwrap();
    tool.call(args, ctx).await.unwrap();
    assert_eq!(state.calls().len(), 1);
}

#[tokio::test]
async fn list_tools_for_specific_connecting_server_lazy_connects_instead_of_returning_empty() {
    let server_id = McpServerId::new("fake-connecting");
    let state = FakeClientState::new(McpConnectionState::NotConnected);
    let manager = manager_with_fake_client_status(
        &server_id,
        true,
        state.clone(),
        Ok(McpCallResult {
            structured_content: None,
            content: vec![],
            is_error: false,
        }),
        McpClientStatus::Connecting,
    )
    .await;
    let status = {
        let servers = manager.servers.read().await;
        Arc::clone(
            &servers
                .get(&server_id)
                .expect("server entry should exist")
                .status,
        )
    };
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        *status.write().await = McpClientStatus::Ready;
    });

    let tools = manager
        .list_tools(ToolFilter {
            server_id: Some(server_id.clone()),
            name_prefix: None,
            include_disabled: true,
        })
        .await
        .expect("specific tools listing should lazy-connect");

    assert_eq!(state.connect_count(), 1);
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "search_docs");
}

#[tokio::test]
async fn disabled_mcp_tools_are_hidden_from_agent_discovery_but_remain_diagnosable() {
    let server_id = McpServerId::new("fake-hidden");
    let state = FakeClientState::new(McpConnectionState::Ready);
    let manager = manager_with_fake_client(
        &server_id,
        true,
        state,
        Ok(McpCallResult {
            structured_content: None,
            content: vec![],
            is_error: false,
        }),
    )
    .await;
    manager
        .servers
        .write()
        .await
        .get_mut(&server_id)
        .expect("server entry")
        .config
        .disabled_tools = vec!["search_docs".to_string()];

    let visible = manager
        .list_tools(ToolFilter::default())
        .await
        .expect("agent-facing tool list");
    assert!(visible.is_empty());

    let diagnostic = manager
        .list_tools(ToolFilter {
            include_disabled: true,
            ..ToolFilter::default()
        })
        .await
        .expect("diagnostic tool list");
    assert_eq!(diagnostic.len(), 1);
    assert_eq!(diagnostic[0].name, "search_docs");
}

#[tokio::test]
async fn stale_absolute_stdio_command_is_failed_and_exposes_no_tools() {
    let temp = tempfile::tempdir().expect("tempdir");
    let server_id = McpServerId::new("desktop-only");
    let command = temp.path().join("desktop-install").join("codegraph");
    let manager = McpManagerImpl::new(McpMobilePolicy::new(PlatformProfile::Desktop))
        .with_runtime_dirs(temp.path(), temp.path().join("cache"));
    manager
        .load_config(McpConfig {
            clients: BTreeMap::from([(
                server_id.clone(),
                McpServerConfig {
                    name: "Desktop CodeGraph".to_string(),
                    description: "Configured on another device".to_string(),
                    enabled: true,
                    transport: McpTransportConfig::Stdio {
                        command: command.to_string_lossy().into_owned(),
                        args: Vec::new(),
                        env: BTreeMap::new(),
                        cwd: None,
                        use_proot: false,
                    },
                    auto_connect: AutoConnectPolicy::Manual,
                    call_timeout_secs: 1,
                    max_concurrency: 1,
                    oauth: None,
                    disabled_tools: Vec::new(),
                    extra: BTreeMap::new(),
                },
            )]),
        })
        .await
        .expect("load config");

    let error = manager
        .connect_server(&server_id)
        .await
        .expect_err("missing command should fail");
    assert!(error.to_string().contains("not found in Runtime Inventory"));

    let statuses = manager.list_servers().await.expect("list statuses");
    assert_eq!(statuses.len(), 1);
    assert_eq!(statuses[0].state, McpConnectionState::Failed);
    assert!(statuses[0].last_error.is_some());
    assert!(manager
        .list_tools(ToolFilter::default())
        .await
        .expect("list tools")
        .is_empty());
}

#[tokio::test]
async fn call_tool_lazy_connects_and_returns_success_payload() {
    let server_id = McpServerId::new("fake-success");
    let state = FakeClientState::new(McpConnectionState::NotConnected);
    let result = McpCallResult {
        structured_content: None,
        content: vec![McpContent::Text {
            text: "mcp success".into(),
        }],
        is_error: false,
    };
    let manager = manager_with_fake_client(&server_id, true, state.clone(), Ok(result)).await;

    let response = manager
        .call_tool(
            &server_id,
            "search_docs",
            serde_json::json!({"query": "rust"}),
            McpCallContext {
                agent_id: "agent-a".into(),
                session_id: "session-a".into(),
                cancel: None,
                timeout: None,
                ..Default::default()
            },
        )
        .await
        .expect("call tool");

    assert!(!response.is_error);
    assert_eq!(response.text(), "mcp success");
    assert_eq!(state.connect_count(), 1);

    let calls = state.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "search_docs");
    assert_eq!(calls[0].args, serde_json::json!({"query": "rust"}));
    assert_eq!(calls[0].agent_id, "agent-a");
    assert_eq!(calls[0].session_id, "session-a");
}

#[tokio::test]
async fn disabled_server_rejects_call_before_touching_client() {
    let server_id = McpServerId::new("fake-disabled");
    let state = FakeClientState::new(McpConnectionState::Ready);
    let manager = manager_with_fake_client(
        &server_id,
        false,
        state.clone(),
        Ok(McpCallResult {
            structured_content: None,
            content: vec![],
            is_error: false,
        }),
    )
    .await;

    let error = manager
        .call_tool(
            &server_id,
            "search_docs",
            serde_json::json!({}),
            McpCallContext::default(),
        )
        .await
        .expect_err("disabled server should reject call");

    assert!(matches!(error, McpError::PermissionDenied { .. }));
    assert_eq!(state.connect_count(), 0);
    assert!(state.calls().is_empty());
}

#[tokio::test]
async fn tool_level_error_result_is_returned_without_transport_error() {
    let server_id = McpServerId::new("fake-tool-error");
    let state = FakeClientState::new(McpConnectionState::Ready);
    let manager = manager_with_fake_client(
        &server_id,
        true,
        state.clone(),
        Ok(McpCallResult {
            structured_content: None,
            content: vec![McpContent::Text {
                text: "tool reported invalid input".into(),
            }],
            is_error: true,
        }),
    )
    .await;

    let response = manager
        .call_tool(
            &server_id,
            "validate",
            serde_json::json!({"bad": true}),
            McpCallContext::default(),
        )
        .await
        .expect("MCP tool-level errors are successful call responses");

    assert!(response.is_error);
    assert_eq!(response.text(), "tool reported invalid input");
    assert_eq!(state.calls().len(), 1);
}

#[tokio::test]
async fn transport_error_is_returned_as_error() {
    let server_id = McpServerId::new("fake-missing-tool");
    let state = FakeClientState::new(McpConnectionState::Ready);
    let manager = manager_with_fake_client(
        &server_id,
        true,
        state.clone(),
        Err(McpError::ToolNotFound {
            server: server_id.to_string(),
            tool: "missing".into(),
        }),
    )
    .await;

    let error = manager
        .call_tool(
            &server_id,
            "missing",
            serde_json::json!({}),
            McpCallContext::default(),
        )
        .await
        .expect_err("transport/protocol errors should return Err");

    assert!(matches!(
        error,
        McpError::ToolNotFound {
            server,
            tool
        } if server == "fake-missing-tool" && tool == "missing"
    ));
    assert_eq!(state.calls().len(), 1);
}

#[tokio::test]
async fn missing_server_returns_not_found_before_client_lookup() {
    let manager = McpManagerImpl::new(McpMobilePolicy::new(PlatformProfile::Desktop));
    manager
        .load_config(McpConfig::default())
        .await
        .expect("empty config");
    let missing = McpServerId::new("missing-server");

    let error = manager
        .call_tool(
            &missing,
            "anything",
            serde_json::json!({}),
            McpCallContext::default(),
        )
        .await
        .expect_err("missing server should return not found");

    assert!(matches!(
        error,
        McpError::ServerNotFound { server } if server == "missing-server"
    ));
}
