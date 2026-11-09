use sqlite_vecstore::{Document, MetadataFilter, VecStore, VecStoreConfig, VecStoreError};

fn doc(
    id: &str,
    content: &str,
    metadata: serde_json::Value,
    embedding: Option<Vec<f32>>,
) -> Document {
    Document {
        id: id.to_string(),
        content: content.to_string(),
        metadata,
        embedding,
    }
}

#[test]
fn upsert_get_text_search_and_delete_keep_fts_in_sync() {
    let store = VecStore::open_in_memory(VecStoreConfig::default()).expect("store");

    store
        .upsert(&doc(
            "doc-1",
            "PostgreSQL migration checklist",
            serde_json::json!({"kind": "plan"}),
            None,
        ))
        .expect("insert doc-1");
    store
        .upsert(&doc(
            "doc-2",
            "React component refactor",
            serde_json::json!({"kind": "frontend"}),
            None,
        ))
        .expect("insert doc-2");

    assert_eq!(store.count().expect("count"), 2);
    assert_eq!(
        store
            .get("doc-1")
            .expect("get doc-1")
            .expect("doc-1 exists")
            .content,
        "PostgreSQL migration checklist"
    );

    let hits = store.text_search("PostgreSQL", 10).expect("text search");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].document.id, "doc-1");
    assert!(hits[0].score >= 0.0);

    store
        .upsert(&doc(
            "doc-1",
            "MySQL rollback checklist",
            serde_json::json!({"kind": "plan"}),
            None,
        ))
        .expect("replace doc-1");
    assert_eq!(store.count().expect("count after replace"), 2);
    assert!(store
        .text_search("PostgreSQL", 10)
        .expect("old fts entry removed")
        .is_empty());
    assert_eq!(
        store.text_search("MySQL", 10).expect("new fts entry").len(),
        1
    );

    assert!(store.delete("doc-1").expect("delete doc-1"));
    assert!(!store.delete("doc-1").expect("delete missing doc-1"));
    assert!(store.get("doc-1").expect("get deleted").is_none());
    assert!(store
        .text_search("MySQL", 10)
        .expect("fts deleted")
        .is_empty());
}

#[test]
fn embedding_identity_requires_matching_model_and_dimension_without_deleting_records() {
    let store = VecStore::open_in_memory(VecStoreConfig::default()).unwrap();
    store
        .upsert(&doc(
            "known",
            "original Memory",
            serde_json::json!({"embedding_model":"model-a"}),
            Some(vec![1.0, 0.0]),
        ))
        .unwrap();
    assert!(store.embeddings_match("model-a", 2).unwrap());
    assert!(!store.embeddings_match("model-b", 2).unwrap());
    assert!(!store.embeddings_match("model-a", 3).unwrap());
    store
        .upsert(&doc(
            "unknown",
            "unlabelled original",
            serde_json::json!({}),
            Some(vec![1.0, 0.0]),
        ))
        .unwrap();
    assert!(!store.embeddings_match("model-a", 2).unwrap());
    assert_eq!(store.count().unwrap(), 2);
    assert_eq!(
        store.get("known").unwrap().unwrap().content,
        "original Memory"
    );
}

#[test]
fn vector_search_orders_by_cosine_similarity_and_hides_embeddings() {
    let store = VecStore::open_in_memory(VecStoreConfig { dimension: Some(3) }).expect("store");

    store
        .upsert(&doc(
            "near",
            "database migration",
            serde_json::json!({"topic": "database"}),
            Some(vec![1.0, 0.0, 0.0]),
        ))
        .expect("near");
    store
        .upsert(&doc(
            "far",
            "frontend refactor",
            serde_json::json!({"topic": "frontend"}),
            Some(vec![0.0, 1.0, 0.0]),
        ))
        .expect("far");
    store
        .upsert(&doc(
            "middle",
            "database validation",
            serde_json::json!({"topic": "database"}),
            Some(vec![0.5, 0.5, 0.0]),
        ))
        .expect("middle");

    let hits = store
        .vector_search(&[1.0, 0.0, 0.0], 2)
        .expect("vector search");
    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0].document.id, "near");
    assert_eq!(hits[1].document.id, "middle");
    assert!(hits[0].score >= hits[1].score);
    assert!(
        hits.iter().all(|hit| hit.document.embedding.is_none()),
        "search results should not echo embedding vectors"
    );
}

#[test]
fn dimension_mismatch_is_rejected_on_insert_and_query() {
    let store = VecStore::open_in_memory(VecStoreConfig { dimension: Some(2) }).expect("store");

    let insert_err = store
        .upsert(&doc(
            "bad",
            "bad vector",
            serde_json::json!({}),
            Some(vec![1.0]),
        ))
        .expect_err("dimension mismatch on insert");
    assert!(matches!(
        insert_err,
        VecStoreError::DimensionMismatch {
            expected: 2,
            actual: 1
        }
    ));

    let query_err = store
        .vector_search(&[1.0, 0.0, 0.0], 10)
        .expect_err("dimension mismatch on query");
    assert!(matches!(
        query_err,
        VecStoreError::DimensionMismatch {
            expected: 2,
            actual: 3
        }
    ));
}

#[test]
fn metadata_filters_and_clear_define_reindex_foundation() {
    let store = VecStore::open_in_memory(VecStoreConfig::default()).expect("store");

    store
        .upsert(&doc(
            "a",
            "database migration runbook",
            serde_json::json!({"agent_id": "a1", "tags": ["db", "migration"]}),
            None,
        ))
        .expect("insert a");
    store
        .upsert(&doc(
            "b",
            "database migration notes",
            serde_json::json!({"agent_id": "a2", "tags": ["db"]}),
            None,
        ))
        .expect("insert b");

    let filtered = store
        .text_search_with_filter(
            "database",
            &MetadataFilter::and(vec![
                MetadataFilter::eq("agent_id", serde_json::json!("a1")),
                MetadataFilter::Contains {
                    field: "tags".into(),
                    value: "migration".into(),
                },
            ]),
            10,
        )
        .expect("filtered search");

    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].document.id, "a");

    assert!(store.health_check().expect("health"));
    store.clear().expect("clear");
    assert_eq!(store.count().expect("count after clear"), 0);
    assert!(store
        .text_search("database", 10)
        .expect("fts cleared")
        .is_empty());
}
