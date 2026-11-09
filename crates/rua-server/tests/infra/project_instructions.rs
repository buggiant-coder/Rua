use std::io::Write;
use std::sync::{Arc, Mutex};

use rua_core::ProjectId;
use rua_server::services::project_instructions::{
    load_project_instructions, PROJECT_INSTRUCTIONS_MAX_BYTES,
};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::Layer;

#[derive(Clone, Default)]
struct CapturedWriter {
    chunks: Arc<Mutex<Vec<String>>>,
}

impl CapturedWriter {
    fn output(&self) -> String {
        self.chunks.lock().unwrap().join("")
    }
}

impl Write for CapturedWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.chunks
            .lock()
            .unwrap()
            .push(String::from_utf8_lossy(bytes).to_string());
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn project_instructions_are_read_fresh_from_only_the_project_root() {
    let host = tempfile::tempdir().unwrap();
    let root = host.path().join("project");
    tokio::fs::create_dir_all(&root).await.unwrap();
    let project_id = ProjectId::new("project-1");

    tokio::fs::write(host.path().join("AGENTS.md"), "parent")
        .await
        .unwrap();
    assert!(load_project_instructions(&root, &project_id)
        .await
        .is_none());

    tokio::fs::create_dir_all(root.join("nested"))
        .await
        .unwrap();
    tokio::fs::write(root.join("nested/AGENTS.md"), "nested")
        .await
        .unwrap();
    for (name, content) in [
        ("AGENTS.override.md", "override"),
        ("AGENTS.local.md", "fallback"),
        ("CLAUDE.md", "vendor"),
        ("CODEX.md", "vendor"),
    ] {
        tokio::fs::write(root.join(name), content).await.unwrap();
    }
    assert!(load_project_instructions(&root, &project_id)
        .await
        .is_none());

    tokio::fs::write(root.join("AGENTS.md"), "first")
        .await
        .unwrap();
    let first = load_project_instructions(&root, &project_id).await.unwrap();
    assert_eq!(first.content, "first");

    tokio::fs::write(root.join("AGENTS.md"), "second")
        .await
        .unwrap();
    let second = load_project_instructions(&root, &project_id).await.unwrap();
    assert_eq!(second.content, "second");
}

#[tokio::test]
async fn project_instructions_are_truncated_at_the_context_limit() {
    let root = tempfile::tempdir().unwrap();
    let project_id = ProjectId::new("project-1");
    let source = "x".repeat(PROJECT_INSTRUCTIONS_MAX_BYTES as usize + 17);
    tokio::fs::write(root.path().join("AGENTS.md"), &source)
        .await
        .unwrap();

    let writer = CapturedWriter::default();
    let captured = writer.clone();
    let subscriber = tracing_subscriber::registry().with(
        tracing_subscriber::fmt::layer()
            .with_writer(move || writer.clone())
            .with_ansi(false)
            .with_filter(tracing_subscriber::filter::LevelFilter::WARN),
    );
    let _guard = tracing::subscriber::set_default(subscriber);
    let instructions = load_project_instructions(root.path(), &project_id)
        .await
        .unwrap();
    assert_eq!(instructions.source_bytes, source.len() as u64);
    assert_eq!(
        instructions.content.len(),
        PROJECT_INSTRUCTIONS_MAX_BYTES as usize
    );
    assert!(instructions.truncated);
    let output = captured.output();
    assert!(output.contains("exceeded 32 KiB"), "got logs:\n{output}");
    assert!(
        output.contains("project_id=project-1"),
        "got logs:\n{output}"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn project_instructions_follow_root_symlink_retargeting() {
    use std::os::unix::fs::symlink;

    let host = tempfile::tempdir().unwrap();
    let first = host.path().join("first");
    let second = host.path().join("second");
    let binding = host.path().join("bound-project");
    tokio::fs::create_dir_all(&first).await.unwrap();
    tokio::fs::create_dir_all(&second).await.unwrap();
    tokio::fs::write(first.join("AGENTS.md"), "first target")
        .await
        .unwrap();
    tokio::fs::write(second.join("AGENTS.md"), "second target")
        .await
        .unwrap();
    symlink(&first, &binding).unwrap();
    let project_id = ProjectId::new("project-symlink");

    assert_eq!(
        load_project_instructions(&binding, &project_id)
            .await
            .unwrap()
            .content,
        "first target"
    );
    tokio::fs::remove_file(&binding).await.unwrap();
    symlink(&second, &binding).unwrap();
    assert_eq!(
        load_project_instructions(&binding, &project_id)
            .await
            .unwrap()
            .content,
        "second target"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn unreadable_project_instructions_are_non_fatal() {
    use std::os::unix::fs::PermissionsExt;

    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("AGENTS.md");
    tokio::fs::write(&path, "unreadable").await.unwrap();
    let original = std::fs::metadata(&path).unwrap().permissions();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();

    let result =
        load_project_instructions(root.path(), &ProjectId::new("project-unreadable")).await;

    std::fs::set_permissions(&path, original).unwrap();
    assert!(result.is_none());
}
