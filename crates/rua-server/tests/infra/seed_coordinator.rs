use std::path::Path;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use rua_server::seed::{SeedCoordinator, SeedProducer, SeedRunStatus};
use tempfile::TempDir;

#[test]
fn coordinator_materializes_producer_and_records_seed_manifest() {
    let tmp = TempDir::new().unwrap();
    let producer = FakeProducer::new(
        "agent:rua_qa",
        "agent_seed_catalog",
        vec![("workspaces/rua_qa/agent.json", "seeded")],
    );
    let calls = Arc::clone(&producer.calls);

    let report = SeedCoordinator::new(tmp.path())
        .with_producer(Box::new(producer))
        .run();

    assert_eq!(report.outcomes[0].status, SeedRunStatus::Materialized);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("workspaces/rua_qa/agent.json")).unwrap(),
        "seeded"
    );
    assert!(rua_core::seed_manifest::is_entry_materialized(tmp.path(), "agent:rua_qa").unwrap());
    assert!(rua_core::seed_manifest::is_file_untouched_seed(
        tmp.path(),
        "workspaces/rua_qa/agent.json"
    )
    .unwrap());
}

#[test]
fn coordinator_skips_entry_already_materialized_even_if_file_was_deleted() {
    let tmp = TempDir::new().unwrap();
    write_file(tmp.path(), "workspaces/default/agent.json", "original");
    rua_core::seed_manifest::record_seed_entry(
        tmp.path(),
        "agent:default",
        "agent_seed_catalog",
        &["workspaces/default/agent.json"],
    )
    .unwrap();
    std::fs::remove_file(tmp.path().join("workspaces/default/agent.json")).unwrap();

    let producer = FakeProducer::new(
        "agent:default",
        "agent_seed_catalog",
        vec![("workspaces/default/agent.json", "recreated")],
    );
    let calls = Arc::clone(&producer.calls);

    let report = SeedCoordinator::new(tmp.path())
        .with_producer(Box::new(producer))
        .run();

    assert_eq!(
        report.outcomes[0].status,
        SeedRunStatus::AlreadyMaterialized
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(!tmp.path().join("workspaces/default/agent.json").exists());
}

#[test]
fn coordinator_does_not_claim_occupied_paths_without_manifest_entry() {
    let tmp = TempDir::new().unwrap();
    write_file(tmp.path(), "workspaces/rua_qa/agent.json", "user-owned");

    let producer = FakeProducer::new(
        "agent:rua_qa",
        "agent_seed_catalog",
        vec![("workspaces/rua_qa/agent.json", "seeded")],
    );
    let calls = Arc::clone(&producer.calls);

    let report = SeedCoordinator::new(tmp.path())
        .with_producer(Box::new(producer))
        .run();

    assert_eq!(report.outcomes[0].status, SeedRunStatus::TargetOccupied);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("workspaces/rua_qa/agent.json")).unwrap(),
        "user-owned"
    );
    assert!(!rua_core::seed_manifest::is_entry_materialized(tmp.path(), "agent:rua_qa").unwrap());
}

#[test]
fn coordinator_continues_after_a_failed_producer() {
    let tmp = TempDir::new().unwrap();
    let ok = FakeProducer::new(
        "prompt_library:builtin",
        "builtin_prompt_library",
        vec![("prompt_library/SOUL.md", "soul")],
    );

    let report = SeedCoordinator::new(tmp.path())
        .with_producer(Box::new(FailingProducer))
        .with_producer(Box::new(ok))
        .run();

    assert_eq!(report.outcomes[0].status, SeedRunStatus::Failed);
    assert_eq!(report.outcomes[1].status, SeedRunStatus::Materialized);
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("prompt_library/SOUL.md")).unwrap(),
        "soul"
    );
}

struct FakeProducer {
    entry_id: &'static str,
    producer: &'static str,
    files: Vec<(&'static str, &'static str)>,
    calls: Arc<AtomicUsize>,
}

impl FakeProducer {
    fn new(
        entry_id: &'static str,
        producer: &'static str,
        files: Vec<(&'static str, &'static str)>,
    ) -> Self {
        Self {
            entry_id,
            producer,
            files,
            calls: Arc::new(AtomicUsize::new(0)),
        }
    }
}

impl SeedProducer for FakeProducer {
    fn entry_id(&self) -> String {
        self.entry_id.to_string()
    }

    fn producer_name(&self) -> &str {
        self.producer
    }

    fn target_paths(&self) -> Vec<String> {
        self.files
            .iter()
            .map(|(path, _)| (*path).to_string())
            .collect()
    }

    fn materialize(&self, data_dir: &Path) -> rua_core::Result<Vec<String>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        for (path, content) in &self.files {
            write_file(data_dir, path, content);
        }
        Ok(self.target_paths())
    }
}

struct FailingProducer;

impl SeedProducer for FailingProducer {
    fn entry_id(&self) -> String {
        "agent:broken".to_string()
    }

    fn producer_name(&self) -> &str {
        "test"
    }

    fn target_paths(&self) -> Vec<String> {
        vec!["workspaces/broken/agent.json".to_string()]
    }

    fn materialize(&self, _data_dir: &Path) -> rua_core::Result<Vec<String>> {
        Err(rua_core::RuaError::Validation {
            message: "boom".to_string(),
        })
    }
}

fn write_file(data_dir: &Path, rel: &str, content: &str) {
    let path = data_dir.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}
