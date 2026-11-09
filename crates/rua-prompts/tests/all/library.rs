use rua_prompts::library::{PromptCreate, PromptUpdate};
use rua_prompts::PromptLibrary;
use tempfile::TempDir;

fn fresh_lib() -> (PromptLibrary, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    let lib = PromptLibrary::new(tmp.path().to_path_buf());
    (lib, tmp)
}

#[test]
fn template_copy_validates_all_files_before_writing_and_never_restores_deleted_builtins() {
    let (lib, tmp) = fresh_lib();
    let root = tmp.path().join("prompt_library");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("CUSTOM.md"), "custom content").unwrap();
    let workspace = tmp.path().join("workspace");
    let files = vec!["CUSTOM.md".into(), "10_AGENT_SAFETY.md".into()];
    assert!(lib.copy_files_to_workspace(&files, &workspace).is_err());
    assert!(!workspace.exists());
    assert!(!root.join("10_AGENT_SAFETY.md").exists());
    assert!(lib
        .validate_template_files(&["../outside.md".into()])
        .is_err());
    lib.copy_files_to_workspace(&["CUSTOM.md".into()], &workspace)
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(workspace.join("prompts/CUSTOM.md")).unwrap(),
        "custom content"
    );
    lib.validate_template_files(&[]).unwrap();
}

#[tokio::test]
async fn list_files_empty_when_no_lib_dir() {
    let (lib, _tmp) = fresh_lib();
    let files = lib.list_files().await.expect("list_files ok");
    assert!(files.is_empty());
}

#[tokio::test]
async fn update_then_list_and_get_file() {
    let (lib, _tmp) = fresh_lib();
    let written = lib
        .update_file("HELLO", "hi there\n")
        .await
        .expect("update_file ok");
    assert_eq!(written.name, "HELLO.md");

    assert_eq!(written.content, "hi there");

    let files = lib.list_files().await.expect("list_files ok");
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].name, "HELLO.md");

    let fetched = lib
        .get_file("HELLO")
        .await
        .expect("get_file ok")
        .expect("file present");
    assert_eq!(fetched.content, "hi there");
}

#[tokio::test]
async fn rename_and_delete_file() {
    let (lib, _tmp) = fresh_lib();
    lib.update_file("a", "x").await.expect("write a");
    let renamed = lib.rename_file("a", "b").await.expect("rename ok");
    assert_eq!(renamed.name, "b.md");

    assert!(lib.get_file("a").await.unwrap().is_none());
    assert!(lib.get_file("b").await.unwrap().is_some());

    lib.delete_file("b").await.expect("delete ok");
    assert!(lib.get_file("b").await.unwrap().is_none());
}

#[tokio::test]
async fn validate_name_rejects_traversal() {
    let (lib, _tmp) = fresh_lib();
    let err = lib
        .get_file("../escape")
        .await
        .expect_err("traversal must be rejected");
    let msg = format!("{err}");
    assert!(msg.contains("Invalid name"), "unexpected msg: {msg}");
}

#[tokio::test]
async fn delete_missing_file_is_not_found() {
    let (lib, _tmp) = fresh_lib();
    let err = lib
        .delete_file("missing")
        .await
        .expect_err("delete missing must fail");
    let msg = format!("{err}");
    assert!(msg.to_lowercase().contains("not found"), "msg: {msg}");
}

#[tokio::test]
async fn subdir_create_resolves_uppercase_and_subdirs_info_lists_paired_md() {
    let (lib, _tmp) = fresh_lib();

    lib.update_file("group", "header").await.expect("group.md");
    let created = lib
        .create_subdir_file("group", Some("child"), "ignored")
        .await
        .expect("create subdir file");
    assert_eq!(created.name, "child.md");

    assert!(created.path.contains("/GROUP/") || created.path.contains("\\GROUP\\"));

    let files = lib
        .list_subdir_files("group")
        .await
        .expect("list subdir files");
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].name, "child.md");

    let info = lib.list_subdirs_info().await.expect("subdirs info");
    assert_eq!(info.get("group.md"), Some(&1));
}

