use rua_project::{
    ProjectSidebarAction, ProjectSidebarItem, ProjectSidebarRecord, ProjectSidebarStore,
    ProjectSidebarValue,
};
use rua_sync::hlc::HlcTimestamp;
use rua_sync::merge_store::MergeStore;
use rua_sync::project_sidebar_adapter::ProjectSidebarSyncAdapter;
use rua_sync::sync_registry::SyncRegistry;
use std::sync::Arc;

fn record(id: &str, version: &str, pinned: bool) -> ProjectSidebarRecord {
    ProjectSidebarRecord {
        id: format!("project:{id}"),
        version: version.into(),
        updated_at_ms: 1,
        value: ProjectSidebarValue::Project {
            item: ProjectSidebarItem {
                project_id: id.into(),
                pinned,
                ..Default::default()
            },
        },
        extra: Default::default(),
    }
}
#[tokio::test]
async fn independent_records_merge_and_concurrent_moves_converge_in_either_delivery_order() {
    let a = ProjectSidebarSyncAdapter::record_change(
        record("a", "a1", true),
        "one",
        HlcTimestamp {
            wall_ms: 1,
            counter: 0,
            device_id: [1; 8],
        },
    );
    let b = ProjectSidebarSyncAdapter::record_change(
        record("b", "b1", true),
        "two",
        HlcTimestamp {
            wall_ms: 1,
            counter: 0,
            device_id: [2; 8],
        },
    );
    let newer = ProjectSidebarSyncAdapter::record_change(
        record("a", "a2", false),
        "two",
        HlcTimestamp {
            wall_ms: 1,
            counter: 0,
            device_id: [3; 8],
        },
    );
    let mut results = Vec::new();
    for changes in [[&a, &b, &newer], [&newer, &b, &a]] {
        let temp = tempfile::tempdir().unwrap();
        let mut registry = SyncRegistry::new();
        registry
            .register(Arc::new(ProjectSidebarSyncAdapter::new(
                temp.path().to_path_buf(),
                "reader",
            )))
            .unwrap();
        let merge = MergeStore::open(&temp.path().join("merge")).unwrap();
        let store = ProjectSidebarStore::new(temp.path());
        let stale = store.snapshot().unwrap().revision;
        for change in changes {
            registry.apply_change(change, &merge).await.unwrap();
        }
        let snapshot = store.snapshot().unwrap();
        assert!(!snapshot.items["a"].pinned);
        assert!(snapshot.items["b"].pinned);
        assert!(store
            .update(
                &stale,
                ProjectSidebarAction::SetPinned {
                    project_id: "a".into(),
                    pinned: true
                },
                &["a".into(), "b".into()]
            )
            .is_err());
        results.push(snapshot.items);
    }
    assert_eq!(results[0], results[1]);
}

#[tokio::test]
async fn section_delete_wins_over_newer_rename_in_both_delivery_orders() {
    let section = |deleted, version| ProjectSidebarRecord {
        id: "section:s".into(),
        version: format!("v{version}"),
        updated_at_ms: version,
        value: ProjectSidebarValue::Section {
            section: rua_project::ProjectSidebarSection {
                section_id: "s".into(),
                name: "Work".into(),
                order: 0,
                deleted,
                extra: Default::default(),
            },
        },
        extra: Default::default(),
    };
    let deleted = ProjectSidebarSyncAdapter::record_change(
        section(true, 1),
        "one",
        HlcTimestamp {
            wall_ms: 1,
            counter: 0,
            device_id: [1; 8],
        },
    );
    let renamed = ProjectSidebarSyncAdapter::record_change(
        section(false, 2),
        "two",
        HlcTimestamp {
            wall_ms: 2,
            counter: 0,
            device_id: [2; 8],
        },
    );
    for changes in [[&deleted, &renamed], [&renamed, &deleted]] {
        let temp = tempfile::tempdir().unwrap();
        let mut registry = SyncRegistry::new();
        registry
            .register(Arc::new(ProjectSidebarSyncAdapter::new(
                temp.path().into(),
                "reader",
            )))
            .unwrap();
        let merge = MergeStore::open(&temp.path().join("merge")).unwrap();
        for change in changes {
            registry.apply_change(change, &merge).await.unwrap();
        }
        let store = ProjectSidebarStore::new(temp.path());
        assert!(store.snapshot().unwrap().sections.is_empty());
        assert!(store.records().unwrap()[0].is_tombstone());
    }
}
