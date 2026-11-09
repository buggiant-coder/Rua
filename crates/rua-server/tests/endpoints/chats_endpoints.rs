#![allow(deprecated)]

use axum::body::Body;
use axum::extract::Request;
use axum::http::{header, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use tempfile::TempDir;
use tower::ServiceExt;

use rua_server::endpoints;
use rua_server::middleware::agent_context_middleware;
use rua_server::AppState;
use rua_sync::chat_adapter::{
    index_record_id, ChatIndexSyncAdapter, CHAT_DOMAIN, CHAT_KIND_INDEX_DELETE,
};
use rua_sync::hlc::HlcTimestamp;
use rua_sync::merge_store::MergeStore;
use rua_sync::opaque_replica_log::SyncChange;
use rua_sync::sync_registry::DomainSyncAdapter;

const AGENT_ID: &str = "default";

fn build_state() -> (AppState, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    let state = AppState::from_tempdir(tmp.path().to_path_buf(), rua_server::ServerProfile::Test);
    (state, tmp)
}

fn build_app(state: AppState) -> Router {
    Router::new()
        .merge(endpoints::chats::root_router())
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            agent_context_middleware,
        ))
        .with_state(state)
}

async fn read_json(resp: axum::response::Response) -> serde_json::Value {
    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    serde_json::from_slice(&bytes).expect("body is json")
}

fn json_post(uri: &str, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .header("X-Agent-Id", AGENT_ID)
        .body(Body::from(body.to_string()))
        .expect("request")
}

