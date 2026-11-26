use super::*;
use rua_contract::Tool;
use rua_core::tool::ToolContext;
use rua_security::{ApprovalReuseScope, SecurityVerdict, ToolCallContext};
use rua_server::services::conversation_authority::ConversationActiveTurnPhase;
use rua_server::services::device_read_check::ForegroundReadAccess;
use rua_sync::{
    evergreen_chat_adapter::EvergreenChatSyncAdapter, sync_registry::DomainSyncAdapter,
};
use serde_json::{json, Value};

pub(super) async fn request(
    state: &AppState,
    method: Method,
    path: &str,
    body: Value,
    token: Option<&str>,
) -> Value {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .extension(axum::extract::ConnectInfo(
            "127.0.0.1:15432".parse::<std::net::SocketAddr>().unwrap(),
        ))
        .header("content-type", "application/json");
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let body = if body.is_null() {
        Body::empty()
    } else {
        Body::from(body.to_string())
    };
    let response = rua_server::router::build_app(state.clone(), None)
        .oneshot(request.body(body).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap();
    assert_eq!(
        status,
        StatusCode::OK,
        "{path}: {}",
        String::from_utf8_lossy(&bytes)
    );
    serde_json::from_slice(&bytes).unwrap()
}

pub(super) async fn logged_in(state: &AppState, user: &str) -> String {
    state
        .auth
        .register_user(user, "fixture-password", None)
        .unwrap();
    request(
        state,
        Method::POST,
        "/api/auth/login",
        json!({"username":user,"password":"fixture-password"}),
        None,
    )
    .await["token"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn invocation(device: &str, address: &ChatAddress, turn: &str, call: &ToolCall) -> ToolCallContext {
    let mut ctx = ToolCallContext::new(
        "default",
        address.chat_id().as_str(),
        "read",
        call.arguments.clone(),
    )
    .with_chat_address(address.clone());
    ctx.local_device_id = Some(device.to_string());
    ctx.metadata.insert("turn_id".into(), turn.into());
    ctx.metadata
        .insert("tool_call_id".into(), call.id.clone().into());
    ctx
}

/// Device-level portable security projections. A handoff requires the target to
/// hold the Authority's current Agent/root security, which normally arrive through
/// ordinary Sync rather than a Chat snapshot.
fn sync_device_security(sender: &AppState, sender_id: &str, receiver: &AppState) {
    let agent = sender
        .rua_root
        .join("workspaces")
        .join("default")
        .join("security.json");
    if agent.is_file() {
        let target = receiver.rua_root.join("workspaces").join("default");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::copy(&agent, target.join("security.json")).expect("copy Agent security snapshot");
    }
    let root = match std::fs::read_to_string(sender.rua_root.join("config.json"))
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
    {
        Some(Value::Object(mut object)) => object.remove("security"),
        _ => None,
    };
    let Some(security) = root else { return };
    let path = receiver.rua_root.join("config.json");
    let mut config = std::fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .unwrap_or_else(|| json!({}));
    let Some(object) = config.as_object_mut() else {
        return;
    };
    if !object.contains_key("security") {
        object.insert("security".into(), security);
        rua_config::atomic_write_json(&path, &config).expect("write root security snapshot");
    }
    let _ = (sender_id, receiver);
}

async fn sync_chat(
    sender: &AppState,
    sender_id: &str,
    receiver: &AppState,
    receiver_id: &str,
    address: &ChatAddress,
) {
    // Deliver the authoritative Chat snapshots through the production Sync adapter.
    // Permission rules travel with the ordinary versioned Chat snapshots.
    let replica = EvergreenChatSyncAdapter::new(receiver.rua_root.clone(), receiver_id);
    let merge_dir = TempDir::new().unwrap();
    let merge = rua_sync::merge_store::MergeStore::open(merge_dir.path()).unwrap();
    let changes = EvergreenChatSyncAdapter::snapshot_changes_for_chat(
        sender.chat_workspace().as_path(),
        address,
        sender_id,
        rua_sync::hlc::HlcTimestamp {
            wall_ms: 1,
            counter: 0,
            device_id: [0; 8],
        },
    )
    .unwrap();
    for change in changes {
        replica.apply_change(&change, &merge).await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn after_moving_a_chat_without_synced_rules_desktop_confirmation_allows_just_that_file() {
    read_after_handoff(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_synced_desktop_reuses_the_shared_file_permission_without_another_confirmation() {
    read_after_handoff(true).await;
}

async fn read_after_handoff(desktop_was_synced: bool) {
    let fixture = RemoteInputConfirmationFixture::new(Duration::ZERO, false).await;
    let phone = &fixture.target_state;
    let desktop = &fixture.source_state;
    let phone_id = &fixture.target.device_id;
    let desktop_id = &fixture.source.device_id;
    let notes = TempDir::new().unwrap();
    let notes = notes.path().canonicalize().unwrap();
    let approved = notes.join("approved.txt");
    let sibling = notes.join("sibling.txt");
    std::fs::write(&approved, "selected on the phone\n").unwrap();
    let sibling_text = format!("sibling-{}", uuid::Uuid::new_v4());
    std::fs::write(&sibling, &sibling_text).unwrap();
    let mut policy = phone.security.config_store().load("default").unwrap();
    policy.tool_guard.approval_mode = "ask_first".into();
    phone
        .security
        .save_and_invalidate("default", &policy)
        .unwrap();
    let local = ToolCall::new("phone-initial-read", "read", json!({"file_path":approved}));
    let phone_finish = Arc::new(Notify::new());
    let (address, first, phone_request) =
        prepare_running_file_calls(&fixture, std::slice::from_ref(&local), &phone_finish).await;
    let ctx = invocation(phone_id, &address, &first.turn_id, &local)
        .with_security_snapshot(Arc::new(policy));
    let SecurityVerdict::RequiresApproval { request_id, .. } =
        phone.security.enforce(&ctx, &[]).await
    else {
        panic!("the phone should offer an approval for its first read");
    };
    let phone_token = logged_in(phone, "phone-user").await;
    let card = phone
        .security
        .approval_authority()
        .request(&request_id)
        .await
        .unwrap();
    let option = card
        .reuse_options
        .iter()
        .find(|o| o.scope == ApprovalReuseScope::Chat)
        .unwrap();
    let decided = request(
        phone,
        Method::POST,
        &format!(
            "/api/chats/{}/approvals/{request_id}/decide",
            fixture.chat_id
        ),
        json!({"decision":"allow_chat","decided_by":"webui","matcher":option.matcher}),
        Some(&phone_token),
    )
    .await;
    assert_eq!(
        decided["authenticated_user_decision"],
        json!({"user_id":"phone-user","decision":"allow_chat"})
    );
    let local_result = rua_tool::ReadFileTool
        .call(
            local.arguments.clone(),
            ToolContext {
                working_dir: notes.clone(),
                local_device_id: Some(phone_id.clone()),
                metadata: [("tool_call_id".into(), json!(local.id))].into(),
                tool_result_store_dir: Some(
                    rua_core::ChatStore::resolve(&phone.rua_root, address.clone())
                        .evergreen_chat_root()
                        .join("tool_call_logs"),
                ),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(
        !local_result.is_error
            && local_result
                .text_content()
                .contains("selected on the phone")
    );
    // The scripted runtime issued the call; commit the actual tool result through
    // the ordinary runtime writer before asking the other device to adopt the Chat.
    let events = [
        RuntimeEvent::ToolCall { call: local },
        RuntimeEvent::ToolResult {
            result: local_result,
        },
        RuntimeEvent::TextDelta {
            delta: "Read the selected file.".into(),
            message_id: Some(uuid::Uuid::new_v4().to_string().into()),
        },
        RuntimeEvent::End {
            finish_reason: Some("stop".into()),
        },
    ];
    let mut saved = rua_agent::turn_persistence::persist_runtime_stream(
        &phone_request,
        Box::pin(futures::stream::iter(events.into_iter().map(Ok))),
    );
    while let Some(event) = saved.next().await {
        event.unwrap();
    }
    phone_finish.notify_one();
    tokio::time::timeout(Duration::from_secs(8), async {
        while phone
            .conversation_authority
            .active_turn(&address)
            .await
            .is_some()
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the phone Turn completed");
    *phone.sync_transfer_session_manager.write().unwrap() =
        Some(fixture.target.runtime.session_manager().clone());
    for (runtime, state) in [(&fixture.target, phone), (&fixture.source, desktop)] {
        runtime
            .runtime
            .session_manager()
            .set_bulk_transfer_handler(Arc::new(
                SyncTransferBulkHandler::new(runtime.device_id.clone())
                    .with_chat_media_root(state.rua_root.clone()),
            ));
    }
    // Device-level security always arrives through ordinary Sync, independently of
    // whether this Chat's rules were already replicated.
    sync_device_security(phone, phone_id, desktop);
    if desktop_was_synced {
        sync_chat(phone, phone_id, desktop, desktop_id, &address).await;
    }
    let moved = request(
        phone,
        Method::PUT,
        &format!("/api/chats/{}/authority-placement", fixture.chat_id),
        json!({"device_id":desktop_id, "model":{"provider_id":"fixture","model":"model-a"}}),
        Some(&phone_token),
    )
    .await;
    assert_eq!(
        moved["meta"]["authority_placement"]["device_id"],
        json!(desktop_id)
    );
    let desktop_view = request(
        desktop,
        Method::GET,
        &format!("/api/chats/{}/metadata", fixture.chat_id),
        Value::Null,
        None,
    )
    .await;
    assert_eq!(
        desktop_view["meta"]["authority_placement"],
        moved["meta"]["authority_placement"]
    );
    let cwd = request(
        desktop,
        Method::PATCH,
        &format!("/api/chats/{}/execution-directory", fixture.chat_id),
        json!({"device_id": desktop_id,"path": fixture._source_dir.path()}),
        None,
    )
    .await;
    assert_eq!(cwd["device_id"], json!(desktop_id));
    sync_chat(desktop, desktop_id, phone, phone_id, &address).await;

    let current_text = format!("phone-current-{}", uuid::Uuid::new_v4());
    std::fs::write(&approved, &current_text).unwrap();
    let requested = ToolCall::new(
        "desktop-approved",
        "read",
        json!({"device_id":phone_id,"file_path":approved}),
    );
    let other = ToolCall::new(
        "desktop-other",
        "read",
        json!({"device_id":phone_id,"file_path":sibling}),
    );
    #[cfg(unix)]
    let replaced = ToolCall::new("desktop-replaced-file", "read", requested.arguments.clone());
    let calls = vec![
        requested.clone(),
        other.clone(),
        #[cfg(unix)]
        replaced.clone(),
    ];
    let desktop_finish = Arc::new(Notify::new());
    desktop
        .registry
        .get_or_create("default")
        .await
        .unwrap()
        .set_react_runtime(Arc::new(CapturingAgentRuntime {
            held_tool_calls: Some((calls, desktop_finish.clone())),
            ..Default::default()
        }))
        .await;
    let accepted = request(desktop, Method::POST, &format!("/api/chats/{}/console/submit",fixture.chat_id),
        json!({"agent_id":"default","message":"Read the named phone file","submission_id":uuid::Uuid::new_v4()}), None).await;
    assert_eq!(accepted["accepted"], true, "{accepted}");
    let turn = tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            if let Some(turn) = desktop.conversation_authority.active_turn(&address).await {
                if turn.phase == ConversationActiveTurnPhase::RunningTool {
                    break turn;
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the same Chat continued on the desktop");
    let header = WorkspaceSessionStore::new(desktop.chat_workspace().as_path())
        .load_chat_header(&fixture.chat_id)
        .await
        .unwrap()
        .unwrap();
    let approvals = desktop.security.approval_authority();
    let ctx = |call: &ToolCall| ToolContext {
        agent_id: "default".into(),
        session_id: fixture.chat_id.clone(),
        local_device_id: Some(desktop_id.clone()),
        tool_result_store_dir: Some(
            rua_core::ChatStore::resolve(&desktop.rua_root, address.clone())
                .evergreen_chat_root()
                .join("tool_call_logs"),
        ),
        working_dir: fixture._source_dir.path().to_owned(),
        device_read_access: Some(Arc::new(ForegroundReadAccess {
            authority: Arc::downgrade(&desktop.conversation_authority),
            approvals: Arc::downgrade(approvals),
            network: Arc::downgrade(&desktop.device_network),
        })),
        metadata: [
            (
                rua_agent::hooks::SECURITY_CHAT_ADDRESS_METADATA_KEY.into(),
                json!(address),
            ),
            ("turn_id".into(), json!(turn.turn_id)),
            ("tool_call_id".into(), json!(call.id)),
        ]
        .into(),
        ..Default::default()
    };
    let captured =
        serde_json::from_value(header.extra["hook_configuration"]["security_config"].clone())
            .unwrap();
    let source_call = invocation(desktop_id, &address, &turn.turn_id, &requested)
        .with_security_snapshot(Arc::new(captured));
    let source_verdict = desktop.security.enforce(&source_call, &[]).await;
    if !desktop_was_synced {
        let SecurityVerdict::RequiresApproval { request_id, .. } = source_verdict else {
            panic!("expected desktop Ask First before permission Sync: {source_verdict:?}");
        };
        let desktop_token = logged_in(desktop, "desktop-user").await;
        let decision = request(
            desktop,
            Method::POST,
            &format!(
                "/api/chats/{}/approvals/{request_id}/decide",
                fixture.chat_id
            ),
            json!({"decision":"run_once","decided_by":"webui"}),
            Some(&desktop_token),
        )
        .await;
        assert_eq!(
            decision["authenticated_user_decision"],
            json!({"user_id":"desktop-user","decision":"run_once"})
        );
    } else {
        assert!(
            source_verdict.is_allow(),
            "source policy: {source_verdict:?}"
        );
    }
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        rua_tool::ReadFileTool.call(requested.arguments.clone(), ctx(&requested)),
    )
    .await
    .expect("reusing the file grant requires no new approval")
    .unwrap();
    assert!(
        !result.is_error && result.text_content().contains(&current_text),
        "{}",
        result.text_content()
    );
    assert_eq!(
        result.metadata["file_location"]["device_id"],
        json!(phone_id)
    );
    assert_eq!(result.metadata["file_location"]["dispatched"], true);
    assert!(approvals.snapshot_at(&address).await.pending.is_empty());

    let mut pending = approvals.subscribe();
    let operation = rua_tool::ReadFileTool.call(other.arguments.clone(), ctx(&other));
    tokio::pin!(operation);
    let result = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::select! {
            result = &mut operation => result.unwrap(),
            _ = async {
                loop {
                    let card = pending.recv().await.unwrap().request;
                    if card.tool_call_id == other.id {
                        let decision = rua_security::ApprovalDecision::Reject { decided_by: "webui".into() };
                        approvals.decide(&card.id, decision).await.unwrap();
                        break;
                    }
                }
            } => operation.await.unwrap(),
        }
    }).await.unwrap();
    assert!(result.is_error);
    assert!(!result.text_content().contains(&sibling_text));
    assert_eq!(
        phone
            .security
            .approval_authority()
            .snapshot_at(&address)
            .await
            .resolved
            .len(),
        1
    );
    #[cfg(unix)]
    {
        std::fs::remove_file(&approved).unwrap();
        std::os::unix::fs::symlink(&sibling, &approved).unwrap();
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            rua_tool::ReadFileTool.call(replaced.arguments.clone(), ctx(&replaced)),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(result.is_error);
        assert!(!result.text_content().contains(&sibling_text));
    }
    desktop_finish.notify_one();
    fixture.shutdown().await;
}
