use axum::body::{to_bytes, Body};
use axum::http::{Method, Request, StatusCode};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tower::ServiceExt;
use uuid::Uuid;

use super::support::{
    build_app, multipart_upload, multipart_upload_with_fields, read_json, request_json,
};
use rua_contract::agent::AgentInputMessage;
use rua_core::{ChatAddress, ChatScopeStore};
use rua_project::ProjectCreateRequest;
use rua_provider::config_loader::ProviderConfig;
use rua_server::services::chat_media::ChatMediaService;
use rua_server::services::conversation_authority_target::load_optional_chat_authority_placement;
use rua_server::services::device_context::current_sync_device_id;
use rua_session::WorkspaceSessionStore;

#[tokio::test]
async fn staged_media_collection_preserves_materialized_chat_media() {
    let test = build_app();
    let chat_id = "00000000-0000-4000-8000-000000000114";
    let address = ChatAddress::agent("default", chat_id);
    let staged = ChatMediaService
        .store_staged(
            &test.state.conversation_authority,
            &test.state.rua_root,
            address.clone(),
            b"temporary".to_vec(),
            Some("text/plain".into()),
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        ChatMediaService
            .collect_expired_staged(
                &test.state.conversation_authority,
                &test.state.rua_root,
                &address,
                Duration::ZERO
            )
            .await
            .unwrap(),
        1
    );
    assert!(ChatMediaService
        .open(&test.state.rua_root, address.clone(), &staged.media_id)
        .await
        .is_err());
    let staged = ChatMediaService
        .store_staged(
            &test.state.conversation_authority,
            &test.state.rua_root,
            address.clone(),
            b"accepted".to_vec(),
            Some("text/plain".into()),
            None,
        )
        .await
        .unwrap();
    test.state.wait_platform_initialization().await.unwrap();
    let submitted = test.state.conversation_input_intake.submit_delivery("default", rua_channel::core::console::ConsoleChatRequest {
        message: "accepted input".into(), chat_id: Some(chat_id.into()), model: None, is_reconnect: false, attachments: vec![],
        input: Some(vec![serde_json::json!({"role":"user","content":[{"type":"file","media_ref":staged.media_ref} ]})]), messages: None, extra: Default::default(), channel_kind: None,
    }).await;
    assert!(matches!(submitted, rua_server::services::conversation_input_intake::ConversationInputDeliveryOutcome::Delivered { .. }), "{submitted:?}");
    let scope = ChatScopeStore::resolve(&test.state.rua_root, address.clone());
    assert!(!scope
        .root()
        .join("media")
        .join(format!(".staged-{}.json", staged.media_id))
        .exists());
    assert_eq!(
        ChatMediaService
            .collect_expired_staged(
                &test.state.conversation_authority,
                &test.state.rua_root,
                &address,
                Duration::ZERO
            )
            .await
            .unwrap(),
        0
    );
    assert!(ChatMediaService
        .open(&test.state.rua_root, address, &staged.media_id)
        .await
        .is_ok());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_chat_media_round_trip_uses_agent_workspace_and_uuid_identity() {
    let test = build_app();
    let payload = b"agent-chat-media".to_vec();
    let address = ChatAddress::agent("default", "chat-a");
    let scope = ChatScopeStore::resolve(&test.state.rua_root, address.clone());
    test.state
        .conversation_authority
        .ensure_initial_placement(&test.state, &address)
        .await
        .expect("materialize the Agent Chat authority state");
    let initial_header = WorkspaceSessionStore::new(scope.root())
        .load_chat_header(address.chat_id().as_str())
        .await
        .expect("read initial Agent Chat header")
        .expect("authority initialization materializes an Agent Chat");
    assert_eq!(initial_header.turn_index.latest_turn, 0);
    assert_eq!(initial_header.extra["selected_agent_id"], "default");

    let response = multipart_upload(
        &test.app,
        "/api/agents/default/chats/chat-a/media",
        "file",
        "../private photo.png",
        payload.clone(),
    )
    .await;

    let status = response.status();
    let body = read_json(response).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let media_id = body["media_id"].as_str().expect("media id");
    Uuid::parse_str(media_id).expect("UUID-only media id");
    assert_eq!(body["media_ref"], format!("media/{media_id}"));
    assert_eq!(body["original_filename"], "private photo.png");
    assert_eq!(body["byte_size"], payload.len());

    let local_device_id = current_sync_device_id(&test.state.sync_device).expect("local Device id");
    let placement =
        load_optional_chat_authority_placement(scope.root(), address.chat_id().as_str())
            .await
            .expect("load placement")
            .expect("upload materializes Agent placement");
    assert_eq!(placement.device_id, local_device_id);
    assert!(
        WorkspaceSessionStore::new(scope.root())
            .load_chat_header(address.chat_id().as_str())
            .await
            .expect("read repaired Agent Chat header")
            .is_some(),
        "upload repairs the placement-only Agent Chat state"
    );
    assert_eq!(
        body["authority_epoch"],
        placement.authority_epoch.to_string()
    );

    let stored = test
        .state
        .rua_root
        .join("workspaces/default/media")
        .join(media_id);
    assert_eq!(std::fs::read(&stored).expect("stored Agent media"), payload);
    assert_eq!(
        std::fs::read_dir(stored.parent().expect("media parent"))
            .expect("media entries")
            .count(),
        2,
        "atomic upload must leave only the Media file and MIME sidecar"
    );

    let response = test
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/api/agents/default/chats/chat-a/media/{media_id}"))
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("read media");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()["content-type"],
        "application/octet-stream"
    );
    assert_eq!(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("media body"),
        payload
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn project_chat_media_round_trip_uses_project_workspace_only() {
    let test = build_app();
    test.state
        .provider_cells
        .cell("fixture")
        .write(
            serde_json::from_value::<ProviderConfig>(serde_json::json!({
                "id": "fixture",
                "name": "Fixture",
                "base_url": "https://example.invalid/v1",
                "api_key": "test-key",
                "chat_model": "OpenAIChatModel",
                "models": [{
                    "id": "project-control-model",
                    "name": "Project Control Model"
                }]
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    test.state
        .provider_manager
        .reload_provider("fixture")
        .await
        .unwrap();
    let payload = b"project-chat-media".to_vec();
    let chat_id = "00000000-0000-4000-8000-000000000401";
    let resource_root = test.state.rua_root.join("external-project-resource");
    std::fs::create_dir_all(&resource_root).expect("resource root");
    std::fs::write(resource_root.join("keep.txt"), b"untouched").expect("resource sentinel");
    let local_device_id = current_sync_device_id(&test.state.sync_device).expect("local Device id");
    let created = test
        .state
        .project_catalog
        .create_bound(ProjectCreateRequest {
            resource_device_id: local_device_id.clone(),
            resource_root: resource_root.to_string_lossy().to_string(),
            display_name: Some("Media Project".to_string()),
        })
        .expect("create Project");
    let project_id = created.project.project_id.to_string();
    let address = ChatAddress::project(project_id.clone(), chat_id);
    let scope = ChatScopeStore::resolve(&test.state.rua_root, address.clone());
    let model = serde_json::json!({
        "provider_id": "fixture",
        "model": "project-control-model"
    });
    let model_json = model.to_string();
    let response = multipart_upload_with_fields(
        &test.app,
        &format!("/api/projects/{project_id}/chats/{chat_id}/media"),
        &[("agent_id", "default"), ("model", model_json.as_str())],
        "file",
        "requirements.txt",
        payload.clone(),
    )
    .await;

    let status = response.status();
    let body = read_json(response).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let media_id = body["media_id"].as_str().expect("media id");
    Uuid::parse_str(media_id).expect("UUID-only media id");
    let stored = test
        .state
        .rua_root
        .join("projects")
        .join(&project_id)
        .join("media")
        .join(media_id);
    assert_eq!(
        std::fs::read(stored).expect("stored Project media"),
        payload
    );
    assert_eq!(
        std::fs::read(resource_root.join("keep.txt")).expect("resource sentinel"),
        b"untouched"
    );
    assert_eq!(
        std::fs::read_dir(&resource_root)
            .expect("resource root entries")
            .count(),
        1,
        "Project Resource Root must not receive Chat Media"
    );
    assert!(
        load_optional_chat_authority_placement(scope.root(), address.chat_id().as_str())
            .await
            .unwrap()
            .is_none()
    );
    assert!(WorkspaceSessionStore::new(scope.root())
        .load_chat_header(chat_id)
        .await
        .unwrap()
        .is_none());
    assert!(body.get("authority_epoch").is_none());
    let submit = request_json(
        &test.app,
        Method::POST,
        &format!("/api/projects/{project_id}/chats/{chat_id}/console/submit"),
        serde_json::json!({
            "agent_id": "default",
            "chat_id": chat_id,
            "input": [{
                "id": "user-media-submit",
                "role": "user",
                "content": [
                    {"type": "text", "text": "inspect"},
                    {"type": "image", "media_ref": body["media_ref"]}
                ]
            }],
            "submission_id": "submission-media",
            "channel": "console",
            "stream": true,
            "model": model
        }),
    )
    .await;
    let status = submit.status();
    let submit = read_json(submit).await;
    assert_eq!(status, StatusCode::OK, "{submit}");
    assert_eq!(submit["accepted"], true);
}

#[test]
fn project_managed_media_materializes_from_the_project_workspace() {
    let rua_root = std::path::Path::new("/tmp/rua-media-scope");
    let address = ChatAddress::project("project-1", "chat-1");
    let mut messages = vec![AgentInputMessage {
        role: "user".to_string(),
        content: vec![rua_core::ContentPart::Image {
            url: "media/00000000-0000-4000-8000-000000000001".to_string(),
            media_type: Some("image/png".to_string()),
            alt: None,
        }],
    }];

    ChatMediaService.materialize_message_parts(rua_root, &address, &mut messages);

    assert!(matches!(
        &messages[0].content[0],
        rua_core::ContentPart::Image { url, .. }
            if url == "/tmp/rua-media-scope/projects/project-1/media/00000000-0000-4000-8000-000000000001"
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scoped_media_rejects_invalid_id_and_old_console_upload_is_absent() {
    let test = build_app();

    let invalid = test
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/agents/default/chats/chat-a/media/not-a-uuid")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("invalid media request");
    assert_eq!(invalid.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let address = ChatAddress::agent("default", "chat-a");
    let scope = ChatScopeStore::resolve(&test.state.rua_root, address.clone());
    assert!(
        load_optional_chat_authority_placement(scope.root(), address.chat_id().as_str())
            .await
            .expect("read placement after invalid Media request")
            .is_none(),
        "Media reads must not materialize an unknown Chat"
    );

    let old = multipart_upload(
        &test.app,
        "/api/console/upload",
        "file",
        "old.txt",
        b"old".to_vec(),
    )
    .await;
    assert_eq!(old.status(), StatusCode::NOT_FOUND);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn media_open_rejects_symlink_and_orphan_gc_preserves_references() {
    let test = build_app();
    let address = ChatAddress::agent("default", "chat-gc");
    let media_dir = test.state.rua_root.join("workspaces/default/media");
    std::fs::create_dir_all(&media_dir).expect("media dir");
    let referenced = "00000000-0000-4000-8000-000000000301";
    let orphan = "00000000-0000-4000-8000-000000000302";
    let rescued = "00000000-0000-4000-8000-000000000303";
    for media_id in [referenced, orphan, rescued] {
        std::fs::write(media_dir.join(media_id), b"media").expect("media file");
        std::fs::write(
            media_dir.join(format!(".{media_id}.media.json")),
            br#"{"mime_type":"image/png"}"#,
        )
        .expect("sidecar");
    }
    let checks = AtomicUsize::new(0);
    let report = rua_server::services::chat_media::ChatMediaService
        .collect_orphans(
            &test.state.rua_root,
            address.clone(),
            Duration::ZERO,
            |media_id| {
                if media_id.to_string() == referenced {
                    true
                } else if media_id.to_string() == rescued {
                    checks.fetch_add(1, Ordering::SeqCst) > 0
                } else {
                    false
                }
            },
        )
        .expect("collect orphans");
    assert_eq!(report.removed, 1);
    assert_eq!(report.restored_after_recheck, 1);
    assert!(media_dir.join(referenced).is_file());
    assert!(media_dir.join(rescued).is_file());
    assert!(!media_dir.join(orphan).exists());
    assert!(!media_dir.join(format!(".{orphan}.media.json")).exists());

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let target = test.state.rua_root.join("outside-media");
        std::fs::write(&target, b"secret").expect("outside file");
        let link_id = "00000000-0000-4000-8000-000000000304";
        symlink(&target, media_dir.join(link_id)).expect("media symlink");
        let error = rua_server::services::chat_media::ChatMediaService
            .open(&test.state.rua_root, address, link_id)
            .await
            .expect_err("symlink Media must be rejected");
        assert!(matches!(
            error,
            rua_server::services::chat_media::ChatMediaError::NotFound
        ));
    }
}
