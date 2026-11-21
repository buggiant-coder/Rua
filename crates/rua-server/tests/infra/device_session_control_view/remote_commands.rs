#![cfg(unix)]

use super::*;
use rua_device_network::DeviceSessionCommandChannel;
use rua_security::{ApprovalReuseMatcher, PermissionRuleEffect, PermissionRuleInput};
use rua_server::services::chat_command::{
    ChatCommandAction, ChatCommandCommand, ChatCommandContext, ChatCommandResponse,
    ChatCommandSnapshot, ChatCommandState,
};
use rua_server::services::device_command_bus::{
    deliver_chat_command_via_channel, deliver_chat_command_via_session,
};
use rua_server::services::device_read_check::{DeviceReadCheckReply, DeviceReadCheckRequest};
use serde_json::json;

struct CommandFixture {
    pair: RemoteInputConfirmationFixture,
    channel: DeviceSessionCommandChannel,
    context: ChatCommandContext,
    cwd: PathBuf,
    release: Arc<Notify>,
}

impl CommandFixture {
    async fn new(command: &str) -> Self {
        let pair = RemoteInputConfirmationFixture::new(Duration::ZERO, false).await;
        let cwd = pair
            .source_state
            .rua_root
            .canonicalize()
            .unwrap()
            .join("command-test");
        std::fs::create_dir(&cwd).unwrap();
        let arguments = json!({"device_id":pair.source.device_id,"cwd":cwd,"command":command});
        let call = ToolCall::new("target-command", "bash", arguments.clone());
        let release = Arc::new(Notify::new());
        let (address, turn, configuration_epoch, _) =
            prepare_running_file_calls(&pair, std::slice::from_ref(&call), &release).await;
        pair.source_state
            .security
            .approval_authority()
            .create_agent_rule(
                "default",
                PermissionRuleInput {
                    label: "Allow the exact target command in the connection fixture".into(),
                    enabled: true,
                    effect: PermissionRuleEffect::Allow,
                    matcher: ApprovalReuseMatcher::ToolCall {
                        tool_name: "bash".into(),
                        input_params: Some(arguments.to_string()),
                    },
                },
            )
            .await
            .unwrap();
        let context = ChatCommandContext {
            execution_id: "target-command-execution".into(),
            call: DeviceReadCheckRequest {
                tool_name: "bash".into(),
                source_device_id: pair.target.device_id.clone(),
                target_device_id: pair.source.device_id.clone(),
                agent_id: "default".into(),
                address,
                authority_epoch: turn.authority_epoch,
                configuration_epoch,
                turn_id: turn.turn_id,
                tool_call_id: call.id,
                confirmation: None,
                arguments,
            },
            extra: Default::default(),
        };
        let channel = pair
            .target
            .runtime
            .session_manager()
            .open_command_channel(&pair.source.device_id, Duration::from_secs(5))
            .await
            .unwrap();
        Self {
            pair,
            channel,
            context,
            cwd,
            release,
        }
    }

    fn command(&self, action: ChatCommandAction) -> ChatCommandCommand {
        ChatCommandCommand {
            context: self.context.clone(),
            action,
            extra: Default::default(),
        }
    }

    async fn send(&self, action: ChatCommandAction) -> ChatCommandResponse {
        deliver_chat_command_via_channel(
            &self.channel,
            &self.command(action),
            Duration::from_secs(5),
        )
        .await
        .unwrap()
    }

    async fn prepare(&self) {
        let response = self.send(ChatCommandAction::Prepare).await;
        let ChatCommandResponse::Prepared { preparation } = response else {
            panic!("{response:?}")
        };
        assert_eq!(preparation.policy, DeviceReadCheckReply::Checked);
        assert_eq!(preparation.public_cwd, self.cwd.to_str().unwrap());
    }

