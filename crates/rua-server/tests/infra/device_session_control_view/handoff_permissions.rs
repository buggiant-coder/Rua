use super::handoff::{Peer, PersistingCaptureRuntime};
use super::*;
use rua_security::{ApprovalDecision, ApprovalReuseScope, SecurityVerdict, ToolCallContext};
use rua_sync::identity::NetworkSecret;
use serde_json::{json, Value};

// Run the real Tool Guard against the actual first Turn's captured security
// snapshot, while the model/stream remains deterministic and makes no file writes.
struct PermissionRuntime {
    inner: PersistingCaptureRuntime,
    security: Arc<rua_security::SecurityModule>,
    device_id: String,
    chat: String,
    arguments: Value,
    verdicts: Arc<tokio::sync::Mutex<Vec<SecurityVerdict>>>,
}

#[async_trait]
impl AgentRuntime for PermissionRuntime {
    async fn stream(
        &self,
        req: AgentRequest,
    ) -> rua_core::Result<Pin<Box<dyn futures::Stream<Item = Result<RuntimeEvent, RuaError>> + Send>>>
    {
        let snapshot = serde_json::from_value(
            req.metadata[rua_security::models::EFFECTIVE_SECURITY_CONFIG_METADATA_KEY].clone(),
        )
        .expect("first Turn carries its actual security snapshot");
        let mut ctx = ToolCallContext::new(
            req.agent_id.as_str(),
            req.session_id.as_str(),
            "read",
            self.arguments.clone(),
        )
        .with_chat_address(ChatAddress::new(&self.chat))
        .with_security_snapshot(Arc::new(snapshot));
        ctx.local_device_id = Some(self.device_id.clone());
        ctx.working_dir = req.working_dir().unwrap().to_string_lossy().into_owned();
        ctx.metadata = req.metadata.clone();
        assert!(ctx.metadata["turn_id"].as_str().is_some());
        ctx.metadata.insert(
            "tool_call_id".into(),
            json!(uuid::Uuid::new_v4().to_string()),
        );
        self.verdicts
            .lock()
            .await
            .push(self.security.enforce(&ctx, &[]).await);
        self.inner.stream(req).await
    }

    async fn cancel(&self, session_id: &rua_core::id::SessionId) -> rua_core::Result<()> {
        self.inner.cancel(session_id).await
    }

    async fn warmup(&self, context: RuntimeWarmupContext) -> rua_core::Result<()> {
        self.inner.warmup(context).await
    }

    async fn shutdown(&self) -> rua_core::Result<()> {
        self.inner.shutdown().await
    }
}

async fn install_permission_runtime(
    peer: &Peer,
    chat: &str,
    arguments: &Value,
) -> Arc<tokio::sync::Mutex<Vec<SecurityVerdict>>> {
    let verdicts = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    peer.state
        .registry
        .get_or_create("default")
        .await
        .unwrap()
        .set_react_runtime(Arc::new(PermissionRuntime {
            inner: PersistingCaptureRuntime(peer.captured.clone()),
            security: peer.state.security.clone(),
            device_id: peer.device_id.clone(),
            chat: chat.into(),
            arguments: arguments.clone(),
            verdicts: verdicts.clone(),
        }))
        .await;
    verdicts
}

