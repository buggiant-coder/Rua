use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use rua_device_network::device_list::{DeviceEntry, DeviceList};
use rua_device_network::{
    BoundDeviceNetworkMembership, DeviceNetworkBootstrap, DeviceNetworkIdentity,
    DeviceNetworkPreset, DeviceNetworkRuntime, DevicePeerDirectory, DeviceSessionCommandHandler,
    DeviceSessionConfig,
};
use rua_server::services::device_command_bus::{
    deliver_project_device_command_via_session, deliver_read_check_via_session,
    DeviceCommandHandler, ProjectDeviceAction, ProjectDeviceCommand, ProjectDeviceResponse,
};
use tempfile::TempDir;
use tokio::sync::{Mutex, Notify};

struct RemoteTaskInputSink {
    inputs: Mutex<Vec<rua_task::TaskInput>>,
    accepted: Notify,
}

#[async_trait::async_trait]
impl rua_task::TaskInputSink for RemoteTaskInputSink {
    fn declared_capability(&self) -> rua_task::TaskInputCapability {
        rua_task::TaskInputCapability::Stdin
    }

    fn active_capability(&self) -> rua_task::TaskInputCapability {
        rua_task::TaskInputCapability::Stdin
    }

    async fn send_input(
        &self,
        input: &rua_task::TaskInput,
    ) -> Result<(), rua_task::TaskInputError> {
        self.inputs.lock().await.push(input.clone());
        self.accepted.notify_one();
        Ok(())
    }

    async fn close(&self) {}
}

struct RemoteTaskRunner {
    sink: Arc<RemoteTaskInputSink>,
}

#[async_trait::async_trait]
impl rua_task::TaskRunner for RemoteTaskRunner {
    fn kind(&self) -> rua_task::TaskRunnerKind {
        rua_task::TaskRunnerKind::Tool
    }

    async fn run(
        &self,
        request: rua_task::TaskRunRequest,
    ) -> Result<rua_task::TaskRunOutput, rua_task::TaskRunnerError> {
        request.session.activate(self.sink.clone()).await.unwrap();
        self.sink.accepted.notified().await;
        Ok(rua_task::TaskRunOutput::completed("remote Project Task"))
    }
}

struct TestPeer {
    dir: TempDir,
    runtime: DeviceNetworkRuntime,
    device_id: String,
    addr: iroh::EndpointAddr,
}

async fn build_peer(
    secret_byte: u8,
    network_secret: [u8; 32],
    peers: Vec<DeviceEntry>,
    command_handler: Option<Arc<dyn DeviceSessionCommandHandler>>,
) -> TestPeer {
    let dir = TempDir::new().expect("tempdir");
    let sync_dir = dir.path().join("sync");
    std::fs::create_dir_all(&sync_dir).expect("sync dir");
    std::fs::write(
        sync_dir.join("device_list.json"),
        serde_json::to_vec_pretty(&DeviceList {
            devices: peers,
            ..Default::default()
        })
        .expect("serialize peers"),
    )
    .expect("write peers");
    let secret = iroh::SecretKey::from_bytes(&[secret_byte; 32]);
    let identity = DeviceNetworkIdentity::new(secret);
    let device_id = hex::encode(identity.local_device_id());
    let bootstrap = DeviceNetworkBootstrap::bind(
        DeviceNetworkPreset::Local,
        identity,
        BoundDeviceNetworkMembership::new(network_secret),
        DevicePeerDirectory::from_sync_dir(&sync_dir),
    )
    .await
    .expect("bind runtime");
    let runtime = bootstrap.spawn_with_session_config(
        Vec::new(),
        command_handler,
        DeviceSessionConfig {
            connect_timeout: Duration::from_secs(2),
            handshake_timeout: Duration::from_secs(2),
            reconnect_attempts: 1,
            ..Default::default()
        },
    );
    let addr = local_endpoint_addr(runtime.endpoint());
    TestPeer {
        dir,
        runtime,
        device_id,
        addr,
    }
}

fn peer_entry(secret_byte: u8) -> DeviceEntry {
    let secret = iroh::SecretKey::from_bytes(&[secret_byte; 32]);
    let public_key = *secret.public().as_bytes();
    let mut device_id = [0; 8];
    device_id.copy_from_slice(&public_key[..8]);
    DeviceEntry {
        device_id,
        public_key,
        label: format!("device-{secret_byte}"),
        friendly_name: format!("Device {secret_byte}"),
        added_at_ms: 1,
        last_seen_ms: 1,
        direct_addrs: Vec::new(),
    }
}

