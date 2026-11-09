use rua_server::profiles::ServerProfile;
use rua_server::AppState;
use tempfile::TempDir;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn voice_channel_listed_in_channel_manager() {
    let tmp = TempDir::new().expect("tempdir");
    let state = AppState::from_tempdir(tmp.path().to_path_buf(), ServerProfile::Test);

    let cm = state
        .channel_manager_for("default")
        .await
        .expect("channel_manager should be available for default agent");
    let ids = cm.channel_ids().await;
    assert!(
        ids.contains(&"voice".to_string()),
        "channel_ids should contain 'voice', got: {ids:?}"
    );
}
