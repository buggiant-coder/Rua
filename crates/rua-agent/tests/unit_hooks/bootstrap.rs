use agent_types::{ContentPart, ModelDialogueItem, Role};
use react_agent::ModelCallSectionKind;
use rua_agent::BootstrapContextContributor;

fn msg(role: Role, text: &str) -> ModelDialogueItem {
    ModelDialogueItem::text(role, text)
}

fn image_user_msg(url: &str) -> ModelDialogueItem {
    ModelDialogueItem {
        id: None,
        role: Role::User,
        content: vec![ContentPart::Image {
            url: url.to_string(),
            media_type: Some("image/png".to_string()),
            alt: None,
        }],
        name: None,
        created_at: None,
        metadata: serde_json::Map::new(),
    }
}

#[tokio::test]
async fn bootstrap_contributes_typed_context_for_first_user_interaction() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("BOOTSTRAP.md"), "project guidance").unwrap();
    let contributor = BootstrapContextContributor::new(dir.path().to_path_buf());

    let section = contributor
        .contribute(
            &[
                msg(Role::System, "system one"),
                msg(Role::System, "system two"),
                msg(Role::User, "hello"),
            ],
            0,
        )
        .await
        .expect("bootstrap section");

    assert_eq!(section.kind, ModelCallSectionKind::BootstrapContext);
    assert!(section.content.contains("project guidance"));
}

#[tokio::test]
async fn bootstrap_skips_missing_file_flag_non_initial_and_existing_conversation() {
    let missing_dir = tempfile::tempdir().unwrap();
    let missing = BootstrapContextContributor::new(missing_dir.path().to_path_buf());
    assert!(missing
        .contribute(&[msg(Role::User, "hello")], 0)
        .await
        .is_none());

    let flagged_dir = tempfile::tempdir().unwrap();
    std::fs::write(flagged_dir.path().join("BOOTSTRAP.md"), "guidance").unwrap();
    std::fs::write(flagged_dir.path().join(".bootstrap_completed"), "").unwrap();
    let flagged = BootstrapContextContributor::new(flagged_dir.path().to_path_buf());
    assert!(flagged
        .contribute(&[msg(Role::User, "hello")], 0)
        .await
        .is_none());

    let later_dir = tempfile::tempdir().unwrap();
    std::fs::write(later_dir.path().join("BOOTSTRAP.md"), "guidance").unwrap();
    let later = BootstrapContextContributor::new(later_dir.path().to_path_buf());
    assert!(later
        .contribute(&[msg(Role::User, "hello")], 1)
        .await
        .is_none());
    assert!(later
        .contribute(
            &[
                msg(Role::User, "first"),
                msg(Role::Assistant, "already answered"),
            ],
            0,
        )
        .await
        .is_none());
}

#[tokio::test]
async fn bootstrap_writes_completed_flag_only_after_contribution() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("BOOTSTRAP.md"), "guidance").unwrap();
    let contributor = BootstrapContextContributor::new(dir.path().to_path_buf());

    contributor.finish().await;
    assert!(!dir.path().join(".bootstrap_completed").exists());
    contributor
        .contribute(&[msg(Role::User, "hello")], 0)
        .await
        .expect("bootstrap contribution");
    assert!(!dir.path().join(".bootstrap_completed").exists());

    contributor.finish().await;
    assert!(dir.path().join(".bootstrap_completed").exists());
}

#[tokio::test]
async fn bootstrap_contribution_does_not_mutate_media_only_user_message() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("BOOTSTRAP.md"), "media guidance").unwrap();
    let contributor = BootstrapContextContributor::new(dir.path().to_path_buf());
    let user = image_user_msg("https://example.test/input.png");

    let section = contributor
        .contribute(std::slice::from_ref(&user), 0)
        .await
        .expect("bootstrap contribution");

    assert!(section.content.contains("media guidance"));
    assert_eq!(user.content.len(), 1);
    assert!(matches!(user.content[0], ContentPart::Image { .. }));
}

#[tokio::test]
async fn bootstrap_second_contributor_skips_after_completed_flag() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("BOOTSTRAP.md"), "project guidance").unwrap();

    let first = BootstrapContextContributor::new(dir.path().to_path_buf());
    first
        .contribute(&[msg(Role::User, "hello")], 0)
        .await
        .expect("first contribution");
    first.finish().await;

    let second = BootstrapContextContributor::new(dir.path().to_path_buf());
    assert!(second
        .contribute(&[msg(Role::User, "hello")], 0)
        .await
        .is_none());
}