fn local_endpoint_addr(endpoint: &iroh::Endpoint) -> iroh::EndpointAddr {
    let mut addr = iroh::EndpointAddr::new(endpoint.id());
    for socket_addr in endpoint.bound_sockets() {
        let dialable = if socket_addr.ip().is_unspecified() {
            match socket_addr {
                std::net::SocketAddr::V4(addr) => std::net::SocketAddr::new(
                    std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
                    addr.port(),
                ),
                std::net::SocketAddr::V6(addr) => std::net::SocketAddr::new(
                    std::net::IpAddr::V6(std::net::Ipv6Addr::LOCALHOST),
                    addr.port(),
                ),
            }
        } else {
            socket_addr
        };
        addr.addrs.insert(iroh::TransportAddr::Ip(dialable));
    }
    addr
}

fn install_connectable_peer(source: &TestPeer, target: &TestPeer) {
    let path = source.dir.path().join("sync/device_list.json");
    let mut device_list =
        serde_json::from_slice::<DeviceList>(&std::fs::read(&path).unwrap()).unwrap();
    let public_key = *target.addr.id.as_bytes();
    let mut device_id = [0; 8];
    device_id.copy_from_slice(&public_key[..8]);
    device_list
        .devices
        .retain(|entry| entry.device_id != device_id);
    device_list.add_device(DeviceEntry {
        device_id,
        public_key,
        label: target.device_id.clone(),
        friendly_name: target.device_id.clone(),
        added_at_ms: 1,
        last_seen_ms: 1,
        direct_addrs: target
            .addr
            .addrs
            .iter()
            .filter_map(|addr| match addr {
                iroh::TransportAddr::Ip(addr) => Some(addr.to_string()),
                _ => None,
            })
            .collect(),
    });
    std::fs::write(path, serde_json::to_vec_pretty(&device_list).unwrap()).unwrap();
}

async fn project_http(
    app: &axum::Router,
    method: axum::http::Method,
    uri: &str,
    body: serde_json::Value,
) -> (axum::http::StatusCode, serde_json::Value) {
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    let response = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method(method)
                .uri(uri)
                .header("content-type", "application/json")
                .body(axum::body::Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[test]
fn directory_command_binds_authenticated_source_and_intended_target() {
    let cmd = ProjectDeviceCommand {
        command_id: "directory".into(),
        source_device_id: "controller".into(),
        target_device_id: "owner".into(),
        action: ProjectDeviceAction::BrowseDirectory { path: "/".into() },
    };
    assert!(cmd
        .validate_session_route(Some("controller"), "owner")
        .is_ok());
    assert!(matches!(
        cmd.validate_session_route(None, "owner"),
        Err(rua_server::services::device_command_bus::DeviceCommandBusError::UnauthenticatedSource)
    ));
    assert!(matches!(
        cmd.validate_session_route(Some("other"), "owner"),
        Err(rua_server::services::device_command_bus::DeviceCommandBusError::UnauthenticatedSource)
    ));
    assert!(matches!(
        cmd.validate_session_route(Some("controller"), "other"),
        Err(rua_server::services::device_command_bus::DeviceCommandBusError::WrongTarget { .. })
    ));
}

fn read_check_request(
    source: &str,
    target: &str,
) -> rua_server::services::device_read_check::DeviceReadCheckRequest {
    use rua_server::services::{
        conversation_authority_placement::ConversationAuthorityEpoch,
        device_read_check::DeviceReadCheckRequest,
    };
    DeviceReadCheckRequest {
        source_device_id: source.into(),
        target_device_id: target.into(),
        address: rua_core::ChatAddress::new(uuid::Uuid::new_v4().to_string()),
        agent_id: "agent".into(),
        authority_epoch: ConversationAuthorityEpoch::next_for_device(source, None),
        configuration_epoch: "config".into(),
        turn_id: "turn".into(),
        tool_call_id: "call".into(),
        confirmation: None,
        arguments: serde_json::json!({"device_id":target,"file_path":"/fixture.txt"}),
    }
}

#[tokio::test]
async fn read_permission_check_requires_the_authenticated_route_and_local_context() {
    let dir = TempDir::new().unwrap();
    let handler = DeviceCommandHandler::new(dir.path().join("commands"), "target".into());
    let request = read_check_request("source", "target");
    for (peer, source, target, argument_device, expected) in [
        (None, "source", "target", "target", "denied"),
        (Some("other"), "source", "target", "target", "denied"),
        (Some("source"), "source", "other", "other", "denied"),
        (Some("target"), "target", "target", "target", "denied"),
        (Some("source"), "source", "target", "other", "denied"),
        (Some("source"), "source", "target", "target", "unavailable"),
    ] {
        let mut request = request.clone();
        request.source_device_id = source.into();
        request.target_device_id = target.into();
        request.arguments["device_id"] = argument_device.into();
        let payload = serde_json::to_vec(&serde_json::json!({
            "command": "read_check", "payload": request
        }))
        .unwrap();
        let response = match peer {
            Some(peer) => handler.handle_command_from(peer, &payload).await,
            None => handler.handle_command(&payload).await,
        }
        .unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&response).unwrap(),
            serde_json::json!({"status":"read_check","response":{"outcome":expected}})
        );
    }
}

