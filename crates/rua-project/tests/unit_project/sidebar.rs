use rua_project::{
    ProjectSidebarAction as Action, ProjectSidebarSort as Sort, ProjectSidebarStore,
};
fn ids() -> Vec<String> {
    ["a", "b", "c"].map(str::to_string).to_vec()
}
#[test]
fn manual_moves_change_one_entry_and_survive_automatic_sort_and_reload() {
    let temp = tempfile::tempdir().unwrap();
    let store = ProjectSidebarStore::new(temp.path());
    let (first, changes) = store
        .update(
            "empty",
            Action::Reorder {
                project_id: "b".into(),
                ordered_project_ids: vec!["b".into(), "a".into(), "c".into()],
            },
            &ids(),
        )
        .unwrap();
    assert_eq!(first.sort, Sort::Manual);
    assert_eq!(changes.len(), 4);
    let (second, changes) = store
        .update(
            &first.revision,
            Action::Reorder {
                project_id: "c".into(),
                ordered_project_ids: vec!["b".into(), "c".into(), "a".into()],
            },
            &ids(),
        )
        .unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].id, "project:c");
    assert!(
        second.items["b"].order < second.items["c"].order
            && second.items["c"].order < second.items["a"].order
    );
    let (automatic, _) = store
        .update(
            &second.revision,
            Action::SetSort { sort: Sort::Name },
            &ids(),
        )
        .unwrap();
    assert_eq!(automatic.items, second.items);
    let reopened = ProjectSidebarStore::new(temp.path()).snapshot().unwrap();
    assert_eq!(reopened.items, second.items);
    assert!(store
        .update(
            &first.revision,
            Action::SetSort {
                sort: Sort::CreatedAt
            },
            &ids()
        )
        .is_err());
}
#[test]
fn pinning_preserves_manual_position_and_invalid_moves_leave_state_untouched() {
    let temp = tempfile::tempdir().unwrap();
    let store = ProjectSidebarStore::new(temp.path());
    let (initial, _) = store
        .update(
            "empty",
            Action::Reorder {
                project_id: "a".into(),
                ordered_project_ids: ids(),
            },
            &ids(),
        )
        .unwrap();
    let (pinned, _) = store
        .update(
            &initial.revision,
            Action::SetPinned {
                project_id: "b".into(),
                pinned: true,
            },
            &ids(),
        )
        .unwrap();
    assert!(pinned.items["b"].pinned);
    assert_eq!(pinned.items["b"].order, initial.items["b"].order);
    let (unpinned, _) = store
        .update(
            &pinned.revision,
            Action::SetPinned {
                project_id: "b".into(),
                pinned: false,
            },
            &ids(),
        )
        .unwrap();
    assert!(!unpinned.items["b"].pinned);
    assert_eq!(unpinned.items["b"].order, initial.items["b"].order);
    assert!(store
        .update(
            &unpinned.revision,
            Action::Reorder {
                project_id: "a".into(),
                ordered_project_ids: vec!["a".into(), "a".into()]
            },
            &ids()
        )
        .is_err());
    assert_eq!(store.snapshot().unwrap().revision, unpinned.revision);
}

#[test]
fn sections_hide_and_delete_preserve_membership_without_resurrection() {
    let temp = tempfile::tempdir().unwrap();
    let store = ProjectSidebarStore::new(temp.path());
    let update = |action| {
        store
            .update(&store.snapshot().unwrap().revision, action, &ids())
            .unwrap()
    };
    let (created, _) = update(Action::CreateSection {
        name: "Work".into(),
    });
    let id = created.sections.keys().next().unwrap().clone();
    let record = store.records().unwrap().into_iter().next().unwrap();
    update(Action::MoveToSection {
        project_id: "a".into(),
        section_id: Some(id.clone()),
    });
    update(Action::SetPinned {
        project_id: "a".into(),
        pinned: true,
    });
    let (hidden, _) = update(Action::SetHidden {
        project_id: "a".into(),
        hidden: true,
    });
    assert_eq!(hidden.items["a"].section_id.as_deref(), Some(id.as_str()));
    assert!(hidden.items["a"].pinned && hidden.items["a"].hidden);
    let (deleted, changes) = update(Action::DeleteSection {
        section_id: id.clone(),
    });
    assert_eq!(changes.len(), 1);
    assert!(changes[0].is_tombstone());
    assert!(deleted.sections.is_empty());
    assert_eq!(deleted.items, hidden.items);
    store.apply_record(record).unwrap();
    assert!(store.snapshot().unwrap().sections.is_empty());
    assert!(store
        .update(
            &deleted.revision,
            Action::MoveToSection {
                project_id: "b".into(),
                section_id: Some(id)
            },
            &ids()
        )
        .is_err());
    let (restored, _) = update(Action::SetHidden {
        project_id: "a".into(),
        hidden: false,
    });
    assert!(!restored.items["a"].hidden);
    assert!(restored.items["a"].pinned);
}