    async fn terminal(&self, reconnect: bool) -> ChatCommandSnapshot {
        tokio::time::timeout(Duration::from_secs(8), async {
            loop {
                let response = if reconnect {
                    deliver_chat_command_via_session(
                        self.pair.target.runtime.session_manager(),
                        &self.command(ChatCommandAction::Status),
                        Duration::from_secs(3),
                    )
                    .await
                    .unwrap()
                } else {
                    self.send(ChatCommandAction::Status).await
                };
                let snapshot = operation(response);
                if !matches!(
                    snapshot.state,
                    ChatCommandState::Prepared | ChatCommandState::Running
                ) {
                    break snapshot;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("a real command terminal result")
    }

    async fn shutdown(self) {
        self.release.notify_one();
        self.pair.shutdown().await;
    }
}

fn operation(response: ChatCommandResponse) -> ChatCommandSnapshot {
    match response {
        ChatCommandResponse::Operation { operation } => *operation,
        response => panic!("expected execution snapshot: {response:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn closing_the_owning_connection_cancels_and_reaps_the_real_target_process() {
    let f = CommandFixture::new("printf '%s' $$ > pid; sleep 60; printf survived > survived").await;
    f.prepare().await;
    assert_eq!(
        operation(f.send(ChatCommandAction::Start).await).state,
        ChatCommandState::Running
    );
    let pid: libc::pid_t = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(text) = std::fs::read_to_string(f.cwd.join("pid")) {
                if let Ok(pid) = text.parse() {
                    break pid;
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    f.pair
        .source
        .runtime
        .session_manager()
        .disconnect_peer(&f.pair.target.device_id)
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), f.channel.closed())
        .await
        .unwrap();
    let result = f.terminal(true).await;
    assert_eq!(result.state, ChatCommandState::Cancelled, "{result:?}");
    let later_adoption = deliver_chat_command_via_session(
        f.pair.target.runtime.session_manager(),
        &f.command(ChatCommandAction::Adopt),
        Duration::from_secs(3),
    )
    .await
    .unwrap_err();
    assert!(
        later_adoption.contains("Only an existing execution can be reconciled"),
        "{later_adoption}"
    );
    assert!(matches!(
        f.pair
            .source_state
            .task_runtime
            .get(
                &rua_task::TaskOwner::from_address(f.context.call.address.clone()),
                &f.context.execution_id,
            )
            .await,
        Err(rua_task::TaskError::NotFound(_))
    ));
    assert_eq!(result.result.unwrap().metadata["cancelled"], true);
    assert!(!f.cwd.join("survived").exists());
    // The terminal receipt requires a reaped process, not just a sent stop signal.
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ESRCH)
    );
    assert_eq!(
        f.pair
            .file_reads
            .command_requests
            .lock()
            .unwrap()
            .iter()
            .filter(|request| request.action == ChatCommandAction::Start)
            .count(),
        1
    );
    f.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn completed_command_survives_late_cancel_and_duplicate_start_without_a_second_process() {
    let f = CommandFixture::new("printf x >> starts; printf complete").await;
    f.prepare().await;
    f.send(ChatCommandAction::Start).await;
    let completed = f.terminal(false).await;
    assert_eq!(
        completed.state,
        ChatCommandState::Succeeded,
        "{completed:?}"
    );
    let result = completed.result.as_ref().unwrap();
    assert!(!result.is_error);
    assert!(result.text_content().contains("complete"));
    assert!(!result.metadata.contains_key("raw_capture"));
    assert!(!result.metadata.contains_key("live_output_path"));
    assert_eq!(
        result.metadata["remote_capture"]["device_id"],
        f.pair.source.device_id
    );
    assert!(result.text_content().contains("stdout capture: Device"));
    assert_eq!(
        operation(f.send(ChatCommandAction::Cancel).await),
        completed
    );
    assert_eq!(operation(f.send(ChatCommandAction::Start).await), completed);
    assert_eq!(operation(f.send(ChatCommandAction::Adopt).await), completed);
    assert!(matches!(
        f.pair
            .source_state
            .task_runtime
            .get(
                &rua_task::TaskOwner::from_address(f.context.call.address.clone()),
                &f.context.execution_id,
            )
            .await,
        Err(rua_task::TaskError::NotFound(_))
    ));
    assert_eq!(std::fs::read_to_string(f.cwd.join("starts")).unwrap(), "x");
    f.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn adopted_command_survives_lost_receipt_foreground_cancel_and_connection_close() {
    let f = CommandFixture::new("printf x >> starts; printf '%s' $$ > pid; while [ ! -e release ]; do sleep 0.05; done; printf original-complete").await;
    f.prepare().await;
    f.send(ChatCommandAction::Start).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while !f.cwd.join("pid").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    f.pair
        .file_reads
        .drop_next_command_adopt_reply
        .store(true, Ordering::SeqCst);
    assert!(deliver_chat_command_via_channel(
        &f.channel,
        &f.command(ChatCommandAction::Adopt),
        Duration::from_secs(5)
    )
    .await
    .is_err());
    let ChatCommandResponse::Task { task, reference } = f.send(ChatCommandAction::Status).await
    else {
        panic!("lost acceptance must be recoverable by the same execution identity")
    };
    assert_eq!(task.task_id, f.context.execution_id);
    assert_eq!(task.state, rua_task::TaskState::Running);
    assert!(reference.is_some());
    assert_eq!(
        task.attempts[0].device.device_id.as_deref(),
        Some(f.pair.source.device_id.as_str())
    );
    assert_eq!(
        task.attempts[0].device.public_cwd.as_deref(),
        f.cwd.to_str()
    );
    assert!(matches!(
        f.send(ChatCommandAction::Cancel).await,
        ChatCommandResponse::Task { .. }
    ));
    tokio::time::timeout(Duration::from_secs(2), async {
        while f
            .pair
            .source
            .runtime
            .session_manager()
            .diagnostics()
            .sessions
            .iter()
            .any(|session| session.active_logical_streams > 0)
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("adoption releases the target foreground connection lease");
    // Selecting another Chat Agent does not change the accepted Task owner.
    let store = WorkspaceSessionStore::new(f.pair.source_state.chat_workspace().as_path());
    let chat_id = f.context.call.address.chat_id().as_str();
    let mut changed_configuration = store
        .load_configuration_snapshot(chat_id, &f.context.call.configuration_epoch)
        .await
        .unwrap()
        .unwrap();
    changed_configuration["epoch"] = json!("changed-agent-configuration");
    changed_configuration["selected_agent_id"] = json!("changed-agent");
    changed_configuration["agent_assignment"]["agent_id"] = json!("changed-agent");
    store
        .commit_chat_agent_switch(
            chat_id,
            "default",
            Some(&f.context.call.configuration_epoch),
            "changed-agent",
            json!({"previous_agent_id":"default", "selected_agent_id":"changed-agent"}),
            &changed_configuration,
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .load_chat_header(chat_id)
            .await
            .unwrap()
            .unwrap()
            .agent_id,
        "changed-agent"
    );
    f.pair
        .source
        .runtime
        .session_manager()
        .disconnect_peer(&f.pair.target.device_id)
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), f.channel.closed())
        .await
        .unwrap();
    rua_config::atomic_write_json(&f.cwd.join("release"), &json!(true)).unwrap();
    let completed = f
        .pair
        .source_state
        .task_runtime
        .await_terminal(&task.spec.owner, &task.task_id, Duration::from_secs(8))
        .await
        .unwrap();
    assert_eq!(
        completed.state,
        rua_task::TaskState::Completed,
        "{completed:?}"
    );
    assert!(completed
        .result
        .as_ref()
        .unwrap()
        .output
        .contains("original-complete"));
    assert_eq!(std::fs::read_to_string(f.cwd.join("starts")).unwrap(), "x");
    assert_eq!(completed.attempts.len(), 1);
    changed_configuration["epoch"] = json!("retry-current-configuration");
    changed_configuration["capability_configuration"][rua_session::EXECUTION_DIRECTORY_KEY] = json!({
        "device_id":f.pair.source.device_id, "path":f.cwd.join("missing-current-chat-default"),
    });
    store
        .replace_chat_configuration(
            chat_id,
            "changed-agent-configuration",
            changed_configuration,
        )
        .await
        .unwrap();
    let resolver = rua_server::services::task_authority::ServerTaskAuthorityResolver::new(
        f.pair.source_state.rua_root.clone(),
        "desktop",
        f.pair.source_state.sync_device.clone(),
    );
    let retry_authorization = rua_task::TaskAttemptAuthorization::new("changed-agent")
        .unwrap()
        .with_configuration_epoch("retry-current-configuration".into());
    let retry_position = rua_task::TaskAuthorityResolver::accept_attempt(
        &resolver,
        &task.spec,
        &retry_authorization,
        Some(&completed.attempts[0].device),
    )
    .await
    .unwrap();
    assert_eq!(
        retry_position.public_cwd.as_deref(),
        f.cwd.to_str(),
        "Retry does not resolve the Chat's changed default directory"
    );
    std::fs::rename(&f.cwd, f.cwd.with_extension("moved")).unwrap();
    let retry = f
        .pair
        .source_state
        .task_runtime
        .retry(
            &task.spec.owner,
            &task.task_id,
            rua_task::TaskAttemptAuthorization::new("changed-agent")
                .unwrap()
                .with_configuration_epoch("retry-current-configuration".into())
                .with_origin(rua_task::TaskOrigin {
                    turn_id: Some("retry-turn".into()),
                    tool_call_id: Some("retry-call".into()),
                }),
        )
        .await
        .unwrap_err();
    assert!(
        retry.to_string().contains(f.cwd.to_str().unwrap()),
        "Retry rejects the missing original directory: {retry}"
    );
    assert_eq!(
        f.pair
            .source_state
            .task_runtime
            .get(&task.spec.owner, &task.task_id)
            .await
            .unwrap()
            .attempts
            .len(),
        1
    );
    assert_eq!(
        f.pair
            .file_reads
            .command_requests
            .lock()
            .unwrap()
            .iter()
            .filter(|call| call.action == ChatCommandAction::Start)
            .count(),
        1
    );
    f.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn task_cancel_reaps_the_original_adopted_process_and_preserves_output() {
    let f =
        CommandFixture::new("printf prefix; printf '%s' $$ > pid; sleep 60; printf late > late")
            .await;
    f.prepare().await;
    f.send(ChatCommandAction::Start).await;
    let pid: libc::pid_t = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(text) = std::fs::read_to_string(f.cwd.join("pid")) {
                if let Ok(pid) = text.parse() {
                    break pid;
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let ChatCommandResponse::Task { task, .. } = f.send(ChatCommandAction::Adopt).await else {
        panic!("expected accepted Task")
    };
    let stopped = f
        .pair
        .source_state
        .task_runtime
        .cancel(&task.spec.owner, &task.task_id)
        .await
        .unwrap();
    assert_eq!(stopped.state, rua_task::TaskState::Cancelled, "{stopped:?}");
    assert!(stopped.result.as_ref().unwrap().output.contains("prefix"));
    assert!(!f.cwd.join("late").exists());
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ESRCH)
    );
    f.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invalid_preparations_never_spawn_and_cancel_releases_the_target_connection_lease() {
    let f = CommandFixture::new("printf should-not-run > started").await;
    std::fs::remove_dir(&f.cwd).unwrap();
    assert!(deliver_chat_command_via_channel(
        &f.channel,
        &f.command(ChatCommandAction::Prepare),
        Duration::from_secs(5)
    )
    .await
    .is_err());
    std::fs::create_dir(&f.cwd).unwrap();
    let mut stale = f.command(ChatCommandAction::Prepare);
    stale.context.execution_id = "stale-configuration".into();
    stale.context.call.configuration_epoch = "stale-configuration".into();
    assert_eq!(
        deliver_chat_command_via_channel(&f.channel, &stale, Duration::from_secs(5))
            .await
            .unwrap(),
        ChatCommandResponse::Policy {
            response: DeviceReadCheckReply::Unavailable
        }
    );
    f.prepare().await;
    assert!(f
        .pair
        .source
        .runtime
        .session_manager()
        .diagnostics()
        .sessions
        .iter()
        .any(|session| session.active_logical_streams > 0));
    let cancelled = operation(f.send(ChatCommandAction::Cancel).await);
    assert_eq!(cancelled.state, ChatCommandState::Cancelled);
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if f.pair
                .source
                .runtime
                .session_manager()
                .diagnostics()
                .sessions
                .iter()
                .all(|session| session.active_logical_streams == 0)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("prepared target lease released while origin channel remains open");
    assert!(!f.channel.is_closed());
    assert_eq!(operation(f.send(ChatCommandAction::Start).await), cancelled);
    assert!(!f.cwd.join("started").exists());
    f.shutdown().await;
}