struct DisconnectBeforeReply {
    handler: DeviceCommandHandler,
    sessions: std::sync::Mutex<Option<rua_device_network::DeviceSessionManager>>,
    calls: std::sync::atomic::AtomicUsize,
}

#[async_trait::async_trait]
impl DeviceSessionCommandHandler for DisconnectBeforeReply {
    async fn handle_command(&self, _payload: &[u8]) -> Result<Vec<u8>, String> {
        Err("Authenticated Device Session required".into())
    }
    async fn handle_command_from(&self, peer: &str, payload: &[u8]) -> Result<Vec<u8>, String> {
        let response = self.handler.handle_command_from(peer, payload).await?;
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let sessions = self.sessions.lock().unwrap().take();
        if let Some(sessions) = sessions {
            sessions
                .disconnect_peer(peer)
                .await
                .map_err(|e| e.to_string())?;
        }
        Ok(response)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_permission_check_uses_the_paired_session_once_on_lost_reply() {
    use rua_server::services::device_read_check::DeviceReadCheckReply;
    use std::sync::atomic::Ordering;
    let temp = TempDir::new().unwrap();
    let source = build_peer(1, [44; 32], Vec::new(), None).await;
    let target_id = hex::encode(peer_entry(2).device_id);
    let receiver = Arc::new(DisconnectBeforeReply {
        handler: DeviceCommandHandler::new(temp.path().join("commands"), target_id.clone()),
        sessions: Default::default(),
        calls: Default::default(),
    });
    let target = build_peer(2, [44; 32], vec![peer_entry(1)], Some(receiver.clone())).await;
    install_connectable_peer(&source, &target);
    let request = read_check_request(&source.device_id, &target_id);
    let check = || {
        deliver_read_check_via_session(
            source.runtime.session_manager(),
            &request,
            Duration::from_secs(5),
        )
    };

    assert_eq!(check().await.unwrap(), DeviceReadCheckReply::Unavailable);
    assert_eq!(receiver.calls.load(Ordering::SeqCst), 1);

    *receiver.sessions.lock().unwrap() = Some(target.runtime.session_manager().clone());
    assert!(check().await.is_err());
    assert_eq!(receiver.calls.load(Ordering::SeqCst), 2);
    source.runtime.shutdown().await.unwrap();
    target.runtime.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn console_directory_create_reports_lost_receipt_without_repeating_the_write() {
    use axum::http::{Method, StatusCode};
    use serde_json::json;
    let temp = tempfile::tempdir().unwrap();
    let selected = temp.path().join("selected-parent");
    std::fs::create_dir(&selected).unwrap();
    let source = build_peer(1, [44; 32], Vec::new(), None).await;
    let target_device =
        hex::encode(&iroh::SecretKey::from_bytes(&[2; 32]).public().as_bytes()[..8]);
    let receiver = Arc::new(DisconnectBeforeReply {
        handler: DeviceCommandHandler::new(
            temp.path().join("device_commands"),
            target_device.clone(),
        ),
        sessions: Default::default(),
        calls: Default::default(),
    });
    let target = build_peer(2, [44; 32], vec![peer_entry(1)], Some(receiver.clone())).await;
    *receiver.sessions.lock().unwrap() = Some(target.runtime.session_manager().clone());
    install_connectable_peer(&source, &target);
    let state = rua_server::AppState::from_tempdir(
        source.dir.path().join("controller"),
        rua_server::ServerProfile::Test,
    );
    state.wait_platform_initialization().await.unwrap();
    *state.sync_device.write().unwrap() = Some(Arc::new(
        serde_json::from_value(json!({"secret_bytes":vec![1u8;32]})).unwrap(),
    ));
    *state.device_network.lock().await = Some(source.runtime.clone());
    let app = rua_server::router::build_app(state, None);
    let (status, response) = project_http(
        &app,
        Method::POST,
        "/api/projects/directory/create",
        json!({"device_id":target_device,"parent":selected,"name":"new-folder"}),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{response}");
    let detail = response["detail"].as_str().unwrap();
    assert!(
        detail.contains(&target_device) && detail.contains("is unconfirmed"),
        "{detail}"
    );
    assert!(selected.join("new-folder").is_dir());
    assert_eq!(receiver.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    source.runtime.shutdown().await.unwrap();
    target.runtime.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn remote_resource_validation_and_inspection_use_device_sessions_and_only_creator_commits() {
    let network_secret = [44; 32];
    let source = build_peer(1, network_secret, Vec::new(), None).await;
    let target_data = tempfile::tempdir().unwrap();
    let target_repo = target_data.path().join("remote-repo");
    std::fs::create_dir_all(target_repo.join("src")).unwrap();
    let stores =
        Arc::new(rua_project::ProjectCatalog::open(target_data.path().join("projects")).unwrap());
    let target_device_id =
        hex::encode(&iroh::SecretKey::from_bytes(&[2; 32]).public().as_bytes()[..8]);
    let target_task_runtime = Arc::new(
        rua_task::TaskRuntime::open(rua_task::TaskRuntimeConfig::new(target_data.path())).unwrap(),
    );
    let remote_task_runner = Arc::new(RemoteTaskRunner {
        sink: Arc::new(RemoteTaskInputSink {
            inputs: Mutex::new(Vec::new()),
            accepted: Notify::new(),
        }),
    });
    target_task_runtime.register_runner(remote_task_runner.clone());
    let handler = Arc::new(
        DeviceCommandHandler::new(
            target_data.path().join("device_commands"),
            target_device_id.clone(),
        )
        .with_project_runtime(Arc::clone(&stores))
        .with_task_runtime(Arc::clone(&target_task_runtime)),
    );
    let target = build_peer(2, network_secret, vec![peer_entry(1)], Some(handler)).await;
    install_connectable_peer(&source, &target);

    let browse = deliver_project_device_command_via_session(
        source.runtime.session_manager(),
        &ProjectDeviceCommand {
            command_id: "project-browse-1".to_string(),
            source_device_id: source.device_id.clone(),
            target_device_id: target.device_id.clone(),
            action: ProjectDeviceAction::BrowseDirectory {
                path: target_repo.to_string_lossy().to_string(),
            },
        },
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    match browse {
        ProjectDeviceResponse::Directory { path, entries, .. } => {
            assert_eq!(path, target_repo);
            assert_eq!(entries[0].name, "src");
        }
        other => panic!("unexpected browse response: {other:?}"),
    }

    let folder = deliver_project_device_command_via_session(
        source.runtime.session_manager(),
        &ProjectDeviceCommand {
            command_id: "project-directory-create-1".to_string(),
            source_device_id: source.device_id.clone(),
            target_device_id: target.device_id.clone(),
            action: ProjectDeviceAction::CreateDirectory {
                parent: target_repo.to_string_lossy().to_string(),
                name: "new-project".to_string(),
            },
        },
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    assert!(
        matches!(folder, ProjectDeviceResponse::Directory { ref path, .. } if *path == target_repo.join("new-project"))
    );
    assert!(target_repo.join("new-project").is_dir());

    run_git(&target_repo, &["init", "-b", "main"]);
    run_git(&target_repo, &["config", "user.email", "test@example.com"]);
    run_git(&target_repo, &["config", "user.name", "Test"]);
    std::fs::write(target_repo.join("README.md"), "initial\n").unwrap();
    run_git(&target_repo, &["add", "README.md"]);
    run_git(&target_repo, &["commit", "-m", "initial"]);
    std::fs::write(target_repo.join("README.md"), "changed\n").unwrap();

    use axum::http::{Method, StatusCode};
    use rua_server::{router, AppState, ServerProfile};
    use serde_json::json;
    let state = AppState::from_tempdir(source.dir.path().join("controller"), ServerProfile::Test);
    *state.sync_device.write().unwrap() = Some(Arc::new(
        serde_json::from_value(json!({"secret_bytes":vec![1u8;32]})).unwrap(),
    ));
    *state.device_network.lock().await = Some(source.runtime.clone());
    let app = router::build_app(state.clone(), None);
    let local = json!({"device_id":source.device_id,"resource_root":target_repo});
    let remote = json!({"device_id":target.device_id,"resource_root":target_repo});
    let (status, result) = project_http(
        &app,
        Method::POST,
        "/api/projects",
        json!({"display_name":"Two devices","directories":[local,remote]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{result}");
    let project: rua_project::ProjectDescriptor =
        serde_json::from_value(result["project"].clone()).unwrap();
    let directories = &project.resources.directories;
    assert_eq!(directories.len(), 2);
    assert_eq!(directories[0].device_id, source.device_id);
    assert_eq!(directories[1].device_id, target.device_id);
    assert_eq!(directories[0].resource_root, directories[1].resource_root);
    assert_ne!(directories[0].directory_id, directories[1].directory_id);
    assert!(stores.list().unwrap().is_empty());
    stores.apply_descriptor(project.clone()).unwrap();

    let inspect = |directory: &rua_project::ProjectResourceDirectory| ProjectDeviceCommand {
        command_id: format!("inspect-{}", directory.directory_id),
        source_device_id: source.device_id.clone(),
        target_device_id: target.device_id.clone(),
        action: ProjectDeviceAction::InspectResource {
            project_id: project.project_id.clone(),
            device_id: directory.device_id.clone(),
            resource_root: directory.resource_root.clone(),
            operation:
                rua_server::services::device_command_bus::ProjectResourceOperation::GitStatus,
        },
    };
    let git = deliver_project_device_command_via_session(
        source.runtime.session_manager(),
        &inspect(&directories[1]),
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    match git {
        ProjectDeviceResponse::GitStatus { status } => {
            assert!(status.repository);
            assert_eq!(status.branch.as_deref(), Some("main"));
            assert_eq!(status.changes[0].path, "README.md");
        }
        other => panic!("unexpected Git response: {other:?}"),
    }
    assert!(deliver_project_device_command_via_session(
        source.runtime.session_manager(),
        &inspect(&directories[0]),
        Duration::from_secs(5)
    )
    .await
    .is_err());

    let uri = format!("/api/projects/{}/resources", project.project_id);
    let remote_new = json!({"device_id":target.device_id,"resource_root":target_repo.join("src")});
    let missing = target_repo.join("missing");
    let (status, error) = project_http(&app, Method::PATCH, &uri, json!({
        "expected_revision":1,"directories":[remote_new,{"device_id":target.device_id,"resource_root":missing}]
    })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
    assert!(error["detail"]
        .as_str()
        .unwrap()
        .contains(missing.to_str().unwrap()));
    assert_eq!(
        state.project_catalog.get(&project.project_id).unwrap(),
        Some(project.clone())
    );
    let mut renamed = serde_json::to_value(&directories[1]).unwrap();
    renamed["display_name"] = json!("Offline rename");
    let network = state.device_network.lock().await.take();
    let (status, error) = project_http(
        &app,
        Method::PATCH,
        &uri,
        json!({"expected_revision":1,"directories":[local,renamed,remote_new]}),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{error}");
    assert_eq!(
        state
            .project_catalog
            .get(&project.project_id)
            .unwrap()
            .as_ref(),
        Some(&project)
    );
    let (status, result) = project_http(
        &app,
        Method::PATCH,
        &uri,
        json!({"expected_revision":1,"directories":[renamed]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(
        result["project"]["resources"]["directories"],
        json!([renamed])
    );
    let (status, result) = project_http(
        &app,
        Method::PATCH,
        &uri,
        json!({"expected_revision":2,"directories":[]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(
        result["project"]["resources"],
        json!({"revision":3,"directories":[]})
    );
    *state.device_network.lock().await = network;
    let (status, result) = project_http(
        &app,
        Method::PATCH,
        &uri,
        json!({"expected_revision":3,"directories":[remote_new]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(result["project"]["resources"]["revision"], 4);
    assert_eq!(
        stores.get(&project.project_id).unwrap(),
        Some(project.clone())
    );
    assert_eq!(
        std::fs::read(target_repo.join("README.md")).unwrap(),
        b"changed\n"
    );

    let task_chat_id = "00000000-0000-4000-8000-000000000915";
    let mut header = rua_session::ChatSessionHeader::new(rua_session::ChatMetadata {
        chat_id: task_chat_id.into(),
        agent_id: "default".into(),
        title: "Remote".into(),
        created_at: chrono::Utc::now().to_rfc3339(),
        last_activity_at: chrono::Utc::now().to_rfc3339(),
        pinned: false,
        archived: false,
        preview: None,
    });
    header.project_id = Some(project.project_id.to_string());
    rua_session::WorkspaceSessionStore::new(
        rua_core::RuaRoot::new(target_data.path())
            .chat_workspace()
            .as_path(),
    )
    .save_chat_header(&header)
    .await
    .unwrap();
    let task_owner = rua_task::TaskOwner::from_address(rua_core::ChatAddress::new(task_chat_id));
    let task_id = target_task_runtime
        .create(
            rua_task::TaskSpec {
                owner: task_owner,
                instruction: "Inspect remote Project".to_string(),
                runner: rua_task::TaskRunnerSpec::Tool {
                    tool_name: "Fixture_RemoteProjectTask".to_string(),
                    arguments: serde_json::json!({}),
                },
                context_refs: Vec::new(),
                depends_on: Vec::new(),
                origin: Default::default(),
            },
            rua_task::TaskAttemptAuthorization::new("default").unwrap(),
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while target_task_runtime
            .get(
                &rua_task::TaskOwner::from_address(rua_core::ChatAddress::new(task_chat_id)),
                &task_id,
            )
            .await
            .is_ok_and(|record| record.input_capability != rua_task::TaskInputCapability::Stdin)
        {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let input = deliver_project_device_command_via_session(
        source.runtime.session_manager(),
        &ProjectDeviceCommand {
            command_id: "project-task-input-1".to_string(),
            source_device_id: source.device_id.clone(),
            target_device_id: target.device_id.clone(),
            action: ProjectDeviceAction::SendTaskInput {
                project_id: project.project_id.clone(),
                chat_id: task_chat_id.to_string(),
                task_id: task_id.clone(),
                submission: rua_task::TaskInputSubmission::new(
                    "project-task-input-submission-1",
                    rua_task::TaskInput::Text {
                        text: "remote-input".to_string(),
                    },
                )
                .unwrap(),
            },
        },
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    match input {
        ProjectDeviceResponse::TaskInput { acceptance } => {
            assert!(acceptance.accepted);
            assert_eq!(acceptance.task_id, task_id);
            assert_eq!(acceptance.input_seq, 1);
        }
        other => panic!("unexpected Task Input response: {other:?}"),
    }
    assert_eq!(
        remote_task_runner.sink.inputs.lock().await.as_slice(),
        &[rua_task::TaskInput::Text {
            text: "remote-input".to_string()
        }]
    );
    let tasks = deliver_project_device_command_via_session(
        source.runtime.session_manager(),
        &ProjectDeviceCommand {
            command_id: "project-task-list-1".to_string(),
            source_device_id: source.device_id.clone(),
            target_device_id: target.device_id.clone(),
            action: ProjectDeviceAction::ListTasks {
                project_id: project.project_id.clone(),
                chat_id: task_chat_id.to_string(),
                filter: rua_task::TaskListFilter::default(),
            },
        },
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    let tasks = match tasks {
        ProjectDeviceResponse::TaskList { tasks } => {
            serde_json::from_value::<Vec<rua_task::TaskRecord>>(tasks).unwrap()
        }
        other => panic!("unexpected Task list response: {other:?}"),
    };
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].task_id, task_id);
    assert_eq!(
        tasks[0].spec.owner.chat_address,
        rua_core::ChatAddress::new(task_chat_id)
    );

    target.runtime.shutdown().await.unwrap();
    source.runtime.shutdown().await.unwrap();
}

fn run_git(root: &std::path::Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .status()
        .unwrap();
    assert!(status.success(), "git command failed: {args:?}");
}
