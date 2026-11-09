use rua_server::profiles::ServerProfile;
use rua_server::AppState;
use rua_workspace::ComponentState;
use tempfile::TempDir;

#[tokio::test]
async fn single_thread_state_construction_supports_explicit_platform_readiness() {
    let tmp = TempDir::new().unwrap();
    let state = AppState::from_tempdir(tmp.path().to_path_buf(), ServerProfile::Test);
    tokio::time::timeout(
        std::time::Duration::from_secs(15),
        state.wait_platform_initialization(),
    )
    .await
    .expect("platform initialization must not require a blocked executor")
    .unwrap();
    state.wait_platform_initialization().await.unwrap();
    let runtime = state.registry.get_or_create("default").await.unwrap();
    assert_eq!(
        runtime.service_manager().service_state("channels").await,
        Some(ComponentState::Ready)
    );
    assert_eq!(
        runtime.service_manager().service_state("skills").await,
        Some(ComponentState::Ready)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn platform_services_registered_and_ready() {
    let tmp = TempDir::new().expect("tempdir");
    let state = AppState::from_tempdir(tmp.path().to_path_buf(), ServerProfile::Test);
    state.wait_platform_initialization().await.unwrap();

    let runtime = state
        .registry
        .get_or_create("default")
        .await
        .expect("get_or_create default ok");

    let sm = runtime.service_manager();

    // `AppState` composes the platform capabilities during construction: the
    // registration and the Runtime-phase start both happen there, so Ready here
    // is evidence that the real adapters started, not that a placeholder did.
    for name in ["channels", "skills"] {
        let st = sm.service_state(name).await;
        assert!(
            matches!(st, Some(ComponentState::Ready)),
            "{name} should be Ready after AppState composition, got: {st:?}"
        );
    }

    // Capabilities with no per-Agent lifecycle must not be accounted for at all.
    for absent in ["provider", "tools", "cron", "session_store"] {
        assert!(
            sm.service_state(absent).await.is_none(),
            "{absent} has no per-Agent lifecycle and must not be a workspace service"
        );
    }

    runtime.initialize_core().await.expect("core phase ok");
    runtime
        .initialize_metadata()
        .await
        .expect("metadata phase ok");
    runtime
        .initialize_runtime()
        .await
        .expect("runtime phase ok");

    for name in ["channels", "skills"] {
        let st = sm.service_state(name).await;
        assert!(
            matches!(st, Some(ComponentState::Ready)),
            "{name} should still be Ready after re-initialization, got: {st:?}"
        );
    }
}
