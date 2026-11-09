use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use rua_device_network::device_list::{DeviceEntry, DeviceList};
use rua_device_network::{
    BoundDeviceNetworkMembership, DeviceNetworkBootstrap, DeviceNetworkIdentity,
    DeviceNetworkPreset, DeviceNetworkRuntime, DevicePeerDirectory, DeviceSessionCommandHandler,
    DeviceSessionConfig,
};
use rua_server::services::device_command_bus::{
    deliver_project_device_command_via_session, DeviceCommandHandler, ProjectDeviceAction,
    ProjectDeviceCommand, ProjectDeviceResponse,
};
use rua_server::sync_publication::SyncPublicationRuntime;
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
        serde_json::to_vec_pretty(&DeviceList { devices: peers }).expect("serialize peers"),
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn remote_browse_and_create_execute_on_target_device() {
    let network_secret = [44; 32];
    let source = build_peer(1, network_secret, Vec::new(), None).await;
    let target_data = tempfile::tempdir().unwrap();
    let target_repo = target_data.path().join("remote-repo");
    std::fs::create_dir_all(target_repo.join("src")).unwrap();
    let stores = rua_project::ProjectStores::open(target_data.path().join("projects")).unwrap();
    let target_device_id =
        hex::encode(&iroh::SecretKey::from_bytes(&[2; 32]).public().as_bytes()[..8]);
    let publication = SyncPublicationRuntime::new(
        target_data.path().join("sync"),
        Arc::new(AtomicBool::new(false)),
        Arc::new(tokio::sync::Mutex::new(None)),
        Arc::new(RwLock::new(None)),
        Arc::new(RwLock::new(None)),
        None,
        Arc::new(tokio::sync::RwLock::new(None)),
    );
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
        .with_project_runtime(
            Arc::clone(&stores.catalog),
            Arc::clone(&stores.bindings),
            publication,
        )
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

    let create = deliver_project_device_command_via_session(
        source.runtime.session_manager(),
        &ProjectDeviceCommand {
            command_id: "project-create-1".to_string(),
            source_device_id: source.device_id.clone(),
            target_device_id: target.device_id.clone(),
            action: ProjectDeviceAction::Create {
                resource_root: target_repo.to_string_lossy().to_string(),
                display_name: Some("Remote repo".to_string()),
            },
        },
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    let project = match create {
        ProjectDeviceResponse::Created { outcome } => outcome.project,
        other => panic!("unexpected create response: {other:?}"),
    };
    assert_eq!(stores.catalog.list().unwrap().len(), 1);
    assert_eq!(
        stores
            .bindings
            .get(&project.project_id)
            .unwrap()
            .unwrap()
            .primary_root(),
        target_repo
    );

    let git = deliver_project_device_command_via_session(
        source.runtime.session_manager(),
        &ProjectDeviceCommand {
            command_id: "project-git-1".to_string(),
            source_device_id: source.device_id.clone(),
            target_device_id: target.device_id.clone(),
            action: ProjectDeviceAction::InspectGit {
                project_id: project.project_id.clone(),
            },
        },
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

    let duplicate = deliver_project_device_command_via_session(
        source.runtime.session_manager(),
        &ProjectDeviceCommand {
            command_id: "project-create-2".to_string(),
            source_device_id: source.device_id.clone(),
            target_device_id: target.device_id.clone(),
            action: ProjectDeviceAction::Create {
                resource_root: target_repo.to_string_lossy().to_string(),
                display_name: Some("Duplicate".to_string()),
            },
        },
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    match duplicate {
        ProjectDeviceResponse::Created { outcome } => {
            assert_ne!(outcome.project.project_id, project.project_id);
            assert_eq!(outcome.overlap_warnings[0].project_id, project.project_id);
            assert_eq!(
                outcome.overlap_warnings[0].relation,
                rua_project::ProjectOverlapRelation::SharedDirectory
            );
        }
        other => panic!("unexpected shared-directory response: {other:?}"),
    }

    let task_owner = rua_task::TaskOwner::from_address(rua_core::ChatAddress::project(
        project.project_id.clone(),
        "project-task-chat",
    ));
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
                &rua_task::TaskOwner::from_address(rua_core::ChatAddress::project(
                    project.project_id.clone(),
                    "project-task-chat",
                )),
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
                chat_id: "project-task-chat".to_string(),
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
                chat_id: "project-task-chat".to_string(),
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
        rua_core::ChatAddress::project(project.project_id.clone(), "project-task-chat")
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