async fn handoff_while_settings_apply_is_blocked(source: &Peer, target: &Peer, chat: &str) {
    let old = source.placement(chat).await;
    let guard = target.state.inbound_apply_lock.lock().await;
    let mut handoff = Box::pin(source.request(
        Method::PUT,
        chat,
        "/authority-placement",
        json!({"device_id":target.device_id}),
    ));
    // Poll the real request while the target's inbound apply lock is held. Its
    // normal transport and command handler remain live; only applying is delayed.
    tokio::select! {
        result = &mut handoff => panic!("handoff finished without applying current permissions: {result:?}"),
        () = async {
            tokio::time::timeout(Duration::from_secs(12), async {
                loop {
                    if source.store().load_chat_handoff(chat).await.unwrap().is_some_and(|record|
                        record.phase == rua_session::chat_handoff::ChatHandoffPhase::Prepared) {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }).await.expect("source durably prepares before waiting on target settings");
            tokio::time::sleep(Duration::from_millis(250)).await;
        } => {}
    }
    assert_eq!(target.placement(chat).await, old);
    assert!(target
        .state
        .conversation_authority
        .active_turn(&ChatAddress::new(chat))
        .await
        .is_none());
    // A first input cannot bypass the durable preparation fence either.
    let (status, response) = source
        .request(
            Method::POST,
            chat,
            "/console/submit",
            json!({
                "message":"must wait for permissions", "agent_id":"default",
                "submission_id":uuid::Uuid::new_v4().to_string(),
                "model":{"provider_id":"fixture","model":"available"},
            }),
        )
        .await;
    assert!(
        !status.is_success() || response["accepted"] != true,
        "{response}"
    );
    drop(guard);
    let (status, response) = handoff.await;
    assert_eq!(status, StatusCode::OK, "{response}");
    assert_eq!(target.placement(chat).await.device_id, target.device_id);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn handoff_waits_for_applied_chat_grant_and_revocation_before_first_turn() {
    let secret = NetworkSecret::generate();
    let source = Peer::new(&secret).await;
    let target = Peer::new(&secret).await;
    source.pair(&[&target]);
    target.pair(&[&source]);
    for peer in [&source, &target] {
        let mut config = peer.state.security.config_store().load("default").unwrap();
        config.tool_guard.approval_mode = "ask_first".into();
        config.extra.remove("permission_rules");
        peer.state
            .security
            .save_and_invalidate("default", &config)
            .unwrap();
    }
    let chat = uuid::Uuid::new_v4().to_string();
    let address = ChatAddress::new(&chat);
    source.submit(&chat, "original writer").await;
    target.synchronize(&source).await;
    let arguments =
        json!({"device_id":target.device_id, "file_path":target.state.rua_root.join("photo.jpg")});
    let source_verdicts = install_permission_runtime(&source, &chat, &arguments).await;
    let target_verdicts = install_permission_runtime(&target, &chat, &arguments).await;
    let authority = source.state.security.approval_authority();
    let card = authority
        .create_request_for_device(
            ApprovalRequestInput {
                agent_id: "default".into(),
                owner: rua_security::ApprovalOwner::chat(address.clone()),
                turn_id: "user-authorizes-photo".into(),
                tool_call_id: "authorize-photo".into(),
                tool_name: "read".into(),
                trigger: ApprovalTrigger::approval_mode("Ask first"),
                tool_call: ToolCallSnapshot {
                    display: "Read the target photo".into(),
                    args: arguments.clone(),
                },
            },
            Some(&source.device_id),
        )
        .await;
    let matcher = card
        .reuse_options
        .iter()
        .find(|option| option.scope == ApprovalReuseScope::Chat)
        .expect("existing Chat permission option")
        .matcher
        .clone();
    authority
        .decide_for_authenticated_user(
            "default",
            &card.id,
            ApprovalDecision::AllowChat {
                decided_by: "webui".into(),
                matcher,
            },
            "fixture-user",
        )
        .await
        .unwrap();
    assert_eq!(authority.list_chat_rules(&address).await.len(), 1);
    assert!(target
        .state
        .security
        .approval_authority()
        .list_chat_rules(&address)
        .await
        .is_empty());

    handoff_while_settings_apply_is_blocked(&source, &target, &chat).await;
    assert!(target_verdicts.lock().await.is_empty());
    target
        .submit(&chat, "first Turn uses the granted photo")
        .await;
    assert!(matches!(
        target_verdicts.lock().await.as_slice(),
        [SecurityVerdict::Allow]
    ));
    let copied = target
        .state
        .security
        .approval_authority()
        .list_chat_rules(&address)
        .await;
    assert_eq!(copied.len(), 1);
    assert_eq!(copied[0].source_approval_id, card.id);

    // C is now the writer. Revoke through the same file-backed permission store,
    // then return to A while A still has the earlier grant in its applied state.
    target
        .state
        .security
        .approval_authority()
        .delete_chat_rules(&address)
        .await
        .unwrap();
    assert_eq!(authority.list_chat_rules(&address).await.len(), 1);
    handoff_while_settings_apply_is_blocked(&target, &source, &chat).await;
    assert!(source_verdicts.lock().await.is_empty());
    source.submit(&chat, "first Turn after revocation").await;
    let request_id = match source_verdicts.lock().await.as_slice() {
        [SecurityVerdict::RequiresApproval { request_id, .. }] => request_id.clone(),
        other => panic!("revoked permission cannot authorize the first Turn: {other:?}"),
    };
    assert!(authority.list_chat_rules(&address).await.is_empty());
    authority
        .decide(
            &request_id,
            ApprovalDecision::Reject {
                decided_by: "webui".into(),
            },
        )
        .await
        .unwrap();
    target.close().await;
    source.close().await;
}
