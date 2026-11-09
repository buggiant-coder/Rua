use rua_server::profiles::ServerProfile;
use rua_server::AppState;
use tempfile::TempDir;

async fn build_test_state() -> (TempDir, AppState) {
    let tmp = TempDir::new().expect("tempdir");
    let path = tmp.path().to_path_buf();
    let state =
        tokio::task::spawn_blocking(move || AppState::from_tempdir(path, ServerProfile::Test))
            .await
            .expect("spawn_blocking join");
    (tmp, state)
}

#[tokio::test(flavor = "multi_thread")]
async fn appstate_memory_manager_is_some() {
    let (_tmp, state) = build_test_state().await;

    assert!(
        state.memory_manager_for("default").await.is_some(),
        "memory_manager must be Some after load"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn no_provider_no_panic() {
    let (_tmp, state) = build_test_state().await;
    let mm = state
        .memory_manager_for("default")
        .await
        .expect("mm is Some");

    let status = mm.health_check().await.expect("health_check ok");
    assert!(status.healthy);
}

#[tokio::test(flavor = "multi_thread")]
async fn memory_manager_structured_write_is_searchable() {
    let (_tmp, state) = build_test_state().await;
    let mm = state
        .memory_manager_for("default")
        .await
        .expect("mm is Some");

    mm.memory_write(rua_agent::memory_manager::MemoryWriteRequest {
        title: "Test memory".into(),
        agent_id: "test-agent".into(),
        kind: rua_agent::memory_manager::MemoryKind::ProjectFact,
        scope: rua_agent::memory_manager::MemoryScope::Project,
        content: "hello world".into(),
        confidence: Some(0.9),
        importance: Some(0.8),
        ttl_days: None,
        labels: vec![],
        search_terms: vec![],
        evidence: None,
        metadata: serde_json::Value::Null,
    })
    .await
    .expect("memory_write ok");

    let search_req = rua_agent::memory_manager::MemorySearchRequest {
        search_mode: rua_config::MemorySearchMode::default(),
        query: "hello".into(),
        agent_id: "test-agent".into(),
        top_k: 5,
    };
    let hits = mm
        .memory_search(search_req)
        .await
        .expect("memory_search ok");
    assert!(
        hits.iter().any(|hit| hit.content.contains("hello world")),
        "structured Memory search should recall written record"
    );
}