#[tokio::test]
async fn subdir_delete_last_file_removes_directory() {
    let (lib, _tmp) = fresh_lib();
    lib.create_subdir_file("topic", Some("only"), "")
        .await
        .expect("create only");
    lib.delete_subdir_file("topic", "only")
        .await
        .expect("delete only");
    let files = lib
        .list_subdir_files("topic")
        .await
        .expect("list after delete");
    assert!(files.is_empty());
}

#[tokio::test]
async fn prompt_crud_round_trip() {
    let (lib, _tmp) = fresh_lib();

    let created = lib
        .create_prompt(PromptCreate {
            title: "first".into(),
            content: "body".into(),
            category: Some("test".into()),
            tags: vec!["a".into()],
            extra: serde_json::Map::new(),
        })
        .await
        .expect("create_prompt ok");
    assert_eq!(created.title, "first");
    assert!(!created.id.is_empty());

    let listed = lib.list_prompts().await.expect("list ok");
    assert_eq!(listed.len(), 1);

    let fetched = lib
        .get_prompt(&created.id)
        .await
        .expect("get ok")
        .expect("present");
    assert_eq!(fetched.id, created.id);

    let updated = lib
        .update_prompt(
            &created.id,
            PromptUpdate {
                title: Some("renamed".into()),
                content: None,
                category: None,
                tags: Some(vec!["b".into()]),
                extra: serde_json::Map::new(),
            },
        )
        .await
        .expect("update ok");
    assert_eq!(updated.title, "renamed");
    assert_eq!(updated.tags, vec!["b".to_string()]);

    lib.delete_prompt(&created.id).await.expect("delete ok");
    assert!(lib.list_prompts().await.unwrap().is_empty());
}

#[tokio::test]
async fn restore_builtin_seeds_all_files() {
    let (lib, tmp) = fresh_lib();
    let result = lib.restore_builtin().await.expect("restore ok");
    assert!(
        result.message.contains("Seeded"),
        "expected seed msg, got: {}",
        result.message
    );
    assert!(!result.seeded.is_empty(), "should have seeded entries");

    let lib_dir = tmp.path().join("prompt_library");
    let expected_top = [
        "10_AGENT_SAFETY.md",
        "20_AGENT_WORKFLOW.md",
        "30_EXECUTION_REPORTING.md",
        "40_STRUCTURED_MEMORY.md",
        "50_AGENT_RESPONSE.md",
        "60_MARKDOWN_PLUS.md",
        "90_DEFAULT_PROFILE.md",
        "91_QA_PROFILE.md",
        "92_QA_WORKFLOW.md",
    ];
    for name in &expected_top {
        assert!(lib_dir.join(name).is_file(), "missing top-level: {name}");
    }
    assert!(
        !lib_dir.join("AGENT_MEMORY.md").exists(),
        "AGENT_MEMORY.md must not be generated as a prompt-library file"
    );
    assert!(
        !lib_dir.join("AGENT_MODE.md").exists(),
        "AGENT_MODE.md must not be generated as a prompt-library file"
    );
    assert!(
        !lib_dir.join("AGENT_MODE").exists(),
        "AGENT_MODE subdirectory must not be generated as prompt-library files"
    );

    let profile = std::fs::read_to_string(lib_dir.join("90_DEFAULT_PROFILE.md")).unwrap();
    assert!(!profile.is_empty(), "default profile should have content");
}

#[tokio::test]
async fn restore_builtin_idempotent_no_overwrite() {
    let (lib, tmp) = fresh_lib();
    lib.restore_builtin().await.expect("first seed");

    let profile_path = tmp.path().join("prompt_library/90_DEFAULT_PROFILE.md");
    std::fs::write(&profile_path, "user edit").unwrap();

    let result = lib.restore_builtin().await.expect("second seed");
    assert!(
        result.message.contains("already complete"),
        "expected no-op msg, got: {}",
        result.message
    );
    assert!(result.seeded.is_empty(), "no new files should be seeded");

    let content = std::fs::read_to_string(&profile_path).unwrap();
    assert_eq!(content, "user edit", "user edit must not be overwritten");
}
