use axum::http::{Method, StatusCode};
use axum::Router;
use serde_json::{json, Value};

use super::support::{
    build_app, json as response_json, local_device_id, request, wait_for_chat_idle, TestApp,
};

const STANDALONE: &str = "00000000-0000-4000-8000-000000000331";
const PHOTOS: &str = "00000000-0000-4000-8000-000000000332";
const ALBUM: &str = "00000000-0000-4000-8000-000000000333";
const ARCHIVED: &str = "00000000-0000-4000-8000-000000000334";

async fn create_project(test: &TestApp, name: &str) -> String {
    let root = test._temp.path().join(name);
    std::fs::create_dir(&root).unwrap();
    let project = call(
        &test.app,
        Method::POST,
        "/api/projects",
        Some(json!({
            "directories": [{"device_id": local_device_id(&test.state), "resource_root": root}],
            "display_name": name,
        })),
    )
    .await;
    project["project"]["project_id"]
        .as_str()
        .unwrap()
        .to_owned()
}

async fn call(app: &Router, method: Method, path: &str, body: Option<Value>) -> Value {
    let response = request(app, method, path, body).await;
    let status = response.status();
    let payload = response_json(response).await;
    assert!(status.is_success(), "{path}: {status} {payload}");
    payload
}

async fn assert_list(app: &Router, filter: &str, expected: &[&str], total: usize) {
    let path = format!("/api/chats?view=user&archived=false&{filter}");
    let response = request(app, Method::GET, &path, None).await;
    assert_eq!(response.status(), StatusCode::OK, "{path}");
    assert_eq!(
        response.headers()["x-total-count"].to_str().unwrap(),
        total.to_string()
    );
    let rows = response_json(response).await;
    let ids: Vec<_> = rows
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, expected, "{path}");
}

async fn assert_filtered_discovery(app: &Router, project: &str, collaboration_id: &str) {
    assert_list(app, "offset=0&limit=1", &[PHOTOS], 3).await;
    assert_list(app, "offset=1&limit=1", &[STANDALONE], 3).await;
    assert_list(app, "offset=2&limit=1", &[ALBUM], 3).await;
    assert_list(
        app,
        "without_project=true&agent_id=default",
        &[STANDALONE],
        1,
    )
    .await;
    assert_list(
        app,
        &format!("project_id={project}&agent_id=photographer"),
        &[PHOTOS],
        1,
    )
    .await;
    assert_list(
        app,
        &format!("project_id={project}&agent_id=default"),
        &[],
        0,
    )
    .await;
    for (query, id) in [
        ("archived=true&view=user", ARCHIVED),
        ("archived=false&view=collaboration", collaboration_id),
    ] {
        let rows = call(
            app,
            Method::GET,
            &format!("/api/chats?project_id={project}&{query}"),
            None,
        )
        .await;
        let ids: Vec<_> = rows
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, [id], "{query}");
    }
    for (agent, expected) in [("default", None), ("photographer", Some(PHOTOS))] {
        let path = format!("/api/chats/search?q=aperture&project_id={project}&agent_id={agent}&view=user&archived=false");
        let search = call(app, Method::GET, &path, None).await;
        let ids: Vec<_> = search["results"]
            .as_array()
            .unwrap()
            .iter()
            .map(|hit| hit["chat_id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, expected.into_iter().collect::<Vec<_>>(), "{path}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn discovery_survives_agent_switch_and_projection_rebuild_without_rewriting_turns() {
    let test = build_app();
    let project = create_project(&test, "Photos").await;
    let project_id = project.as_str();
    let albums = create_project(&test, "Albums").await;
    for (id, membership) in [
        (STANDALONE, None),
        (PHOTOS, Some(project_id)),
        (ALBUM, Some(albums.as_str())),
        (ARCHIVED, Some(project_id)),
    ] {
        let accepted = call(
            &test.app,
            Method::POST,
            &format!("/api/chats/{id}/console/submit"),
            Some(json!({
                "chat_id": id,
                "project_id": membership,
                "agent_id": "default",
                "message": "Record the aperture",
            })),
        )
        .await;
        assert_eq!(accepted["accepted"], true);
        wait_for_chat_idle(&test.state, id).await;
    }
    for id in [STANDALONE, PHOTOS] {
        call(
            &test.app,
            Method::PUT,
            &format!("/api/chats/{id}"),
            Some(json!({"pinned": true})),
        )
        .await;
    }
    call(
        &test.app,
        Method::PUT,
        "/api/chats/pin-order",
        Some(json!([
            {"id": PHOTOS, "pin_order": 1},
            {"id": STANDALONE, "pin_order": 2}
        ])),
    )
    .await;
    call(
        &test.app,
        Method::PUT,
        &format!("/api/chats/{ARCHIVED}"),
        Some(json!({"archived": true})),
    )
    .await;
    let history_path = format!("/api/chats/{PHOTOS}");
    let history = call(&test.app, Method::GET, &history_path, None).await;
    assert_eq!(history["agent_id"], "default");
    assert!(!history["messages"].as_array().unwrap().is_empty());
    let collaboration = test
        .state
        .collaboration
        .create(
            rua_server::services::collaboration::CollaborationCaller {
                address: rua_core::ChatAddress::new(PHOTOS),
                turn_id: history["messages"][0]["turn_id"].as_str().unwrap().into(),
                tool_call_id: "inspect-proof".into(),
                working_dir: test._temp.path().join("Photos"),
            },
            "default".into(),
            None,
            0,
        )
        .await
        .expect("create a real collaboration Chat");
    let child = collaboration.chat.chat_id().as_str();
    assert_list(
        &test.app,
        &format!("project_id={project_id}&agent_id=default"),
        &[PHOTOS],
        1,
    )
    .await;

    call(
        &test.app,
        Method::POST,
        "/api/agents",
        Some(json!({"id": "photographer", "name": "Photographer"})),
    )
    .await;
    call(
        &test.app,
        Method::PATCH,
        &format!("{history_path}/agent-selection"),
        Some(json!({"expected_agent_id": "default", "agent_id": "photographer"})),
    )
    .await;
    assert_filtered_discovery(&test.app, project_id, child).await;
    let workspace = test.state.chat_workspace();
    rua_session::chat_index::delete_many_sync(
        workspace.as_path(),
        &[
            PHOTOS.into(),
            STANDALONE.into(),
            ALBUM.into(),
            ARCHIVED.into(),
            child.into(),
        ],
    )
    .expect("simulate loss of the derived rows");
    assert!(
        rua_session::chat_index::get_sync(workspace.as_path(), PHOTOS)
            .unwrap()
            .is_none()
    );
    let service = rua_chat::ChatService::new(&test.state.rua_root);
    service
        .rebuild_index()
        .await
        .expect("rebuild discovery from saved Chats");
    assert_filtered_discovery(&test.app, project_id, child).await;
    let reopened = call(&test.app, Method::GET, &history_path, None).await;
    assert_eq!(reopened["agent_id"], "photographer");
    assert_eq!(reopened["project_id"], project_id);
    assert_eq!(reopened["messages"], history["messages"]);
    let original_turn = reopened["messages"][0]["turn_id"].as_str().unwrap();
    let assignment = service
        .load_turn_agent_assignment(&rua_core::ChatAddress::new(PHOTOS), original_turn)
        .await
        .unwrap();
    assert_eq!(assignment.agent_id, "default");
}
