use super::*;
use rua_server::services::chat_transfer::{
    ChatTransferAction, ChatTransferCommand, ChatTransferContext, ChatTransferResponse,
    PreparedChatReceive,
};
use rua_server::services::device_read_check::DeviceReadCheckRequest;
use rua_server::services::sync_transfer::{TransferJobStatus, TransferManifest};

fn command(context: &ChatTransferContext, action: ChatTransferAction) -> ChatTransferCommand {
    ChatTransferCommand {
        context: context.clone(),
        action,
        extra: Default::default(),
    }
}

async fn prepared(
    f: &Fixture,
) -> (
    ChatTransferContext,
    TransferManifest,
    Box<PreparedChatReceive>,
    std::path::PathBuf,
) {
    let source = f.app.state.rua_root.join("selected.txt");
    let destination = f.app.state.rua_root.join("received");
    std::fs::write(&source, "original source").unwrap();
    std::fs::create_dir(&destination).unwrap();
    let context = ChatTransferContext {
        operation_id: "participant-transfer".into(),
        call: DeviceReadCheckRequest {
            tool_name: "sync_send".into(),
            source_device_id: AUTHORITY.into(),
            target_device_id: f.device.clone(),
            agent_id: AGENT.into(),
            address: f.address.clone(),
            authority_epoch: f.epoch,
            turn_id: "turn-1".into(),
            tool_call_id: "tool-1".into(),
            confirmation: None,
            arguments: json!({"source_device_id":f.device,"source_paths":[source],
                "target_device_id":f.device,"target_dir":destination,"conflict":"rename"}),
        },
        extra: [("future_context_field".into(), json!(true))].into(),
    };
    let service = &f.app.state.chat_transfer;
    let ChatTransferResponse::SourcePlan { manifest, paths } = service
        .handle(command(&context, ChatTransferAction::SourcePlan))
        .await
        .unwrap()
    else {
        panic!("source metadata should be authorized");
    };
    assert_eq!(
        paths,
        vec![source.canonicalize().unwrap().to_str().unwrap().to_owned()]
    );
    assert!(manifest.entries.iter().all(|entry| entry.sha256.is_empty()));
    let ChatTransferResponse::Prepared { prepared } = service
        .handle(command(
            &context,
            ChatTransferAction::PrepareReceive {
                manifest: manifest.clone(),
            },
        ))
        .await
        .unwrap()
    else {
        panic!("target metadata should be authorized");
    };
    assert_eq!(
        std::fs::read_dir(&destination).unwrap().count(),
        0,
        "prepare must not create temporary files or content"
    );
    (context, manifest, prepared, source)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn start_rejects_changed_source_or_destination_without_publishing_bytes() {
    let f = Fixture::materialize(build_agent().await).await;
    let (context, manifest, destination, source) = prepared(&f).await;
    let service = &f.app.state.chat_transfer;
    let mut changed_destination = destination.clone();
    changed_destination.context.call.tool_call_id = "another-tool".into();
    let error = service
        .handle(command(
            &context,
            ChatTransferAction::Start {
                manifest: manifest.clone(),
                destination: changed_destination,
            },
        ))
        .await
        .unwrap_err();
    assert!(error.contains("original Chat invocation"), "{error}");
    std::fs::write(
        &source,
        "source changed after metadata and Agent instructions",
    )
    .unwrap();
    let error = service
        .handle(command(
            &context,
            ChatTransferAction::Start {
                manifest,
                destination: destination.clone(),
            },
        ))
        .await
        .unwrap_err();
    assert!(error.contains("source files changed"), "{error}");
    assert_eq!(
        std::fs::read_dir(&destination.target_dir).unwrap().count(),
        0
    );
    assert!(service
        .handle(command(&context, ChatTransferAction::Status))
        .await
        .is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_keeps_original_invocation_bound_after_chat_configuration_revocation() {
    let f = Fixture::materialize(build_agent().await).await;
    let (context, manifest, destination, _) = prepared(&f).await;
    let service = &f.app.state.chat_transfer;
    let store = rua_session::WorkspaceSessionStore::new(f.app.state.chat_workspace().as_path());
    let mut header = store
        .load_chat_header(f.address.chat_id().as_str())
        .await
        .unwrap()
        .unwrap();
    header.metadata.archived = true;
    store.save_chat_header(&header).await.unwrap();
    assert!(matches!(
        service
            .handle(command(&context, ChatTransferAction::CancelReceive))
            .await
            .unwrap(),
        ChatTransferResponse::ReceiveCancelled
    ));
    let ChatTransferResponse::Operation { operation } = service
        .handle(command(&context, ChatTransferAction::Cancel))
        .await
        .unwrap()
    else {
        panic!("cancel snapshot");
    };
    assert_eq!(operation.status, TransferJobStatus::Cancelled);
    let mut changed = context.clone();
    changed.call.arguments["target_dir"] = json!(f.app.state.rua_root);
    assert!(service
        .handle(command(&changed, ChatTransferAction::Cancel))
        .await
        .is_err());
    assert!(matches!(
        service
            .handle(command(
                &context,
                ChatTransferAction::Start {
                    manifest: manifest.clone(),
                    destination: destination.clone()
                }
            ))
            .await
            .unwrap(),
        ChatTransferResponse::Policy { .. }
    ));
    assert_eq!(
        std::fs::read_dir(&destination.target_dir).unwrap().count(),
        0
    );
    header.metadata.archived = false;
    store.save_chat_header(&header).await.unwrap();
    // Cancellation-before-Start is a tombstone, so restoring the Chat does not replay IO.
    let ChatTransferResponse::Operation { operation } = service
        .handle(command(
            &context,
            ChatTransferAction::Start {
                manifest,
                destination: destination.clone(),
            },
        ))
        .await
        .unwrap()
    else {
        panic!("cancel snapshot");
    };
    assert_eq!(operation.status, TransferJobStatus::Cancelled);
    assert_eq!(
        std::fs::read_dir(&destination.target_dir).unwrap().count(),
        0
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn receive_rechecks_the_original_directory_alias_for_conflict_renamed_files() {
    use rua_server::services::chat_transfer::{
        ChatTransferAuthorization, RuntimeChatTransferAuthorization,
    };
    let f = Fixture::materialize(build_agent().await).await;
    let (mut context, _, destination, _) = prepared(&f).await;
    let alias = f.app.state.rua_root.join("destination-alias");
    std::os::unix::fs::symlink(&destination.target_dir, &alias).unwrap();
    context.call.arguments["target_dir"] = json!(alias);
    let native = destination.target_dir.join("selected (1).txt");
    let authorization = RuntimeChatTransferAuthorization {
        rua_root: f.app.state.rua_root.clone(),
        target: std::sync::Arc::new(f.context.clone()),
    };
    authorization
        .check_receive(&context, &native, true)
        .await
        .unwrap();
    f.app
        .state
        .security
        .approval_authority()
        .create_agent_rule(
            AGENT,
            PermissionRuleInput {
                label: "Deny renamed alias destination".into(),
                enabled: true,
                effect: PermissionRuleEffect::Deny,
                matcher: ApprovalReuseMatcher::ToolCall {
                    tool_name: "edit".into(),
                    input_params: Some(
                        json!({"device_id":f.device,"path":alias.join("selected (1).txt")})
                            .to_string(),
                    ),
                },
            },
        )
        .await
        .unwrap();
    assert!(authorization
        .check_receive(&context, &native, true)
        .await
        .is_err());
    assert!(!native.exists());
}
