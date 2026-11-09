use rua_memory::{IssueSeverity, MemoryMaintenance, ReindexScope};
use tempfile::TempDir;
use tokio::fs;

#[tokio::test]
async fn health_check_reports_missing_structured_store_and_index() {
    let tmp = TempDir::new().unwrap();
    let maintenance = MemoryMaintenance::new(tmp.path());

    let health = maintenance.health_check().await.unwrap();

    assert!(!health.index_ok);
    assert!(!health.source_files_ok);
    assert_eq!(health.document_count, 0);
    assert_eq!(health.index_size_bytes, 0);
    assert!(health.issues.contains(&"memory.db not found".to_string()));
    assert!(health
        .issues
        .contains(&"memory_vec.db not found (reindex needed)".to_string()));
    assert!(
        !health
            .issues
            .iter()
            .any(|issue| issue.contains("MEMORY.md")),
        "Removed Memory Markdown must not appear in health issues: {:?}",
        health.issues
    );
}

#[tokio::test]
async fn diagnose_maps_missing_structured_store_and_index_to_repair_plan() {
    let tmp = TempDir::new().unwrap();

    let plan = MemoryMaintenance::new(tmp.path()).diagnose().await.unwrap();

    assert_eq!(plan.estimated_duration_ms, 1000);
    assert!(plan.issues.iter().any(|issue| {
        issue.description == "Memory search index missing or corrupt"
            && matches!(issue.severity, IssueSeverity::High)
            && issue.fix == "Rebuild index from source files"
    }));
    assert!(plan.issues.iter().any(|issue| {
        issue.description == "Structured Memory store missing or corrupt"
            && matches!(issue.severity, IssueSeverity::High)
            && issue.fix == "Create or repair memory.db"
    }));
}

#[tokio::test]
async fn repair_creates_structured_stores_without_touching_removed_memory_markdown() {
    let tmp = TempDir::new().unwrap();
    let memory_path = tmp.path().join("MEMORY.md");
    fs::write(&memory_path, "user-authored memory")
        .await
        .unwrap();

    let actions = MemoryMaintenance::new(tmp.path()).repair().await.unwrap();

    assert!(tmp.path().join("memory.db").is_file());
    assert!(tmp.path().join("memory_vec.db").is_file());
    assert_eq!(
        fs::read_to_string(&memory_path).await.unwrap(),
        "user-authored memory",
        "repair must not overwrite existing MEMORY.md"
    );
    assert!(actions.contains(&"Created memory.db".to_string()));
    assert!(actions.contains(&"Created memory_vec.db".to_string()));
    assert!(!actions.contains(&"Created default MEMORY.md".to_string()));
}

#[tokio::test]
async fn repair_does_not_create_removed_memory_markdown_when_missing() {
    let tmp = TempDir::new().unwrap();

    let actions = MemoryMaintenance::new(tmp.path()).repair().await.unwrap();

    assert!(
        !tmp.path().join("MEMORY.md").exists(),
        "repair must not recreate removed root Memory markdown"
    );
    assert!(
        !tmp.path().join("MEMORY").exists(),
        "repair must not recreate removed Memory markdown directory"
    );
    assert!(
        !tmp.path().join("memory").exists(),
        "repair must not recreate removed lowercase Memory markdown directory"
    );
    assert!(!actions.contains(&"Created default MEMORY.md".to_string()));
}

#[tokio::test]
async fn create_backup_does_not_copy_removed_memory_markdown() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("MEMORY.md"), "memory snapshot")
        .await
        .unwrap();

    let backup_dir = MemoryMaintenance::new(tmp.path())
        .create_backup()
        .await
        .unwrap();

    assert_eq!(backup_dir, tmp.path().join("backup"));
    let entries = std::fs::read_dir(&backup_dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    assert!(
        entries.is_empty(),
        "Removed Memory Markdown must not be backed up as Memory state"
    );
}

#[tokio::test]
async fn reindex_ignores_removed_memory_markdown_sources() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("MEMORY.md"), "# MEMORY.md\n")
        .await
        .unwrap();
    fs::create_dir(tmp.path().join("MEMORY")).await.unwrap();
    fs::write(tmp.path().join("MEMORY").join("2026-06-19.md"), "daily")
        .await
        .unwrap();

    #[cfg(unix)]
    let _ = std::os::unix::fs::symlink(tmp.path().join("MEMORY"), tmp.path().join("memory"));

    let report = MemoryMaintenance::new(tmp.path())
        .reindex(ReindexScope::Full)
        .await
        .unwrap();

    assert_eq!(report.documents_indexed, 0);
    assert!(report.errors.is_empty());
}
