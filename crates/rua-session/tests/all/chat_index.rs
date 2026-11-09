use rua_session::{
    chat_index::{
        self, ChatConversationView, ChatIndexFilter, ChatIndexRecord, ChatSortField, SortOrder,
    },
    collaboration::{
        CollaborationConfigurationState, CollaborationDescriptor, CollaborationExecutor,
        COLLABORATION_META_KEY,
    },
    ChatTag,
};

fn view_record(
    id: &str,
    status: &str,
    collaboration: Option<CollaborationDescriptor>,
) -> ChatIndexRecord {
    let now = chrono::DateTime::parse_from_rfc3339("2026-09-21T00:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let mut meta = serde_json::Map::new();
    if let Some(collaboration) = collaboration {
        meta.insert(
            COLLABORATION_META_KEY.into(),
            serde_json::to_value(collaboration).unwrap(),
        );
    }
    ChatIndexRecord {
        id: id.into(),
        name: id.into(),
        user_id: "default".into(),
        channel: "console".into(),
        created_at: now,
        updated_at: now,
        meta: serde_json::Value::Object(meta),
        status: status.into(),
        pinned: false,
        pin_order: 0,
        tag: ChatTag::None,
        archived: false,
    }
}

#[test]
fn archive_filter_applies_before_count_and_pagination_and_restores() {
    let workspace = tempfile::TempDir::new().unwrap();
    for n in 0..4 {
        let mut record = view_record(&format!("chat-{n}"), "idle", None);
        record.archived = n % 2 == 0;
        chat_index::upsert_sync(workspace.path(), &record).unwrap();
    }
    let archived = chat_index::list_sync(
        workspace.path(),
        ChatIndexFilter {
            archived: Some(true),
            ..Default::default()
        },
        0,
        1,
    )
    .unwrap();
    assert_eq!(archived.total, 2);
    assert_eq!(archived.entries.len(), 1);
    assert!(archived.entries[0].archived);
    let mut restored = archived.entries[0].clone();
    restored.archived = false;
    chat_index::upsert_sync(workspace.path(), &restored).unwrap();
    let active = chat_index::list_sync(
        workspace.path(),
        ChatIndexFilter {
            archived: Some(false),
            ..Default::default()
        },
        0,
        20,
    )
    .unwrap();
    assert_eq!(active.total, 3);
    assert!(active.entries.iter().all(|chat| !chat.archived));
}

#[test]
fn conversation_views_and_collaboration_filters_apply_before_pagination() {
    let workspace = tempfile::tempdir().unwrap();
    for record in [
        view_record("user-a", "idle", None),
        view_record("user-b", "idle", None),
        view_record(
            "agent-child",
            "running",
            Some(CollaborationDescriptor {
                source: rua_core::ChatAddress::agent("default", "source-a"),
                executor: CollaborationExecutor::Agent,
                executor_agent_id: "reviewer".into(),
                authority_device_id: "device".into(),
                working_dir: workspace.path().to_path_buf(),
                creation_id: "create-agent".into(),
                configuration: CollaborationConfigurationState::ready("epoch-a"),
                extra: Default::default(),
            }),
        ),
        view_record(
            "acp-child",
            "idle",
            Some(CollaborationDescriptor {
                source: rua_core::ChatAddress::agent("default", "source-b"),
                executor: CollaborationExecutor::Acp {
                    runner: "mock_agent".into(),
                },
                executor_agent_id: "default".into(),
                authority_device_id: "device".into(),
                working_dir: workspace.path().to_path_buf(),
                creation_id: "create-acp".into(),
                configuration: CollaborationConfigurationState::ready("epoch-b"),
                extra: Default::default(),
            }),
        ),
    ] {
        chat_index::insert_sync(workspace.path(), &record).unwrap();
    }

    let users = chat_index::list_sync(
        workspace.path(),
        ChatIndexFilter {
            conversation_view: ChatConversationView::User,
            ..Default::default()
        },
        0,
        1,
    )
    .unwrap();
    assert_eq!(users.total, 2);
    assert_eq!(users.entries.len(), 1);
    let collaboration = chat_index::list_sync(
        workspace.path(),
        ChatIndexFilter {
            conversation_view: ChatConversationView::Collaboration,
            ..Default::default()
        },
        0,
        20,
    )
    .unwrap();
    assert_eq!(collaboration.total, 2);
    let source = chat_index::list_sync(
        workspace.path(),
        ChatIndexFilter {
            conversation_view: ChatConversationView::Collaboration,
            source_chat_id: Some("source-a".into()),
            executor: Some("reviewer".into()),
            status: Some("running".into()),
            ..Default::default()
        },
        0,
        20,
    )
    .unwrap();
    assert_eq!(source.entries[0].id, "agent-child");
    let acp = chat_index::list_sync(
        workspace.path(),
        ChatIndexFilter {
            conversation_view: ChatConversationView::Collaboration,
            executor: Some("mock_agent".into()),
            ..Default::default()
        },
        0,
        20,
    )
    .unwrap();
    assert_eq!(acp.entries[0].id, "acp-child");
}

#[test]
fn opening_legacy_index_drops_the_duplicate_session_identity() {
    let workspace = tempfile::tempdir().unwrap();
    let path = workspace.path().join("chat-index.sqlite");
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute_batch(
            r#"
            CREATE TABLE chat_index (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                name TEXT NOT NULL,
                user_id TEXT NOT NULL,
                channel TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                meta_json TEXT NOT NULL,
                status TEXT NOT NULL,
                pinned INTEGER NOT NULL DEFAULT 0,
                pin_order INTEGER NOT NULL DEFAULT 0
            );
            INSERT INTO chat_index (
                id, session_id, name, user_id, channel, created_at, updated_at,
                meta_json, status, pinned, pin_order
            ) VALUES (
                'chat-1', 'legacy-other-id', 'Chat', 'default', 'console',
                '2026-08-01T00:00:00Z', '2026-08-01T00:00:00Z',
                '{}', 'idle', 0, 0
            );
            "#,
        )
        .unwrap();
    drop(connection);

    let record = chat_index::get_sync(workspace.path(), "chat-1")
        .unwrap()
        .unwrap();

    assert_eq!(record.id, "chat-1");
    let connection = rusqlite::Connection::open(&path).unwrap();
    let columns = connection
        .prepare("PRAGMA table_info(chat_index)")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert!(!columns.iter().any(|column| column == "session_id"));
    assert!(columns.iter().any(|column| column == "tag"));
    assert_eq!(record.tag, ChatTag::None);
}

#[test]
fn opening_index_without_tag_adds_defaulted_tag_column() {
    let workspace = tempfile::tempdir().unwrap();
    let path = workspace.path().join("chat-index.sqlite");
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute_batch(
            r#"
            CREATE TABLE chat_index (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                user_id TEXT NOT NULL,
                channel TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                meta_json TEXT NOT NULL,
                status TEXT NOT NULL,
                pinned INTEGER NOT NULL DEFAULT 0,
                pin_order INTEGER NOT NULL DEFAULT 0
            );
            INSERT INTO chat_index (
                id, name, user_id, channel, created_at, updated_at,
                meta_json, status, pinned, pin_order
            ) VALUES (
                'chat-1', 'Chat', 'default', 'console',
                '2026-08-01T00:00:00Z', '2026-08-01T00:00:00Z',
                '{}', 'idle', 0, 0
            );
            "#,
        )
        .unwrap();
    drop(connection);

    let record = chat_index::get_sync(workspace.path(), "chat-1")
        .unwrap()
        .unwrap();
    assert_eq!(record.tag, ChatTag::None);
}

#[tokio::test]
async fn chat_index_round_trip_uses_chat_id_as_its_only_identity() {
    let workspace = tempfile::tempdir().unwrap();
    let now = chrono::Utc::now();
    chat_index::upsert(
        workspace.path(),
        ChatIndexRecord {
            id: "chat-1".to_string(),
            name: "Chat".to_string(),
            user_id: "default".to_string(),
            channel: "console".to_string(),
            created_at: now,
            updated_at: now,
            meta: serde_json::json!({}),
            status: "idle".to_string(),
            pinned: false,
            pin_order: 0,
            tag: ChatTag::Red,
            archived: false,
        },
    )
    .await
    .unwrap();

    assert_eq!(
        chat_index::get(workspace.path(), "chat-1")
            .await
            .unwrap()
            .unwrap()
            .tag,
        ChatTag::Red
    );
}

#[test]
fn pinned_chats_are_included_on_every_unpinned_page() {
    let workspace = tempfile::tempdir().unwrap();
    let now = chrono::Utc::now();
    for (id, minutes_ago, pinned) in [
        ("chat-old", 3, false),
        ("chat-new", 1, false),
        ("chat-pinned", 0, true),
    ] {
        chat_index::upsert_sync(
            workspace.path(),
            &ChatIndexRecord {
                id: id.to_string(),
                name: id.to_string(),
                user_id: "default".to_string(),
                channel: "console".to_string(),
                created_at: now - chrono::Duration::minutes(minutes_ago),
                updated_at: now - chrono::Duration::minutes(minutes_ago),
                meta: serde_json::json!({}),
                status: "idle".to_string(),
                pinned,
                pin_order: if pinned { 1 } else { 0 },
                tag: ChatTag::None,
                archived: false,
            },
        )
        .unwrap();
    }

    let first = chat_index::list_sync(workspace.path(), Default::default(), 0, 1).unwrap();
    assert_eq!(
        first
            .entries
            .iter()
            .map(|entry| entry.id.as_str())
            .collect::<Vec<_>>(),
        vec!["chat-pinned", "chat-new"]
    );

    let second = chat_index::list_sync(workspace.path(), Default::default(), 1, 1).unwrap();
    assert_eq!(
        second
            .entries
            .iter()
            .map(|entry| entry.id.as_str())
            .collect::<Vec<_>>(),
        vec!["chat-pinned", "chat-old"]
    );
}

#[test]
fn list_filters_by_tag_including_untagged_chats() {
    let workspace = tempfile::tempdir().unwrap();
    let now = chrono::Utc::now();
    for (id, tag) in [
        ("chat-none", ChatTag::None),
        ("chat-red", ChatTag::Red),
        ("chat-yellow", ChatTag::Yellow),
    ] {
        chat_index::upsert_sync(
            workspace.path(),
            &ChatIndexRecord {
                id: id.to_string(),
                name: id.to_string(),
                user_id: "default".to_string(),
                channel: "console".to_string(),
                created_at: now,
                updated_at: now,
                meta: serde_json::json!({}),
                status: "idle".to_string(),
                pinned: false,
                pin_order: 0,
                tag,
                archived: false,
            },
        )
        .unwrap();
    }

    let red = chat_index::list_sync(
        workspace.path(),
        ChatIndexFilter {
            tag: Some(ChatTag::Red),
            ..Default::default()
        },
        0,
        20,
    )
    .unwrap();
    assert_eq!(red.total, 1);
    assert_eq!(red.entries[0].id, "chat-red");

    let untagged = chat_index::list_sync(
        workspace.path(),
        ChatIndexFilter {
            tag: Some(ChatTag::None),
            ..Default::default()
        },
        0,
        20,
    )
    .unwrap();
    assert_eq!(untagged.total, 1);
    assert_eq!(untagged.entries[0].id, "chat-none");
}

#[test]
fn list_supports_all_sort_fields_and_directions_without_breaking_pin_order() {
    let workspace = tempfile::tempdir().unwrap();
    let base = chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    for (id, name, created_minutes, updated_minutes, pinned, pin_order) in [
        ("alpha", "Alpha", 2, 3, false, 0),
        ("beta", "beta", 1, 2, false, 0),
        ("gamma", "Gamma", 3, 1, false, 0),
        ("pinned-later", "Zulu", 4, 4, true, 2),
        ("pinned-first", "Omega", 5, 5, true, 1),
    ] {
        chat_index::upsert_sync(
            workspace.path(),
            &ChatIndexRecord {
                id: id.to_string(),
                name: name.to_string(),
                user_id: "default".to_string(),
                channel: "console".to_string(),
                created_at: base + chrono::Duration::minutes(created_minutes),
                updated_at: base + chrono::Duration::minutes(updated_minutes),
                meta: serde_json::json!({}),
                status: "idle".to_string(),
                pinned,
                pin_order,
                tag: ChatTag::None,
                archived: false,
            },
        )
        .unwrap();
    }

    for (sort_by, sort_order, expected_unpinned) in [
        (
            ChatSortField::Title,
            SortOrder::Asc,
            vec!["alpha", "beta", "gamma"],
        ),
        (
            ChatSortField::Title,
            SortOrder::Desc,
            vec!["gamma", "beta", "alpha"],
        ),
        (
            ChatSortField::CreatedAt,
            SortOrder::Asc,
            vec!["beta", "alpha", "gamma"],
        ),
        (
            ChatSortField::CreatedAt,
            SortOrder::Desc,
            vec!["gamma", "alpha", "beta"],
        ),
        (
            ChatSortField::UpdatedAt,
            SortOrder::Asc,
            vec!["gamma", "beta", "alpha"],
        ),
        (
            ChatSortField::UpdatedAt,
            SortOrder::Desc,
            vec!["alpha", "beta", "gamma"],
        ),
    ] {
        let page = chat_index::list_sync(
            workspace.path(),
            ChatIndexFilter {
                sort_by,
                sort_order,
                ..Default::default()
            },
            0,
            10,
        )
        .unwrap();
        let ids = page
            .entries
            .iter()
            .map(|entry| entry.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(&ids[..2], &["pinned-first", "pinned-later"]);
        assert_eq!(&ids[2..], expected_unpinned);
    }
}

#[test]
fn opening_index_without_archived_adds_defaulted_archived_column() {
    let workspace = tempfile::tempdir().unwrap();
    let path = workspace.path().join("chat-index.sqlite");
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute_batch(
            r#"
            CREATE TABLE chat_index (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                user_id TEXT NOT NULL,
                channel TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                meta_json TEXT NOT NULL,
                status TEXT NOT NULL,
                pinned INTEGER NOT NULL DEFAULT 0,
                pin_order INTEGER NOT NULL DEFAULT 0,
                tag TEXT NOT NULL DEFAULT 'none'
            );
            INSERT INTO chat_index (
                id, name, user_id, channel, created_at, updated_at,
                meta_json, status, pinned, pin_order, tag
            ) VALUES (
                'chat-1', 'Chat', 'default', 'console',
                '2026-08-01T00:00:00Z', '2026-08-01T00:00:00Z',
                '{}', 'idle', 0, 0, 'none'
            );
            "#,
        )
        .unwrap();
    drop(connection);

    let record = chat_index::get_sync(workspace.path(), "chat-1")
        .unwrap()
        .unwrap();
    assert!(!record.archived);
}

#[test]
fn archived_filter_separates_archived_from_active_chats() {
    let workspace = tempfile::tempdir().unwrap();
    let now = chrono::Utc::now();
    for (id, archived) in [
        ("active-a", false),
        ("active-b", false),
        ("archived-a", true),
    ] {
        chat_index::upsert_sync(
            workspace.path(),
            &ChatIndexRecord {
                id: id.to_string(),
                name: id.to_string(),
                user_id: "default".to_string(),
                channel: "console".to_string(),
                created_at: now,
                updated_at: now,
                meta: serde_json::json!({}),
                status: "idle".to_string(),
                pinned: false,
                pin_order: 0,
                tag: ChatTag::None,
                archived,
            },
        )
        .unwrap();
    }

    let all = chat_index::list_sync(
        workspace.path(),
        ChatIndexFilter {
            ..Default::default()
        },
        0,
        20,
    )
    .unwrap();
    assert_eq!(all.total, 3);

    let active_only = chat_index::list_sync(
        workspace.path(),
        ChatIndexFilter {
            archived: Some(false),
            ..Default::default()
        },
        0,
        20,
    )
    .unwrap();
    assert_eq!(active_only.total, 2);
    assert!(active_only.entries.iter().all(|entry| !entry.archived));

    let archived_only = chat_index::list_sync(
        workspace.path(),
        ChatIndexFilter {
            archived: Some(true),
            ..Default::default()
        },
        0,
        20,
    )
    .unwrap();
    assert_eq!(archived_only.total, 1);
    assert_eq!(archived_only.entries[0].id, "archived-a");
    assert!(archived_only.entries[0].archived);
}

#[test]
fn upsert_persists_and_round_trips_archived_flag() {
    let workspace = tempfile::tempdir().unwrap();
    let now = chrono::Utc::now();
    chat_index::upsert_sync(
        workspace.path(),
        &ChatIndexRecord {
            id: "chat-archive".to_string(),
            name: "Archived Chat".to_string(),
            user_id: "default".to_string(),
            channel: "console".to_string(),
            created_at: now,
            updated_at: now,
            meta: serde_json::json!({}),
            status: "idle".to_string(),
            pinned: false,
            pin_order: 0,
            tag: ChatTag::None,
            archived: true,
        },
    )
    .unwrap();

    let record = chat_index::get_sync(workspace.path(), "chat-archive")
        .unwrap()
        .unwrap();
    assert!(record.archived);

    chat_index::upsert_sync(
        workspace.path(),
        &ChatIndexRecord {
            id: "chat-archive".to_string(),
            name: "Archived Chat".to_string(),
            user_id: "default".to_string(),
            channel: "console".to_string(),
            created_at: now,
            updated_at: now,
            meta: serde_json::json!({}),
            status: "idle".to_string(),
            pinned: false,
            pin_order: 0,
            tag: ChatTag::None,
            archived: false,
        },
    )
    .unwrap();
    let record = chat_index::get_sync(workspace.path(), "chat-archive")
        .unwrap()
        .unwrap();
    assert!(!record.archived);
}

#[test]
fn latest_activity_uses_all_index_rows_without_materializing_empty_workspaces() {
    let workspace = tempfile::tempdir().unwrap();
    let absent = workspace.path().join("unmaterialized");
    assert_eq!(chat_index::latest_activity_sync(&absent).unwrap(), None);
    assert!(!absent.exists());

    let mut older = view_record("pinned", "idle", None);
    older.pinned = true;
    chat_index::insert_sync(workspace.path(), &older).unwrap();
    let mut latest = view_record("archived-collaboration", "idle", None);
    latest.updated_at += chrono::Duration::hours(2);
    latest.archived = true;
    latest.meta = serde_json::json!({"collaboration": {"source": {"chat_id": "pinned"}}});
    chat_index::insert_sync(workspace.path(), &latest).unwrap();
    assert_eq!(
        chat_index::latest_activity_sync(workspace.path()).unwrap(),
        Some(latest.updated_at),
    );
    chat_index::delete_sync(workspace.path(), &latest.id).unwrap();
    assert_eq!(
        chat_index::latest_activity_sync(workspace.path()).unwrap(),
        Some(older.updated_at),
    );
    chat_index::delete_sync(workspace.path(), &older.id).unwrap();
    assert_eq!(
        chat_index::latest_activity_sync(workspace.path()).unwrap(),
        None
    );
}
