use rua_prompts::{file_store::PromptFileChange, library::PromptLibraryPublisher, PromptLibrary};
use std::io::Write;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Recorder(Mutex<Vec<PromptFileChange>>);
#[async_trait::async_trait]
impl PromptLibraryPublisher for Recorder {
    async fn publish(&self, changes: Vec<PromptFileChange>) {
        self.0.lock().unwrap().extend(changes);
    }
}
fn archive(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, bytes) in files {
        zip.start_file(*name, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(bytes).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

#[tokio::test]
async fn all_file_mutations_publish_canonical_committed_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let recorder = Arc::new(Recorder::default());
    let lib = PromptLibrary::new(dir.path().into()).with_publisher(recorder.clone());
    lib.update_file("old", " bytes ").await.unwrap();
    lib.rename_file("old", "new").await.unwrap();
    lib.delete_file("new").await.unwrap();
    std::fs::create_dir(dir.path().join("prompt_library/Mixed")).unwrap();
    lib.create_subdir_file("mixed", Some("a"), "")
        .await
        .unwrap();
    lib.write_subdir_file("MIXED", "a", " subdir ")
        .await
        .unwrap();
    lib.rename_subdir_file("mixed", "a", "b").await.unwrap();
    lib.delete_subdir_file("mixed", "b").await.unwrap();
    let changes = recorder.0.lock().unwrap();
    assert_eq!(
        changes.iter().map(|c| c.path.as_str()).collect::<Vec<_>>(),
        vec![
            "old.md",
            "old.md",
            "new.md",
            "new.md",
            "Mixed/a.md",
            "Mixed/a.md",
            "Mixed/a.md",
            "Mixed/b.md",
            "Mixed/b.md"
        ]
    );
    assert_eq!(changes[0].content.as_deref(), Some(" bytes "));
    assert_eq!(changes[7].content.as_deref(), Some(" subdir "));
    assert!(changes[8].content.is_none());
}

#[tokio::test]
async fn import_partial_failure_publishes_completed_files_and_keeps_them_recoverable() {
    let dir = tempfile::tempdir().unwrap();
    let recorder = Arc::new(Recorder::default());
    let lib = PromptLibrary::new(dir.path().into()).with_publisher(recorder.clone());
    std::fs::create_dir_all(dir.path().join("prompt_library/z.md")).unwrap();
    let error = lib
        .upload(&archive(&[("a.md", b" first "), ("z.md", b"second")]))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("after 1 committed"));
    assert_eq!(
        std::fs::read_to_string(dir.path().join("prompt_library/a.md")).unwrap(),
        " first "
    );
    assert_eq!(recorder.0.lock().unwrap().len(), 1);
    let guard = rua_prompts::file_store::PromptFileStore::for_library(dir.path())
        .lock()
        .await;
    assert!(guard.local_revision("a.md").unwrap().is_some());
}

#[tokio::test]
async fn invalid_import_is_rejected_before_any_file_commits() {
    let dir = tempfile::tempdir().unwrap();
    let lib = PromptLibrary::new(dir.path().into());
    assert!(lib
        .upload(&archive(&[("a.md", b"first"), ("z.md", &[255])]))
        .await
        .is_err());
    assert!(!dir.path().join("prompt_library/a.md").exists());
}

#[tokio::test]
async fn explicit_restore_only_publishes_missing_files_and_startup_stays_local() {
    let dir = tempfile::tempdir().unwrap();
    let recorder = Arc::new(Recorder::default());
    let lib = PromptLibrary::new(dir.path().into()).with_publisher(recorder.clone());
    lib.restore_builtin_now().unwrap();
    assert!(recorder.0.lock().unwrap().is_empty());
    lib.delete_file("10_AGENT_SAFETY").await.unwrap();
    lib.update_file("20_AGENT_WORKFLOW", "user edit")
        .await
        .unwrap();
    recorder.0.lock().unwrap().clear();
    lib.restore_builtin().await.unwrap();
    assert_eq!(recorder.0.lock().unwrap().len(), 1);
    assert_eq!(recorder.0.lock().unwrap()[0].path, "10_AGENT_SAFETY.md");
    assert_eq!(
        lib.get_file("20_AGENT_WORKFLOW")
            .await
            .unwrap()
            .unwrap()
            .content,
        "user edit"
    );
}

#[tokio::test]
async fn nested_import_preserves_bytes_and_does_not_export_private_state() {
    let dir = tempfile::tempdir().unwrap();
    let recorder = Arc::new(Recorder::default());
    let lib = PromptLibrary::new(dir.path().into()).with_publisher(recorder.clone());
    lib.upload(&archive(&[
        ("root.md", b"root"),
        ("GROUP/deep/a.md", b" nested "),
        ("image.bin", &[255]),
    ]))
    .await
    .unwrap();
    assert_eq!(recorder.0.lock().unwrap().len(), 2);
    let bytes = lib.download().await.unwrap();
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    for i in 0..zip.len() {
        assert!(!zip.by_index(i).unwrap().name().contains("state"));
    }
}

#[tokio::test]
async fn malformed_wrapper_path_is_rejected_before_import() {
    let dir = tempfile::tempdir().unwrap();
    let lib = PromptLibrary::new(dir.path().into());
    let error = lib
        .upload(&archive(&[
            ("wrapper/a.md", b"first"),
            ("wrapper//bad.txt", b"bad"),
        ]))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("unsafe path"));
    assert!(!dir.path().join("prompt_library/a.md").exists());
}
