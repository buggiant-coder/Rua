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
    assert_eq!(std::fs::read_to_string(f.cwd.join("starts")).unwrap(), "x");
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
