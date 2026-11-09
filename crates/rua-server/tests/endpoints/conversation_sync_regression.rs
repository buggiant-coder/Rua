use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use futures::StreamExt;
use http_body_util::BodyExt;
use rua_contract::agent::{AgentRequest, AgentRuntime, RuntimeWarmupContext};
use rua_core::{ChatAddress, RuaError, RuntimeEvent};
use rua_session::{NewConversationMessage, NewConversationTurn, TurnLifecycle};
use serde_json::{json, Value};
use std::pin::Pin;
use tempfile::TempDir;
use tower::ServiceExt;

use rua_server::endpoints;
use rua_server::middleware::agent_context_middleware;
use rua_server::services::chat_index;
use rua_server::services::console_chat::materialize_targeted_chat;
use rua_server::services::conversation_authority_placement::{
    ConversationAuthorityEpoch, ConversationAuthorityPlacement,
};
use rua_server::AppState;

fn running_cancel_rx() -> tokio::sync::watch::Receiver<rua_core::RunControl> {
    tokio::sync::watch::channel(rua_core::RunControl::Running).1
}

struct SyncFailingRuntime;
struct SyncStartupFailingRuntime;
struct SyncOkRuntime;

#[async_trait::async_trait]
impl AgentRuntime for SyncFailingRuntime {
    async fn stream(
        &self,
        _req: AgentRequest,
    ) -> rua_core::Result<
        Pin<Box<dyn futures::Stream<Item = std::result::Result<RuntimeEvent, RuaError>> + Send>>,
    > {
        Ok(Box::pin(futures::stream::iter(vec![Err(
            RuaError::Provider {
                provider: "sync-test".to_string(),
                message: "provider disconnected".to_string(),
                retryable: false,
            },
        )])))
    }

    async fn cancel(&self, _session_id: &rua_core::id::SessionId) -> rua_core::Result<()> {
        Ok(())
    }

    async fn warmup(&self, _context: RuntimeWarmupContext) -> rua_core::Result<()> {
        Ok(())
    }

    async fn shutdown(&self) -> rua_core::Result<()> {
        Ok(())
    }
}

#[async_trait::async_trait]
impl AgentRuntime for SyncStartupFailingRuntime {
    async fn stream(
        &self,
        _req: AgentRequest,
    ) -> rua_core::Result<
        Pin<Box<dyn futures::Stream<Item = std::result::Result<RuntimeEvent, RuaError>> + Send>>,
    > {
        Err(RuaError::Provider {
            provider: "sync-test".to_string(),
            message: "provider failed before stream".to_string(),
            retryable: false,
        })
    }

    async fn cancel(&self, _session_id: &rua_core::id::SessionId) -> rua_core::Result<()> {
        Ok(())
    }

    async fn warmup(&self, _context: RuntimeWarmupContext) -> rua_core::Result<()> {
        Ok(())
    }

    async fn shutdown(&self) -> rua_core::Result<()> {
        Ok(())
    }
}

#[async_trait::async_trait]
impl AgentRuntime for SyncOkRuntime {
    async fn stream(
        &self,
        req: AgentRequest,
    ) -> rua_core::Result<
        Pin<Box<dyn futures::Stream<Item = std::result::Result<RuntimeEvent, RuaError>> + Send>>,
    > {
        Ok(Box::pin(futures::stream::iter(vec![
            Ok(RuntimeEvent::Start {
                session_id: req.session_id.clone(),
                agent_id: Some(req.agent_id),
            }),
            Ok(RuntimeEvent::TextDelta {
                delta: "ok".to_string(),
                message_id: None,
            }),
            Ok(RuntimeEvent::End {
                finish_reason: Some("stop".to_string()),
            }),
        ])))
    }

    async fn cancel(&self, _session_id: &rua_core::id::SessionId) -> rua_core::Result<()> {
        Ok(())
    }

    async fn warmup(&self, _context: RuntimeWarmupContext) -> rua_core::Result<()> {
        Ok(())
    }

    async fn shutdown(&self) -> rua_core::Result<()> {
        Ok(())
    }
}

fn build_app() -> (Router, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join("workspaces/default")).expect("seed workspace");

    let state = AppState::from_tempdir(tmp.path().to_path_buf(), rua_server::ServerProfile::Test);
    assert!(
        !state
            .sync_enabled
            .load(std::sync::atomic::Ordering::Relaxed),
        "sync should be disabled for regression test"
    );

    let app = Router::new()
        .merge(endpoints::chats::root_router())
        .merge(endpoints::console::agent_scoped_router())
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            agent_context_middleware,
        ))
        .with_state(state);
    (app, tmp)
}

fn build_sync_enabled_app() -> (Router, AppState, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    let ws = tmp.path().join("workspaces/default");
    std::fs::create_dir_all(&ws).expect("seed workspace");
    std::fs::write(
        ws.join("agent.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "id": "default",
            "name": "Default",
            "description": "",
            "workspace_dir": ws,
            "language": "",
            "system_prompt_files": []
        }))
        .expect("agent json"),
    )
    .expect("seed agent");
    rua_config::atomic_write_json(&tmp.path().join("config.json"), &serde_json::json!({
        "agents": {"profiles": {"default": {"id":"default", "workspace_dir":ws, "enabled":true}}, "active_agent":"default"}
    })).expect("register fixture Agent profile");
    let sync_dir = tmp.path().join("sync");
    std::fs::create_dir_all(&sync_dir).expect("sync dir");
    std::fs::write(sync_dir.join("enabled"), "true").expect("sync enabled flag");
    rua_sync::identity::NetworkSecret::generate()
        .save(&sync_dir)
        .expect("network secret");

    let state = AppState::from_tempdir(tmp.path().to_path_buf(), rua_server::ServerProfile::Test);
    assert!(
        state
            .sync_enabled
            .load(std::sync::atomic::Ordering::Relaxed),
        "sync should be enabled for v4 emit regression test"
    );

    let app = Router::new()
        .merge(endpoints::chats::root_router())
        .merge(endpoints::console::agent_scoped_router())
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            agent_context_middleware,
        ))
        .with_state(state.clone());
    (app, state, tmp)
}