fn is_uuid_v4_surface(s: &str) -> bool {
    if s.len() != 36 {
        return false;
    }
    let bytes = s.as_bytes();
    for (idx, ch) in bytes.iter().enumerate() {
        let is_dash_pos = matches!(idx, 8 | 13 | 18 | 23);
        if is_dash_pos {
            if *ch != b'-' {
                return false;
            }
        } else if !ch.is_ascii_hexdigit() {
            return false;
        }
    }
    true
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn post_chats_mints_one_chat_id_uuid_without_session_id() {
    let (state, _guard) = build_state();
    let app = build_app(state);

    let req = json_post(
        "/chats",
        serde_json::json!({
            "name": "Chat ID only",
        }),
    );
    let resp = app.oneshot(req).await.expect("oneshot");
    assert_eq!(resp.status(), StatusCode::OK, "POST /chats must succeed");

    let body = read_json(resp).await;
    let id = body["id"].as_str().expect("response.id must be a string");
    assert!(body.get("session_id").is_none());
    assert!(
        is_uuid_v4_surface(id),
        "response.id must be a 36-char UUID surface (8-4-4-4-12), got {id:?}"
    );
}

fn json_get(uri: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .header("X-Agent-Id", AGENT_ID)
        .body(Body::empty())
        .expect("request")
}

fn json_delete(uri: &str) -> Request<Body> {
    Request::builder()
        .method("DELETE")
        .uri(uri)
        .header("X-Agent-Id", AGENT_ID)
        .body(Body::empty())
        .expect("request")
}

fn build_app_with_agent_scoped(state: AppState) -> Router {
    Router::new()
        .merge(endpoints::chats::root_router())
        .merge(endpoints::chats::agent_scoped_router())
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            agent_context_middleware,
        ))
        .with_state(state)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_chat_existing_returns_200_with_content() {
    let (state, _guard) = build_state();
    let app = build_app(state);

    let create_req = json_post("/chats", serde_json::json!({"name": "existing chat"}));
    let create_resp = app.clone().oneshot(create_req).await.expect("create");
    assert_eq!(create_resp.status(), StatusCode::OK);
    let created = read_json(create_resp).await;
    let chat_id = created["id"].as_str().expect("created chat id");

    let get_req = json_get(&format!("/chats/{chat_id}"));
    let get_resp = app.oneshot(get_req).await.expect("get");
    assert_eq!(
        get_resp.status(),
        StatusCode::OK,
        "existing chat must return 200"
    );

    let body = read_json(get_resp).await;
    assert!(body.get("status").is_none());
    assert!(body.get("active_turn").is_none());
    assert!(body.get("control").is_none());
    assert!(body["messages"].is_array(), "messages must be an array");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_chat_unknown_uuid_returns_200_empty() {
    let (state, _guard) = build_state();
    let app = build_app(state);

    let unknown_id = uuid::Uuid::new_v4().to_string();
    let req = json_get(&format!("/chats/{unknown_id}"));
    let resp = app.oneshot(req).await.expect("get unknown");
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "unknown UUID must return 200, not 404"
    );

    let body = read_json(resp).await;
    assert!(body.get("status").is_none());
    assert!(body.get("active_turn").is_none());
    assert!(body.get("control").is_none());
    assert!(
        body["model"].is_null(),
        "model must be null for unknown chat"
    );
    let messages = body["messages"].as_array().expect("messages array");
    assert!(
        messages.is_empty(),
        "messages must be empty for unknown chat"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_chat_deleted_returns_200_empty() {
    let (state, _guard) = build_state();
    let app = build_app(state.clone());

    let create_req = json_post("/chats", serde_json::json!({"name": "to-delete"}));
    let create_resp = app.clone().oneshot(create_req).await.expect("create");
    let created = read_json(create_resp).await;
    let chat_id = created["id"].as_str().expect("chat id");
    let outcome = state
        .conversation_input_intake
        .submit_delivery(
            "default",
            rua_channel::core::console::ConsoleChatRequest {
                message: "to-delete".into(),
                chat_id: Some(chat_id.into()),
                model: None,
                is_reconnect: false,
                attachments: vec![],
                input: None,
                messages: None,
                extra: Default::default(),
                channel_kind: None,
            },
        )
        .await;
    assert!(matches!(outcome,rua_server::services::conversation_input_intake::ConversationInputDeliveryOutcome::Delivered { .. }),"{outcome:?}");

    let del_req = json_delete(&format!("/chats/{chat_id}"));
    let del_resp = app.clone().oneshot(del_req).await.expect("delete");
    assert_eq!(del_resp.status(), StatusCode::OK, "DELETE must succeed");

    let get_req = json_get(&format!("/chats/{chat_id}"));
    let get_resp = app.oneshot(get_req).await.expect("get deleted");
    assert_eq!(
        get_resp.status(),
        StatusCode::OK,
        "deleted chat must return 200, not 404"
    );

    let body = read_json(get_resp).await;
    assert!(body.get("status").is_none());
    assert!(body.get("active_turn").is_none());
    assert!(body.get("control").is_none());
    assert!(
        body["model"].is_null(),
        "model must be null for deleted chat"
    );
    let messages = body["messages"].as_array().expect("messages array");
    assert!(
        messages.is_empty(),
        "messages must be empty for deleted chat"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_chat_agent_scoped_unknown_chat_returns_200_empty() {
    let (state, _guard) = build_state();
    let app = build_app_with_agent_scoped(state);

    let unknown_id = uuid::Uuid::new_v4().to_string();
    let req = Request::builder()
        .method("GET")
        .uri(format!("/agents/{AGENT_ID}/chats/{unknown_id}"))
        .body(Body::empty())
        .expect("request");
    let resp = app.oneshot(req).await.expect("get");
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "agent-scoped GET with unknown chat must return 200"
    );

    let body = read_json(resp).await;
    assert!(body.get("status").is_none());
    assert!(body.get("active_turn").is_none());
    assert!(body.get("control").is_none());
    assert!(body["model"].is_null(), "model must be null");
    let messages = body["messages"].as_array().expect("messages array");
    assert!(messages.is_empty(), "messages must be empty");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_chats_uses_v4_maintained_sqlite_index() {
    let (state, guard) = build_state();
    let app = build_app(state);

    let initial = app
        .clone()
        .oneshot(json_get("/chats"))
        .await
        .expect("initial list");
    assert_eq!(initial.status(), StatusCode::OK);
    assert_eq!(read_json(initial).await.as_array().unwrap().len(), 0);

    let adapter = ChatIndexSyncAdapter::new(guard.path().to_path_buf(), "device-b");
    let merge_dir = TempDir::new().expect("merge dir");
    let merge_store = MergeStore::open(merge_dir.path()).expect("merge store");
    let chat_payload = serde_json::json!({
        "id": "11111111-1111-4111-8111-111111111111",
        "name": "synced chat from v4",
        "user_id": "sync-e2e",
        "channel": "console",
        "created_at": "2026-01-01T00:00:00Z",
        "updated_at": "2026-01-01T00:00:01Z",
        "meta": {"source": "v4"},
        "status": "idle",
        "model": null,
        "pinned": false,
        "pin_order": 0
    });
    let upsert = ChatIndexSyncAdapter::index_change(
        "op-chat-index-upsert",
        rua_core::ChatAddress::agent("default", "11111111-1111-4111-8111-111111111111"),
        chat_payload,
        "device-a",
        HlcTimestamp {
            wall_ms: 100,
            counter: 0,
            device_id: [0; 8],
        },
    );
    adapter
        .apply_change(&upsert, &merge_store)
        .await
        .expect("apply v4 chat index upsert");

    let synced = app
        .clone()
        .oneshot(json_get("/chats"))
        .await
        .expect("synced list");
    assert_eq!(synced.status(), StatusCode::OK);
    let body = read_json(synced).await;
    let rows = body.as_array().expect("list response");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], "synced chat from v4");

    let delete = SyncChange::new(
        "op-chat-index-delete",
        CHAT_DOMAIN,
        index_record_id(&rua_core::ChatAddress::agent(
            "default",
            "11111111-1111-4111-8111-111111111111",
        )),
        CHAT_KIND_INDEX_DELETE,
        "device-a",
        HlcTimestamp {
            wall_ms: 200,
            counter: 0,
            device_id: [0; 8],
        },
        serde_json::json!({}),
    );
    adapter
        .apply_change(&delete, &merge_store)
        .await
        .expect("apply v4 chat index delete");

    let deleted = app.oneshot(json_get("/chats")).await.expect("deleted list");
    assert_eq!(deleted.status(), StatusCode::OK);
    assert_eq!(read_json(deleted).await.as_array().unwrap().len(), 0);
}