fn json_req(method: Method, uri: &str, body: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(uri);
    let body = match body {
        Some(json) => {
            builder = builder.header("content-type", "application/json");
            Body::from(json.to_string())
        }
        None => Body::empty(),
    };
    builder.body(body).unwrap()
}

async fn read_json(resp: axum::response::Response) -> Value {
    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    serde_json::from_slice(&bytes).expect("response json")
}

async fn materialize_local_chat(state: &AppState, tmp: &TempDir, chat_id: &str, name: &str) {
    let device_id = state
        .sync_device
        .read()
        .expect("sync device lock")
        .as_ref()
        .map(|device| hex::encode(device.device_id_short()))
        .expect("local device id");
    let address = ChatAddress::agent("default", chat_id);
    let base_manifest_hash =
        rua_server::services::conversation_manifest::initial_conversation_manifest_hash(&address)
            .expect("initial manifest hash");
    materialize_targeted_chat(
        &tmp.path().join("workspaces/default"),
        "default",
        chat_id,
        name,
        &ConversationAuthorityPlacement {
            authority_epoch: ConversationAuthorityEpoch::next_for_device(&device_id, None),
            device_id,
            base_manifest_hash,
        },
    )
    .await
    .expect("materialize local chat");
}

async fn accept_first_input(app: &Router, chat_id: &str, message: &str) {
    let response = app
        .clone()
        .oneshot(json_req(
            Method::POST,
            &format!("/agents/default/chats/{chat_id}/console/submit"),
            Some(&json!({"message":message}).to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = read_json(response).await;
    assert_eq!(body["accepted"], true, "{body}");
}

async fn await_stable_first_input_publication(
    state: &AppState,
    chat_id: &str,
) -> Vec<rua_sync::opaque_replica_log::SyncChange> {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let changes = collect_v4_changes(state).await;
            if changes.iter().any(|change| {
                change.domain == "evergreen_chat"
                    && change.record_id == format!("agent/default/{chat_id}/session")
            }) {
                return changes;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("first Turn reaches stable session publication")
}

async fn collect_v4_changes(state: &AppState) -> Vec<rua_sync::opaque_replica_log::SyncChange> {
    let (sync_doc, blobs_store) = {
        let guard = state.sync_node.lock().await;
        let node = guard.as_ref().expect("sync node");
        (node.doc().clone(), node.blobs_store().clone())
    };
    let network_secret = state
        .sync_network_secret
        .read()
        .unwrap()
        .as_ref()
        .expect("network secret")
        .clone();
    let device = state
        .sync_device
        .read()
        .unwrap()
        .as_ref()
        .expect("device")
        .clone();
    let log = rua_sync::opaque_replica_log::OpaqueReplicaLog::new(
        sync_doc.clone(),
        device.to_iroh_author(),
        network_secret.derive_sync_key(),
        blobs_store,
    );

    let entries = sync_doc
        .get_many(iroh_docs::store::Query::all().build())
        .await
        .expect("query Replica Log");
    let entries = futures_util::TryStreamExt::try_collect::<Vec<_>>(entries)
        .await
        .expect("Replica Log entries");
    let mut changes = Vec::new();
    for entry in entries {
        if let Some(frame) = log
            .read_frame(entry.author(), entry.key())
            .await
            .expect("read sync frame")
        {
            changes.extend(frame.into_changes());
        }
    }
    changes
}

async fn append_evergreen_turn(tmp: &TempDir, chat_id: &str, turn_id: &str) {
    let workspace = tmp.path().join("workspaces/default");
    rua_session::WorkspaceSessionStore::new(&workspace)
        .append_conversation_turn(
            chat_id,
            NewConversationTurn {
                turn_id: turn_id.to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-06-19T00:00:00Z".to_string(),
                    assistant_started_at: Some("2026-06-19T00:00:01Z".to_string()),
                    assistant_completed_at: Some("2026-06-19T00:00:02Z".to_string()),
                    status: "completed".to_string(),
                },
                selected_variant_id: Some(format!("{turn_id}-assistant")),
                messages: vec![
                    NewConversationMessage {
                        message_id: format!("{turn_id}-user"),
                        role: "user".to_string(),
                        created_at: "2026-06-19T00:00:00Z".to_string(),
                        status: "completed".to_string(),
                        content: json!([{"type": "text", "text": "hello"}]),
                        provider: None,
                        tool: None,
                        metadata: serde_json::Map::new(),
                    },
                    NewConversationMessage {
                        message_id: format!("{turn_id}-assistant"),
                        role: "assistant".to_string(),
                        created_at: "2026-06-19T00:00:02Z".to_string(),
                        status: "completed".to_string(),
                        content: json!([{"type": "text", "text": "world"}]),
                        provider: None,
                        tool: None,
                        metadata: serde_json::Map::new(),
                    },
                ],
            },
        )
        .await
        .expect("append evergreen turn");
}

async fn append_in_progress_evergreen_turn(
    tmp: &TempDir,
    chat_id: &str,
    turn_id: &str,
    user_text: &str,
) {
    let workspace = tmp.path().join("workspaces/default");
    rua_session::WorkspaceSessionStore::new(&workspace)
        .append_conversation_turn(
            chat_id,
            NewConversationTurn {
                turn_id: turn_id.to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-06-19T00:00:00Z".to_string(),
                    assistant_started_at: Some("2026-06-19T00:00:01Z".to_string()),
                    assistant_completed_at: None,
                    status: "in_progress".to_string(),
                },
                selected_variant_id: None,
                messages: vec![NewConversationMessage {
                    message_id: format!("{turn_id}-user"),
                    role: "user".to_string(),
                    created_at: "2026-06-19T00:00:00Z".to_string(),
                    status: "completed".to_string(),
                    content: json!([{"type": "text", "text": user_text}]),
                    provider: None,
                    tool: None,
                    metadata: serde_json::Map::new(),
                }],
            },
        )
        .await
        .expect("append in-progress evergreen turn");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_chats_sync_disabled() {
    let (app, _tmp) = build_app();
    let req = json_req(Method::GET, "/chats", None);
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_chat_sync_disabled() {
    let (app, _tmp) = build_app();
    let req = json_req(Method::POST, "/chats", Some(r#"{"name":"Test Chat"}"#));
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_chat_emits_chat_index_v4_when_sync_enabled() {
    let (app, state, _tmp) = build_sync_enabled_app();
    state.wait_platform_initialization().await.unwrap();
    state
        .registry
        .get("default")
        .await
        .unwrap()
        .set_react_runtime(std::sync::Arc::new(SyncOkRuntime))
        .await;

    let resp = app
        .clone()
        .oneshot(json_req(
            Method::POST,
            "/chats",
            Some(r#"{"name":"Sync Runtime Chat"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let chat = read_json(resp).await;
    let chat_id = chat["id"].as_str().expect("chat id");
    accept_first_input(&app, chat_id, "Sync Runtime Chat").await;

    let changes = await_stable_first_input_publication(&state, chat_id).await;
    let chat_change = changes.iter().find(|change| {
        change.domain == "chat" && change.record_id == format!("agent/default/chats/{chat_id}")
    });
    assert!(
        chat_change.is_some_and(|change| {
            change.kind == "index_upsert"
                && change.payload["id"] == Value::String(chat_id.to_string())
                && change.payload["name"] == Value::String("Sync Runtime Chat".to_string())
        }),
        "chat POST must append a chat index v4 change with chat metadata; got {changes:#?}"
    );
    let evergreen_session = changes.iter().find(|change| {
        change.domain == "evergreen_chat"
            && change.record_id == format!("agent/default/{chat_id}/session")
            && change.kind == "file_snapshot"
            && change.payload["metadata"]["title"] == Value::String("Sync Runtime Chat".to_string())
    });
    assert!(
        evergreen_session.is_some(),
        "chat POST must append an Evergreen session snapshot for the newly created chat header; got {changes:#?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_chat_records_device_activity_metadata_when_sync_enabled() {
    let (app, state, _tmp) = build_sync_enabled_app();
    state.wait_platform_initialization().await.unwrap();
    state
        .registry
        .get("default")
        .await
        .unwrap()
        .set_react_runtime(std::sync::Arc::new(SyncOkRuntime))
        .await;
    let device_id = state
        .sync_device
        .read()
        .unwrap()
        .as_ref()
        .map(|device| hex::encode(device.device_id_short()))
        .expect("device id");

    let resp = app
        .clone()
        .oneshot(json_req(
            Method::POST,
            "/chats",
            Some(r#"{"name":"Device activity chat"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    let activity = &body["meta"]["device_activity"];
    assert_eq!(activity["created_on_device_id"], device_id);
    assert_eq!(activity["last_active_device_id"], device_id);
    assert_eq!(activity["participant_device_ids"], json!([device_id]));
    assert_eq!(activity["last_run_host_profile"], "test");
    assert!(
        activity.get("resource_refs").is_none(),
        "coarse metadata must not record per-file provenance"
    );

    let chat_id = body["id"].as_str().expect("chat id");
    accept_first_input(&app, chat_id, "Device activity chat").await;
    let changes = await_stable_first_input_publication(&state, chat_id).await;
    let chat_change = changes.iter().find(|change| {
        change.domain == "chat"
            && change.record_id == format!("agent/default/chats/{chat_id}")
            && change.payload["meta"]["device_activity"]["last_active_device_id"] == device_id
    });
    assert!(
        chat_change.is_some(),
        "chat index sync payload must include coarse device activity; got {changes:#?}"
    );
    let evergreen_session_change = changes.iter().find(|change| {
        change.domain == "evergreen_chat"
            && change.record_id == format!("agent/default/{chat_id}/session")
            && change.payload["device_activity"]["last_active_device_id"] == device_id
    });
    assert!(
        evergreen_session_change.is_some(),
        "Evergreen session snapshot must include coarse device activity; got {changes:#?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn console_submit_updates_chat_device_activity_metadata_when_sync_enabled() {
    let (_app, state, tmp) = build_sync_enabled_app();
    let device_id = state
        .sync_device
        .read()
        .unwrap()
        .as_ref()
        .map(|device| hex::encode(device.device_id_short()))
        .expect("device id");
    let runtime = state
        .registry
        .get_or_create("default")
        .await
        .expect("default runtime");
    runtime
        .set_react_runtime(std::sync::Arc::new(SyncOkRuntime))
        .await;
    let chat_id = "00000000-0000-0000-0000-000000000942";
    materialize_local_chat(&state, &tmp, chat_id, "Device activity").await;
    let store = rua_session::WorkspaceSessionStore::new(runtime.workspace_dir());
    let mut header = store.load_chat_header(chat_id).await.unwrap().unwrap();
    let configuration = state
        .product_hooks
        .resolve_complete_chat_configuration(
            &ChatAddress::agent("default", chat_id),
            rua_server::services::product_hooks::ChatConfigurationMaterialization {
                selected_agent_id: "default".into(),
                running_config: Default::default(),
                security_config: Default::default(),
                agent_assignment: None,
                prompt_file_content: None,
                tool_config_snapshot: Default::default(),
                capability_configuration: None,
                model: None,
                thinking: None,
                collaboration_mode: serde_json::json!("default"),
                harness_configuration: None,
                authority_device_id: device_id.clone(),
            },
        )
        .await
        .unwrap();
    header.extra.insert(
        "hook_configuration".into(),
        serde_json::to_value(configuration).unwrap(),
    );
    store.save_chat_header(&header).await.unwrap();

    let events: Vec<_> = state
        .chat_service
        .chat_stream(
            "default",
            runtime,
            rua_server::endpoints::dto::ChatRequest {
                message: "record coarse device activity".to_string(),
                chat_id: Some(chat_id.to_string()),
                input: None,
                messages: None,
                user_id: None,
                channel: None,
                stream: Some(true),
                model: None,
                attachments: Vec::new(),
                extra: serde_json::Map::new(),
            },
            running_cancel_rx(),
        )
        .collect()
        .await;
    assert!(
        events.iter().any(|event| {
            matches!(
                event,
                Ok(rua_server::endpoints::dto::ChatEvent::Done { .. })
            )
        }),
        "events: {events:?}"
    );

    let indexed = chat_index::get(&tmp.path().join("workspaces/default"), chat_id)
        .await
        .expect("read chat index")
        .expect("chat row");
    let activity = &indexed.meta["device_activity"];
    assert_eq!(activity["last_active_device_id"], device_id);
    assert_eq!(activity["participant_device_ids"], json!([device_id]));
    assert_eq!(activity["last_run_host_profile"], "test");
    assert!(activity.get("resource_refs").is_none());

    let changes = collect_v4_changes(&state).await;
    let chat_change = changes.iter().find(|change| {
        change.domain == "chat"
            && change.record_id == format!("agent/default/chats/{chat_id}")
            && change.payload["meta"]["device_activity"]["last_active_device_id"] == device_id
    });
    assert!(
        chat_change.is_some(),
        "submit must emit updated chat index metadata; got {changes:#?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn update_chat_emits_chat_index_v4_when_sync_enabled() {
    let (app, state, tmp) = build_sync_enabled_app();

    let resp = app
        .clone()
        .oneshot(json_req(
            Method::POST,
            "/chats",
            Some(r#"{"name":"Sync Chat Before Rename"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let chat = read_json(resp).await;
    let chat_id = chat["id"].as_str().expect("chat id");
    materialize_local_chat(&state, &tmp, chat_id, "Existing synchronization Chat").await;
    append_evergreen_turn(&tmp, chat_id, "rename-turn").await;

    let resp = app
        .oneshot(json_req(
            Method::PUT,
            &format!("/chats/{chat_id}"),
            Some(r#"{"name":"Sync Chat After Rename","pinned":true}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let changes = collect_v4_changes(&state).await;
    let chat_change = changes.iter().find(|change| {
        change.domain == "chat"
            && change.record_id == format!("agent/default/chats/{chat_id}")
            && change.kind == "index_upsert"
            && change.payload["name"] == Value::String("Sync Chat After Rename".to_string())
    });
    assert!(
        chat_change.is_some_and(|change| {
            change.payload["id"] == Value::String(chat_id.to_string())
                && change.payload["pinned"] == Value::Bool(true)
        }),
        "chat PUT must append a chat index v4 upsert with updated metadata; got {changes:#?}"
    );
    let evergreen_session = changes.iter().find(|change| {
        change.domain == "evergreen_chat"
            && change.record_id == format!("agent/default/{chat_id}/session")
            && change.kind == "file_snapshot"
            && change.payload["metadata"]["title"]
                == Value::String("Sync Chat After Rename".to_string())
            && change.payload["metadata"]["pinned"] == Value::Bool(true)
    });
    assert!(
        evergreen_session.is_some(),
        "chat PUT must append an Evergreen session snapshot when it updates chat header metadata; got {changes:#?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delete_chat_emits_chat_index_v4_when_sync_enabled() {
    let (app, state, tmp) = build_sync_enabled_app();

    let resp = app
        .clone()
        .oneshot(json_req(
            Method::POST,
            "/chats",
            Some(r#"{"name":"Sync Chat To Delete"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let chat = read_json(resp).await;
    let chat_id = chat["id"].as_str().expect("chat id");
    materialize_local_chat(&state, &tmp, chat_id, "Existing synchronization Chat").await;
    append_evergreen_turn(&tmp, chat_id, "delete-turn").await;

    let resp = app
        .oneshot(json_req(Method::DELETE, &format!("/chats/{chat_id}"), None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let changes = collect_v4_changes(&state).await;
    let chat_change = changes.iter().find(|change| {
        change.domain == "chat"
            && change.record_id == format!("agent/default/chats/{chat_id}")
            && change.kind == "index_delete"
    });
    assert!(
        chat_change.is_some_and(|change| change.payload == Value::Object(Default::default())),
        "chat DELETE must append a chat index v4 delete tombstone; got {changes:#?}"
    );
    let evergreen_delete = changes.iter().find(|change| {
        change.domain == "evergreen_chat"
            && change.record_id == format!("agent/default/{chat_id}/session")
            && change.kind == "file_delete"
    });
    assert!(
        evergreen_delete.is_some(),
        "chat DELETE must append Evergreen file tombstones before deleting local chat storage; got {changes:#?}"
    );
    assert!(
        changes.iter().all(|change| {
            change.payload.get("scope_root").is_none()
                && change.payload.get("tool_recall_keys").is_none()
        }),
        "replicated Chat deletion changes must not leak Device-local paths or Tool archive identities; got {changes:#?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn batch_delete_chat_emits_chat_index_and_evergreen_v4_when_sync_enabled() {
    let (app, state, tmp) = build_sync_enabled_app();

    let resp = app
        .clone()
        .oneshot(json_req(
            Method::POST,
            "/chats",
            Some(r#"{"name":"Sync Chat Batch Delete"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let chat = read_json(resp).await;
    let chat_id = chat["id"].as_str().expect("chat id");
    materialize_local_chat(&state, &tmp, chat_id, "Existing synchronization Chat").await;
    append_evergreen_turn(&tmp, chat_id, "batch-delete-turn").await;

    let body = format!(r#"["{chat_id}"]"#);
    let resp = app
        .oneshot(json_req(Method::POST, "/chats/batch-delete", Some(&body)))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let changes = collect_v4_changes(&state).await;
    let chat_delete = changes.iter().find(|change| {
        change.domain == "chat"
            && change.record_id == format!("agent/default/chats/{chat_id}")
            && change.kind == "index_delete"
    });
    assert!(
        chat_delete.is_some(),
        "chat batch-delete must append chat index v4 tombstones; got {changes:#?}"
    );
    let evergreen_delete = changes.iter().find(|change| {
        change.domain == "evergreen_chat"
            && change.record_id == format!("agent/default/{chat_id}/session")
            && change.kind == "file_delete"
    });
    assert!(
        evergreen_delete.is_some(),
        "chat batch-delete must append Evergreen file tombstones before deleting local chat storage; got {changes:#?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pin_order_emits_chat_index_v4_when_sync_enabled() {
    let (app, state, tmp) = build_sync_enabled_app();

    let resp = app
        .clone()
        .oneshot(json_req(
            Method::POST,
            "/chats",
            Some(r#"{"name":"Pinned Sync Chat"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let chat = read_json(resp).await;
    let chat_id = chat["id"].as_str().expect("chat id");
    materialize_local_chat(&state, &tmp, chat_id, "Existing synchronization Chat").await;

    let body = format!(r#"[{{"id":"{chat_id}","pin_order":42}}]"#);
    let resp = app
        .oneshot(json_req(Method::PUT, "/chats/pin-order", Some(&body)))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let changes = collect_v4_changes(&state).await;
    let pin_order_change = changes.iter().find(|change| {
        change.domain == "chat"
            && change.record_id == format!("agent/default/chats/{chat_id}")
            && change.kind == "index_upsert"
            && change.payload["pin_order"] == Value::Number(42.into())
    });
    assert!(
        pin_order_change.is_some(),
        "chat pin-order PUT must append updated chat index v4 metadata; got {changes:#?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn branch_chat_emits_chat_index_and_evergreen_v4_when_sync_enabled() {
    let (app, state, tmp) = build_sync_enabled_app();

    let resp = app
        .clone()
        .oneshot(json_req(
            Method::POST,
            "/chats",
            Some(r#"{"name":"Branch Source Chat"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let chat = read_json(resp).await;
    let chat_id = chat["id"].as_str().expect("chat id");
    materialize_local_chat(&state, &tmp, chat_id, "Existing synchronization Chat").await;
    append_evergreen_turn(&tmp, chat_id, "branch-turn").await;
    append_evergreen_turn(&tmp, chat_id, "branch-turn-2").await;

    let resp = app
        .oneshot(json_req(
            Method::POST,
            &format!("/chats/{chat_id}/branch"),
            Some(r#"{"before_turn_id":"branch-turn-2"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    let branch_chat_id = body["chat"]["id"].as_str().expect("branch chat id");

    let changes = collect_v4_changes(&state).await;
    let branch_index = changes.iter().find(|change| {
        change.domain == "chat"
            && change.record_id == format!("agent/default/chats/{branch_chat_id}")
            && change.kind == "index_upsert"
    });
    assert!(
        branch_index.is_some(),
        "chat branch must append a chat index v4 upsert for the new branch; got {changes:#?}"
    );
    let branch_session = changes.iter().find(|change| {
        change.domain == "evergreen_chat"
            && change.record_id == format!("agent/default/{branch_chat_id}/session")
            && change.kind == "file_snapshot"
    });
    assert!(
        branch_session.is_some(),
        "chat branch must append Evergreen v4 snapshots for copied branch content; got {changes:#?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rollback_chat_emits_chat_index_and_evergreen_v4_when_sync_enabled() {
    let (app, state, tmp) = build_sync_enabled_app();

    let resp = app
        .clone()
        .oneshot(json_req(
            Method::POST,
            "/chats",
            Some(r#"{"name":"Rollback Source Chat"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let chat = read_json(resp).await;
    let chat_id = chat["id"].as_str().expect("chat id");
    materialize_local_chat(&state, &tmp, chat_id, "Existing synchronization Chat").await;
    append_evergreen_turn(&tmp, chat_id, "rollback-turn-1").await;
    append_evergreen_turn(&tmp, chat_id, "rollback-turn-2").await;

    let resp = app
        .oneshot(json_req(
            Method::POST,
            &format!("/chats/{chat_id}/rollback"),
            Some(r#"{"before_turn_id":"rollback-turn-2"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let changes = collect_v4_changes(&state).await;
    let rollback_index = changes.iter().find(|change| {
        change.domain == "chat"
            && change.record_id == format!("agent/default/chats/{chat_id}")
            && change.kind == "index_upsert"
    });
    assert!(
        rollback_index.is_some(),
        "chat rollback must append an updated chat index v4 upsert; got {changes:#?}"
    );
    let turn_index_snapshot = changes.iter().find(|change| {
        change.domain == "evergreen_chat"
            && change.record_id == format!("agent/default/{chat_id}/turn_index")
            && change.kind == "file_snapshot"
    });
    assert!(
        turn_index_snapshot.is_some(),
        "chat rollback must append Evergreen v4 snapshots for the rewritten active turn index; got {changes:#?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn console_submit_eager_chat_index_emits_v4_before_turn_is_stable() {
    let (app, state, _tmp) = build_sync_enabled_app();
    let runtime = state
        .registry
        .get_or_create("default")
        .await
        .expect("default runtime");
    runtime
        .set_react_runtime(std::sync::Arc::new(SyncStartupFailingRuntime))
        .await;
    let chat_id = "00000000-0000-0000-0000-000000000943";

    let resp = app
        .oneshot(json_req(
            Method::POST,
            &format!("/agents/default/chats/{chat_id}/console/submit"),
            Some(&format!(
                r#"{{"message":"eager chat index sync","chat_id":"{chat_id}","submission_id":"submission-eager-sync"}}"#
            )),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_slice(
        &resp
            .into_body()
            .collect()
            .await
            .expect("collect submit body")
            .to_bytes(),
    )
    .expect("submit response json");
    assert_eq!(body["accepted"], true, "submission: {body}");
    assert_eq!(body["mode"], "started");
    assert_eq!(body["submission_id"], "submission-eager-sync");

    let changes = collect_v4_changes(&state).await;
    let chat_index_change = changes.iter().find(|change| {
        change.domain == "chat"
            && change.record_id == format!("agent/default/chats/{chat_id}")
            && change.kind == "index_upsert"
            && change.payload["name"] == Value::String("eager chat index sync".to_string())
    });
    assert!(
        chat_index_change.is_some(),
        "eager console submit must publish chat index even when the turn never reaches a stable terminal state; got {changes:#?}"
    );
    let stable_session_change = changes.iter().find(|change| {
        change.domain == "evergreen_chat"
            && change.record_id == format!("agent/default/{chat_id}/turn_index")
    });
    assert!(
        stable_session_change.is_none(),
        "eager chat index publication must not publish an Evergreen turn snapshot before the turn is stable; got {changes:#?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_failure_terminal_fact_emits_evergreen_v4_when_sync_enabled() {
    let (_app, state, tmp) = build_sync_enabled_app();
    state.wait_platform_initialization().await.unwrap();
    let runtime = state
        .registry
        .get_or_create("default")
        .await
        .expect("default runtime");
    runtime
        .set_react_runtime(std::sync::Arc::new(SyncFailingRuntime))
        .await;
    let chat_id = "00000000-0000-0000-0000-000000000904";
    materialize_local_chat(&state, &tmp, chat_id, "Runtime failure").await;
    let authority_device_id = hex::encode(
        state
            .sync_device
            .read()
            .unwrap()
            .as_ref()
            .unwrap()
            .device_id_short(),
    );
    let store = rua_session::WorkspaceSessionStore::new(runtime.workspace_dir());
    let mut header = store.load_chat_header(chat_id).await.unwrap().unwrap();
    let configuration = state
        .product_hooks
        .resolve_complete_chat_configuration(
            &ChatAddress::agent("default", chat_id),
            rua_server::services::product_hooks::ChatConfigurationMaterialization {
                selected_agent_id: "default".into(),
                running_config: Default::default(),
                security_config: Default::default(),
                agent_assignment: None,
                prompt_file_content: None,
                tool_config_snapshot: Default::default(),
                capability_configuration: None,
                model: None,
                thinking: None,
                collaboration_mode: serde_json::json!("default"),
                harness_configuration: None,
                authority_device_id,
            },
        )
        .await
        .unwrap();
    header.extra.insert(
        "hook_configuration".into(),
        serde_json::to_value(configuration).unwrap(),
    );
    store.save_chat_header(&header).await.unwrap();

    let events: Vec<_> = state
        .chat_service
        .chat_stream(
            "default",
            runtime,
            rua_server::endpoints::dto::ChatRequest {
                message: "trigger sync failure".to_string(),
                chat_id: Some(chat_id.to_string()),
                input: None,
                messages: None,
                user_id: None,
                channel: None,
                stream: Some(true),
                model: None,
                attachments: Vec::new(),
                extra: serde_json::Map::new(),
            },
            running_cancel_rx(),
        )
        .collect()
        .await;
    assert!(
        events.iter().any(|event| {
            matches!(
                event,
                Ok(rua_server::endpoints::dto::ChatEvent::Error { detail })
                    if detail.contains("provider disconnected")
            )
        }),
        "events: {events:?}"
    );

    let changes = collect_v4_changes(&state).await;
    let turn_index_change = changes.iter().find(|change| {
        change.domain == "evergreen_chat"
            && change.record_id == format!("agent/default/{chat_id}/turn_index")
            && change.payload.to_string().contains("failed")
    });
    assert!(
        turn_index_change.is_some(),
        "runtime failure must emit a stable Evergreen turn_index snapshot after writing failed terminal fact; got {changes:#?}"
    );
    let session_change = changes.iter().find(|change| {
        change.domain == "evergreen_chat"
            && change.record_id == format!("agent/default/{chat_id}/session")
            && change
                .payload
                .to_string()
                .contains("conversation_runtime_terminal")
            && change.payload.to_string().contains("failed")
    });
    assert!(
        session_change.is_some(),
        "runtime failure must emit a stable Evergreen session snapshot with the failed runtime note; got {changes:#?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_open_orphan_in_progress_read_is_read_only_when_sync_enabled() {
    let (app, state, tmp) = build_sync_enabled_app();
    let turn_id = "turn-orphan-sync-1";

    let create = app
        .clone()
        .oneshot(json_req(
            Method::POST,
            "/chats",
            Some(r#"{"name":"Orphan sync chat"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(create.status(), StatusCode::OK);
    let created = read_json(create).await;
    let chat_id = created["id"].as_str().expect("chat id");
    materialize_local_chat(&state, &tmp, chat_id, "Existing synchronization Chat").await;

    append_in_progress_evergreen_turn(&tmp, chat_id, turn_id, "restart before approval").await;
    let _ = collect_v4_changes(&state).await;

    let open = app
        .oneshot(json_req(Method::GET, &format!("/chats/{chat_id}"), None))
        .await
        .unwrap();
    assert_eq!(open.status(), StatusCode::OK);

    let workspace = tmp.path().join("workspaces/default");
    let turns = rua_session::WorkspaceSessionStore::new(&workspace)
        .read_turn_range(chat_id, 1, usize::MAX)
        .await
        .expect("read turns after chat open");
    assert_eq!(turns.len(), 1);
    assert_eq!(
        turns[0].lifecycle.status, "in_progress",
        "opening Chat History must not reconcile orphan in_progress turns"
    );

    let changes = collect_v4_changes(&state).await;
    let turn_index_change = changes.iter().find(|change| {
        change.domain == "evergreen_chat"
            && change.record_id == format!("agent/default/{chat_id}/turn_index")
            && change.payload.to_string().contains(turn_id)
            && change.payload.to_string().contains("interrupted")
    });
    assert!(
        turn_index_change.is_none(),
        "Chat History read must not emit an interrupted turn_index snapshot; got {changes:#?}"
    );
    let session_change = changes.iter().find(|change| {
        change.domain == "evergreen_chat"
            && change.record_id == format!("agent/default/{chat_id}/session")
            && change
                .payload
                .to_string()
                .contains("conversation_runtime_terminal")
            && change.payload.to_string().contains("runtime_restart")
    });
    assert!(
        session_change.is_none(),
        "Chat History read must not emit a runtime_restart terminal note snapshot; got {changes:#?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn incomplete_tool_group_does_not_emit_evergreen_v4_when_sync_enabled() {
    let (_app, state, _tmp) = build_sync_enabled_app();
    let runtime = state
        .registry
        .get_or_create("default")
        .await
        .expect("default runtime");
    let chat_id = "00000000-0000-0000-0000-000000000905";

    rua_session::WorkspaceSessionStore::new(runtime.workspace_dir())
        .save_chat_header(&rua_session::ChatSessionHeader::new(
            rua_session::ChatMetadata {
                chat_id: chat_id.into(),
                agent_id: "default".into(),
                title: "In progress sync gate".into(),
                created_at: "2026-06-25T00:00:00Z".into(),
                last_activity_at: "2026-06-25T00:00:00Z".into(),
                pinned: false,
                archived: false,
                preview: None,
            },
        ))
        .await
        .unwrap();

    let persistence = rua_server::services::console_chat::WorkspaceSessionPersistence::new(
        runtime.workspace_dir(),
    )
    .with_session_sync_emitter(rua_server::services::console_chat::SessionSyncEmitter::new(
        _tmp.path().to_path_buf(),
        state.sync_enabled.clone(),
        state.sync_node.clone(),
        state.sync_network_secret.clone(),
        state.sync_device.clone(),
        state.sync_hlc_clock.clone(),
        state.sync_trigger.clone(),
    ));
    rua_channel::core::console::SessionPersistence::save_messages(
        &persistence,
        "default",
        chat_id,
        &[
            rua_contract::agent::AgentInputMessage {
                role: "user".to_string(),
                content: vec![rua_core::message::ContentPart::Text {
                    text: "run a gated command".to_string(),
                }],
            },
            rua_contract::agent::AgentInputMessage {
                role: "assistant".to_string(),
                content: vec![rua_core::message::ContentPart::ToolUse {
                    call: rua_core::ToolCall::new(
                        "call-open",
                        "bash",
                        json!({"command": "du -sh ."}),
                    ),
                }],
            },
        ],
    )
    .await
    .expect("save in-progress messages");

    let store = rua_session::WorkspaceSessionStore::new(runtime.workspace_dir());
    let turns = store.read_turn_range(chat_id, 1, 10).await.unwrap();
    assert_eq!(turns[0].lifecycle.status, "in_progress");

    let changes = collect_v4_changes(&state).await;
    assert!(
        changes.iter().all(|change| {
            !(change.domain == "evergreen_chat"
                && change.record_id.starts_with(&format!("default/{chat_id}/")))
        }),
        "an incomplete tool group must not emit an Evergreen snapshot; got {changes:#?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn complete_model_call_publishes_active_turn_and_advances_watermark() {
    let (_app, state, tmp) = build_sync_enabled_app();
    let runtime = state
        .registry
        .get_or_create("default")
        .await
        .expect("default runtime");
    let chat_id = "00000000-0000-0000-0000-000000000907";
    let turn_id = "turn-active-model-call";

    rua_session::WorkspaceSessionStore::new(runtime.workspace_dir())
        .save_chat_header(&rua_session::ChatSessionHeader::new(
            rua_session::ChatMetadata {
                chat_id: chat_id.into(),
                agent_id: "default".into(),
                title: "Active model call publication".into(),
                created_at: "2026-06-25T00:00:00Z".into(),
                last_activity_at: "2026-06-25T00:00:00Z".into(),
                pinned: false,
                archived: false,
                preview: None,
            },
        ))
        .await
        .unwrap();
    let header_store = rua_session::WorkspaceSessionStore::new(runtime.workspace_dir());
    let mut header = header_store
        .load_chat_header(chat_id)
        .await
        .expect("load active Chat header")
        .expect("active Chat header exists");
    header.context_snapshot.pending_action = Some(serde_json::json!({
        "action_type": "ask_user",
        "request_id": "ask-active-model-call",
        "status": "waiting"
    }));
    header_store
        .save_chat_header(&header)
        .await
        .expect("save active pending action");
    append_in_progress_evergreen_turn(&tmp, chat_id, turn_id, "say hi").await;

    let persistence = rua_server::services::console_chat::WorkspaceSessionPersistence::new(
        runtime.workspace_dir(),
    )
    .with_active_turn_id(turn_id.to_string())
    .with_session_sync_emitter(rua_server::services::console_chat::SessionSyncEmitter::new(
        tmp.path().to_path_buf(),
        state.sync_enabled.clone(),
        state.sync_node.clone(),
        state.sync_network_secret.clone(),
        state.sync_device.clone(),
        state.sync_hlc_clock.clone(),
        state.sync_trigger.clone(),
    ));
    rua_channel::core::console::SessionPersistence::save_messages(
        &persistence,
        "default",
        chat_id,
        &[rua_contract::agent::AgentInputMessage {
            role: "assistant".to_string(),
            content: vec![rua_core::message::ContentPart::Text {
                text: "hi".to_string(),
            }],
        }],
    )
    .await
    .expect("save complete active model call");

    let store = rua_session::WorkspaceSessionStore::new(runtime.workspace_dir());
    let turns = store.read_turn_range(chat_id, 1, 10).await.unwrap();
    assert_eq!(turns[0].lifecycle.status, "in_progress");

    let changes = collect_v4_changes(&state).await;
    assert!(
        changes.iter().any(|change| {
            change.domain == "evergreen_chat"
                && change.record_id == format!("agent/default/{chat_id}/turn_index")
                && change.payload.to_string().contains("in_progress")
        }),
        "a complete Model Call Boundary must publish while the Turn remains active; got {changes:#?}"
    );

    let address = ChatAddress::agent("default", chat_id);
    let manifest = rua_server::services::conversation_manifest::load_conversation_manifest(
        &runtime.workspace_dir(),
        &address,
    )
    .await
    .expect("load manifest")
    .expect("manifest exists");
    let watermark =
        rua_server::services::durable_publication_watermark::DurablePublicationWatermarkStore::new(
            tmp.path().join("sync"),
        )
        .expect("open watermark store")
        .load(&address)
        .await
        .expect("load watermark")
        .expect("published watermark exists");
    assert_eq!(watermark.durable_sequence, manifest.latest_sequence);
    assert_eq!(watermark.manifest_hash, manifest.hash().unwrap());
    assert_eq!(watermark.manifest, manifest);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn completed_turn_save_emits_stable_evergreen_v4_when_sync_enabled() {
    let (_app, state, _tmp) = build_sync_enabled_app();
    let runtime = state
        .registry
        .get_or_create("default")
        .await
        .expect("default runtime");
    let chat_id = "00000000-0000-0000-0000-000000000906";

    rua_session::WorkspaceSessionStore::new(runtime.workspace_dir())
        .save_chat_header(&rua_session::ChatSessionHeader::new(
            rua_session::ChatMetadata {
                chat_id: chat_id.into(),
                agent_id: "default".into(),
                title: "Completed sync gate".into(),
                created_at: "2026-06-25T00:00:00Z".into(),
                last_activity_at: "2026-06-25T00:00:00Z".into(),
                pinned: false,
                archived: false,
                preview: None,
            },
        ))
        .await
        .unwrap();

    let persistence = rua_server::services::console_chat::WorkspaceSessionPersistence::new(
        runtime.workspace_dir(),
    )
    .with_session_sync_emitter(rua_server::services::console_chat::SessionSyncEmitter::new(
        _tmp.path().to_path_buf(),
        state.sync_enabled.clone(),
        state.sync_node.clone(),
        state.sync_network_secret.clone(),
        state.sync_device.clone(),
        state.sync_hlc_clock.clone(),
        state.sync_trigger.clone(),
    ));
    rua_channel::core::console::SessionPersistence::save_messages(
        &persistence,
        "default",
        chat_id,
        &[
            rua_contract::agent::AgentInputMessage {
                role: "user".to_string(),
                content: vec![rua_core::message::ContentPart::Text {
                    text: "say hi".to_string(),
                }],
            },
            rua_contract::agent::AgentInputMessage {
                role: "assistant".to_string(),
                content: vec![rua_core::message::ContentPart::Text {
                    text: "hi".to_string(),
                }],
            },
        ],
    )
    .await
    .expect("save completed messages");

    let store = rua_session::WorkspaceSessionStore::new(runtime.workspace_dir());
    let turns = store.read_turn_range(chat_id, 1, 10).await.unwrap();
    assert_eq!(turns[0].lifecycle.status, "completed");

    let changes = collect_v4_changes(&state).await;
    assert!(
        changes.iter().any(|change| {
            change.domain == "evergreen_chat"
                && change.record_id == format!("agent/default/{chat_id}/turn_index")
                && change.payload.to_string().contains("completed")
        }),
        "completed turns must emit a stable turn_index snapshot; got {changes:#?}"
    );
    assert!(
        changes.iter().any(|change| {
            change.domain == "evergreen_chat"
                && change.record_id
                    == format!("agent/default/{chat_id}/messages/000000000000.jsonl")
                && change.payload.to_string().contains("hi")
        }),
        "completed turns must emit stable message chunk snapshot; got {changes:#?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_and_list_chat_sync_disabled() {
    let (app, _tmp) = build_app();

    let req = json_req(Method::POST, "/chats", Some(r#"{"name":"My Chat"}"#));
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let draft = read_json(resp).await;
    accept_first_input(&app, draft["id"].as_str().unwrap(), "My Chat").await;
    let req = json_req(Method::GET, "/chats", None);
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 1024 * 64)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let chats = json.as_array().unwrap();
    assert!(!chats.is_empty());
    assert_eq!(chats[0]["name"], "My Chat");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delete_chat_sync_disabled() {
    let (app, tmp) = build_app();

    let req = json_req(Method::POST, "/chats", Some(r#"{"name":"To Delete"}"#));
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 1024 * 64)
        .await
        .unwrap();
    let created: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let chat_id = created["id"].as_str().unwrap();
    accept_first_input(&app, chat_id, "To Delete").await;

    let req = json_req(Method::DELETE, &format!("/chats/{chat_id}"), None);
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let req = json_req(Method::GET, "/chats", None);
    let resp = app.oneshot(req).await.unwrap();
    let body = axum::body::to_bytes(resp.into_body(), 1024 * 64)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let chats = json.as_array().unwrap();
    assert!(chats.is_empty());

    let address = rua_core::chat::ChatAddress::agent("default", chat_id);
    let intent =
        rua_server::services::chat_deletion::ChatDeleteIntentStore::from_rua_root(tmp.path())
            .load(&address)
            .expect("read durable Chat Delete Intent")
            .expect("Sync-disabled deletion must still persist a durable Chat Delete Intent");
    assert_eq!(intent.address, address);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delete_chat_fails_closed_when_intent_cannot_be_persisted() {
    let (app, tmp) = build_app();

    let req = json_req(Method::POST, "/chats", Some(r#"{"name":"Keep Me"}"#));
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 1024 * 64)
        .await
        .unwrap();
    let created: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let chat_id = created["id"].as_str().unwrap();
    accept_first_input(&app, chat_id, "Existing synchronization Chat").await;

    std::fs::remove_dir_all(tmp.path().join("sync")).unwrap();
    std::fs::write(tmp.path().join("sync"), "not a directory").unwrap();
    let req = json_req(Method::DELETE, &format!("/chats/{chat_id}"), None);
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);

    let req = json_req(Method::GET, "/chats", None);
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 1024 * 64)
        .await
        .unwrap();
    let chats: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(chats
        .as_array()
        .unwrap()
        .iter()
        .any(|chat| chat["id"] == chat_id));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delete_chat_rejects_pinned_chat_without_creating_intent() {
    let (app, tmp) = build_app();

    let req = json_req(Method::POST, "/chats", Some(r#"{"name":"Pinned"}"#));
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 1024 * 64)
        .await
        .unwrap();
    let created: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let chat_id = created["id"].as_str().unwrap();
    accept_first_input(&app, chat_id, "Existing synchronization Chat").await;

    let req = json_req(
        Method::PUT,
        &format!("/chats/{chat_id}"),
        Some(r#"{"pinned":true}"#),
    );
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let req = json_req(
        Method::DELETE,
        &format!("/chats/{chat_id}?force=true"),
        None,
    );
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);

    let address = rua_core::chat::ChatAddress::agent("default", chat_id);
    assert!(
        rua_server::services::chat_deletion::ChatDeleteIntentStore::from_rua_root(tmp.path())
            .load(&address)
            .unwrap()
            .is_none()
    );

    let req = json_req(Method::GET, "/chats", None);
    let resp = app.oneshot(req).await.unwrap();
    let body = axum::body::to_bytes(resp.into_body(), 1024 * 64)
        .await
        .unwrap();
    let chats: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(chats
        .as_array()
        .unwrap()
        .iter()
        .any(|chat| chat["id"] == chat_id));
}
