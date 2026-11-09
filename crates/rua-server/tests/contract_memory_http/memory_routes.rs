use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use axum::http::{Method, StatusCode};
use rua_agent::memory_manager::{
    DreamMemoryRequest, DreamMemoryResult, LoadContextRequest, MemoryContext, MemoryGetRequest,
    MemoryManager, MemoryRecord, MemoryRecordStatus, MemorySearchHit, MemorySearchRequest,
    MemoryStatus, MemoryUpdateRequest, MemoryWriteOutcome, MemoryWriteRequest,
};
use rua_agent::AgentError;
use rua_memory::MemoryId;
use rua_memory::{
    MemoryTerm, MemoryTermKind, MemoryTermSource, MemoryVisibility, MemoryVisibilityGrant,
    MemoryVisibilityMode, MemoryVisibilityUpdateRequest,
};
use serde_json::json;
use tokio::sync::Mutex;

use super::support::{build_app, create_agent, read_json, request};

struct StaticMemoryManager {
    agent_id: String,
    document_count: usize,
    records: Mutex<Vec<MemoryRecord>>,
    visibility: Mutex<HashMap<String, MemoryVisibility>>,
    deleted: Mutex<Vec<String>>,
}

impl StaticMemoryManager {
    fn new(agent_id: &str, document_count: usize) -> Arc<Self> {
        Arc::new(Self {
            agent_id: agent_id.to_string(),
            document_count,
            records: Mutex::new(Vec::new()),
            visibility: Mutex::new(HashMap::new()),
            deleted: Mutex::new(Vec::new()),
        })
    }
}

#[async_trait]
impl MemoryManager for StaticMemoryManager {
    async fn dream_memory(
        &self,
        _req: DreamMemoryRequest,
    ) -> Result<DreamMemoryResult, AgentError> {
        Ok(DreamMemoryResult {
            items_consolidated: 0,
            new_insights: Vec::new(),
        })
    }

    async fn memory_search(
        &self,
        req: MemorySearchRequest,
    ) -> Result<Vec<MemorySearchHit>, AgentError> {
        Ok(vec![MemorySearchHit {
            content: format!("{} handled {}", self.agent_id, req.query),
            score: 1.0,
            source: "contract-memory".to_string(),
            metadata: json!({
                "manager_agent_id": self.agent_id,
                "request_agent_id": req.agent_id,
                "top_k": req.top_k
            }),
        }])
    }
    async fn memory_write(
        &self,
        req: MemoryWriteRequest,
    ) -> Result<MemoryWriteOutcome, AgentError> {
        let now = chrono::Utc::now();
        let id = MemoryId::new("mem_http_contract");
        let record = MemoryRecord {
            title: req.title,
            id: id.clone(),
            agent_id: req.agent_id,
            scope: req.scope,
            kind: req.kind,
            content: req.content,
            status: MemoryRecordStatus::Active,
            confidence: req.confidence.unwrap_or(1.0),
            importance: req.importance,
            created_at: now,
            updated_at: Some(now),
            last_used_at: None,
            expires_at: None,
            superseded_by: None,
            written_on_device_id: req
                .metadata
                .get("written_on_device_id")
                .and_then(|value| value.as_str())
                .map(ToOwned::to_owned),
            written_on_device_name_snapshot: req
                .metadata
                .get("written_on_device_name_snapshot")
                .and_then(|value| value.as_str())
                .map(ToOwned::to_owned),
            written_on_host_profile: req
                .metadata
                .get("written_on_host_profile")
                .and_then(|value| value.as_str())
                .map(ToOwned::to_owned),
            resource_device_id: req
                .metadata
                .get("resource_device_id")
                .and_then(|value| value.as_str())
                .map(ToOwned::to_owned),
            resource_device_name_snapshot: req
                .metadata
                .get("resource_device_name_snapshot")
                .and_then(|value| value.as_str())
                .map(ToOwned::to_owned),
            metadata: req.metadata,
            sources: req.evidence.into_iter().collect(),
            links: vec![],
            terms: req
                .labels
                .iter()
                .map(|label| MemoryTerm {
                    term: label.clone(),
                    normalized_term: label.to_lowercase(),
                    term_kind: MemoryTermKind::Label,
                    source: MemoryTermSource::Agent,
                    language: None,
                    created_at: now,
                })
                .chain(req.search_terms.iter().map(|term| MemoryTerm {
                    term: term.clone(),
                    normalized_term: term.to_lowercase(),
                    term_kind: MemoryTermKind::SearchTerm,
                    source: MemoryTermSource::Agent,
                    language: None,
                    created_at: now,
                }))
                .collect(),
            resources: vec![],
            relations: vec![],
        };
        self.records.lock().await.push(record.clone());
        self.visibility.lock().await.insert(
            id.to_string(),
            MemoryVisibility {
                memory_id: id.clone(),
                owner_agent_id: record.agent_id.clone(),
                mode: MemoryVisibilityMode::Private,
                grantee_agent_ids: vec![],
                grants: vec![],
            },
        );
        Ok(MemoryWriteOutcome {
            rejected: false,
            reason: None,
            record: Some(record),
        })
    }

    async fn memory_get(&self, req: MemoryGetRequest) -> Result<Vec<MemoryRecord>, AgentError> {
        let records = self.records.lock().await;
        Ok(records
            .iter()
            .filter(|record| req.ids.iter().any(|id| id == &record.id))
            .cloned()
            .collect())
    }

    async fn memory_list(&self, agent_id: &str) -> Result<Vec<MemoryRecord>, AgentError> {
        let records = self.records.lock().await;
        Ok(records
            .iter()
            .filter(|record| record.agent_id == agent_id)
            .cloned()
            .collect())
    }

    async fn memory_update(
        &self,
        req: MemoryUpdateRequest,
    ) -> Result<Option<MemoryRecord>, AgentError> {
        let mut records = self.records.lock().await;
        let Some(record) = records
            .iter_mut()
            .find(|record| record.id == req.id && record.agent_id == req.agent_id)
        else {
            return Ok(None);
        };
        record.kind = req.kind;
        record.scope = req.scope;
        record.title = req.title;
        record.content = req.content;
        record.confidence = req.confidence;
        record.importance = req.importance;
        record.updated_at = Some(chrono::Utc::now());
        Ok(Some(record.clone()))
    }

    async fn memory_visibility(
        &self,
        owner_agent_id: &str,
        memory_id: &MemoryId,
    ) -> Result<Option<MemoryVisibility>, AgentError> {
        Ok(self
            .visibility
            .lock()
            .await
            .get(&memory_id.to_string())
            .filter(|visibility| visibility.owner_agent_id == owner_agent_id)
            .cloned())
    }

    async fn memory_visibility_list(
        &self,
        owner_agent_id: &str,
    ) -> Result<Vec<MemoryVisibility>, AgentError> {
        Ok(self
            .visibility
            .lock()
            .await
            .values()
            .filter(|visibility| visibility.owner_agent_id == owner_agent_id)
            .cloned()
            .collect())
    }

    async fn memory_visibility_update(
        &self,
        owner_agent_id: &str,
        memory_id: MemoryId,
        req: MemoryVisibilityUpdateRequest,
    ) -> Result<Option<MemoryVisibility>, AgentError> {
        let records = self.records.lock().await;
        if !records
            .iter()
            .any(|record| record.id == memory_id && record.agent_id == owner_agent_id)
        {
            return Ok(None);
        }
        drop(records);

        let grantee_agent_ids = match req.mode {
            MemoryVisibilityMode::Private => vec![],
            MemoryVisibilityMode::AllAgents => vec!["*".to_string()],
            MemoryVisibilityMode::Selected => req.grantee_agent_ids,
        };
        let now = chrono::Utc::now();
        let grants = grantee_agent_ids
            .iter()
            .map(|grantee_agent_id| MemoryVisibilityGrant {
                memory_id: memory_id.clone(),
                owner_agent_id: owner_agent_id.to_string(),
                grantee_agent_id: grantee_agent_id.clone(),
                permission: "read".to_string(),
                created_by: "user".to_string(),
                created_at: now,
            })
            .collect::<Vec<_>>();
        let visibility = MemoryVisibility {
            memory_id: memory_id.clone(),
            owner_agent_id: owner_agent_id.to_string(),
            mode: req.mode,
            grantee_agent_ids,
            grants,
        };
        self.visibility
            .lock()
            .await
            .insert(memory_id.to_string(), visibility.clone());
        Ok(Some(visibility))
    }

    async fn load_context(&self, _req: LoadContextRequest) -> Result<MemoryContext, AgentError> {
        Ok(MemoryContext {
            recent_items: Vec::new(),
        })
    }

    async fn delete_memory(&self, memory_id: &str) -> Result<bool, AgentError> {
        self.deleted.lock().await.push(memory_id.to_string());
        Ok(true)
    }

    async fn health_check(&self) -> Result<MemoryStatus, AgentError> {
        Ok(MemoryStatus {
            healthy: true,
            document_count: self.document_count,
            index_size_bytes: self.document_count as u64 * 10,
            issues: vec![format!("{} health marker", self.agent_id)],
        })
    }

    async fn reindex(&self, _workspace_dir: &Path) -> Result<usize, AgentError> {
        Ok(7)
    }
}

async fn install_manager(app: &super::support::TestApp, agent_id: &str, document_count: usize) {
    let runtime = app
        .state
        .registry
        .get_or_create(agent_id)
        .await
        .expect("workspace runtime");
    runtime
        .set_memory_manager(StaticMemoryManager::new(agent_id, document_count))
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn root_memory_routes_are_removed() {
    let app = build_app();

    for (method, uri, body) in [
        (Method::GET, "/api/memory/health", None),
        (
            Method::POST,
            "/api/memory/search",
            Some(json!({"query": "x"})),
        ),
        (Method::GET, "/api/memory/diagnose", None),
        (Method::POST, "/api/memory/repair", None),
    ] {
        let resp = request(&app.app, method, uri, body, Some("default")).await;
        assert_eq!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "root-scoped Memory route must be removed: {uri}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn path_scoped_memory_health_uses_path_agent_over_conflicting_header() {
    let app = build_app();
    let header_agent = "memory_health_header_conflict";
    let path_agent = "memory_health_path_wins";
    create_agent(&app.app, header_agent, "Memory health header conflict").await;
    create_agent(&app.app, path_agent, "Memory health path wins").await;
    install_manager(&app, header_agent, 11).await;
    install_manager(&app, path_agent, 33).await;

    let resp = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{path_agent}/memory/health"),
        None,
        Some(header_agent),
    )
    .await;

    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["document_count"], 33);
    assert_eq!(body["issues"][0], "memory_health_path_wins health marker");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn path_scoped_memory_search_uses_path_agent_over_body_agent_id() {
    let app = build_app();
    let body_agent = "memory_search_body_agent";
    let path_agent = "memory_search_path_agent";
    create_agent(&app.app, body_agent, "Memory search body agent").await;
    create_agent(&app.app, path_agent, "Memory search path agent").await;
    install_manager(&app, body_agent, 11).await;
    install_manager(&app, path_agent, 33).await;

    let resp = request(
        &app.app,
        Method::POST,
        &format!("/api/agents/{path_agent}/memory/search"),
        Some(json!({
            "query": "scope check",
            "agent_id": body_agent,
            "top_k": 3
        })),
        None,
    )
    .await;

    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    let hit = &body["results"][0];
    assert_eq!(
        hit["content"],
        "memory_search_path_agent handled scope check"
    );
    assert_eq!(hit["metadata"]["manager_agent_id"], path_agent);
    assert_eq!(hit["metadata"]["request_agent_id"], path_agent);
    assert_eq!(hit["metadata"]["top_k"], 3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn path_scoped_memory_write_get_delete_and_reindex_use_path_agent() {
    let app = build_app();
    let path_agent = "memory_structured_path_agent";
    create_agent(&app.app, path_agent, "Structured Memory path agent").await;
    install_manager(&app, path_agent, 0).await;

    let write = request(
        &app.app,
        Method::POST,
        &format!("/api/agents/{path_agent}/memory/write"),
        Some(json!({
            "kind": "preference",
            "scope": "user",
            "title": "Direct engineering conclusions",
            "content": "User prefers direct engineering conclusions.",
            "labels": ["response style", "engineering"],
            "search_terms": ["direct conclusions", "直接结论"],
            "confidence": 0.9,
            "evidence": {"chat_id": "chat-1", "turn_id": "turn-1"}
        })),
        None,
    )
    .await;

    assert_eq!(write.status(), StatusCode::OK);
    let write_body = read_json(write).await;
    assert_eq!(write_body["rejected"], false);
    assert_eq!(write_body["record"]["id"], "mem_http_contract");
    assert_eq!(write_body["record"]["agent_id"], path_agent);
    assert_eq!(
        write_body["record"]["title"],
        "Direct engineering conclusions"
    );
    assert_eq!(
        write_body["record"]["content"],
        "User prefers direct engineering conclusions."
    );
    assert_eq!(write_body["record"]["sources"][0]["chat_id"], "chat-1");
    assert_eq!(write_body["record"]["sources"][0]["turn_id"], "turn-1");
    assert!(write_body["record"]["terms"]
        .as_array()
        .expect("terms")
        .iter()
        .any(|term| term["term"] == "response style" && term["term_kind"] == "label"));
    assert!(write_body["record"]["terms"]
        .as_array()
        .expect("terms")
        .iter()
        .any(|term| term["term"] == "direct conclusions" && term["term_kind"] == "search_term"));

    let get = request(
        &app.app,
        Method::POST,
        &format!("/api/agents/{path_agent}/memory/get"),
        Some(json!({"ids": ["mem_http_contract"], "include_inactive": true})),
        None,
    )
    .await;
    assert_eq!(get.status(), StatusCode::OK);
    let get_body = read_json(get).await;
    assert_eq!(get_body["records"][0]["id"], "mem_http_contract");
    assert_eq!(get_body["records"][0]["agent_id"], path_agent);

    let list = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{path_agent}/memory"),
        None,
        None,
    )
    .await;
    assert_eq!(list.status(), StatusCode::OK);
    let list_body = read_json(list).await;
    assert_eq!(list_body["records"][0]["id"], "mem_http_contract");
    assert_eq!(list_body["records"][0]["agent_id"], path_agent);

    let update = request(
        &app.app,
        Method::PATCH,
        &format!("/api/agents/{path_agent}/memory/mem_http_contract"),
        Some(json!({
            "kind": "personal_fact",
            "scope": "user",
            "title": "Concise engineering conclusions",
            "content": "User prefers concise direct conclusions.",
            "confidence": 0.8,
            "importance": 0.6,
            "search_terms": ["concise conclusions", "简洁结论"]
        })),
        None,
    )
    .await;
    assert_eq!(update.status(), StatusCode::OK);
    let update_body = read_json(update).await;
    assert_eq!(
        update_body["content"],
        "User prefers concise direct conclusions."
    );
    assert_eq!(update_body["title"], "Concise engineering conclusions");

    let delete = request(
        &app.app,
        Method::DELETE,
        &format!("/api/agents/{path_agent}/memory/mem_http_contract"),
        None,
        None,
    )
    .await;
    assert_eq!(delete.status(), StatusCode::OK);
    assert_eq!(read_json(delete).await["deleted"], true);

    let reindex = request(
        &app.app,
        Method::POST,
        &format!("/api/agents/{path_agent}/memory/reindex"),
        None,
        None,
    )
    .await;
    assert_eq!(reindex.status(), StatusCode::OK);
    assert_eq!(read_json(reindex).await["documents_indexed"], 7);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn memory_visibility_routes_update_current_agent_grants() {
    let app = build_app();
    let owner = "memory_visibility_owner";
    let grantee = "memory_visibility_reader";
    create_agent(&app.app, owner, "Memory visibility owner").await;
    create_agent(&app.app, grantee, "Memory visibility reader").await;
    install_manager(&app, owner, 0).await;

    let write = request(
        &app.app,
        Method::POST,
        &format!("/api/agents/{owner}/memory/write"),
        Some(json!({
            "title": "Shareable visibility marker",
            "content": "Shareable Memory visibility marker.",
            "kind": "preference",
            "scope": "user",
            "search_terms": ["shareable marker"]
        })),
        None,
    )
    .await;
    assert_eq!(write.status(), StatusCode::OK);

    let options = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{owner}/memory/visibility-options"),
        None,
        None,
    )
    .await;
    assert_eq!(options.status(), StatusCode::OK);
    let options_body = read_json(options).await;
    assert!(options_body["agents"]
        .as_array()
        .expect("agent options")
        .iter()
        .any(|agent| agent["id"] == grantee));
    assert!(!options_body["agents"]
        .as_array()
        .expect("agent options")
        .iter()
        .any(|agent| agent["id"] == owner));

    let update = request(
        &app.app,
        Method::PATCH,
        &format!("/api/agents/{owner}/memory/mem_http_contract/visibility"),
        Some(json!({
            "mode": "selected",
            "grantee_agent_ids": [grantee]
        })),
        None,
    )
    .await;
    assert_eq!(update.status(), StatusCode::OK);
    let update_body = read_json(update).await;
    assert_eq!(update_body["mode"], "selected");
    assert_eq!(update_body["grantee_agent_ids"][0], grantee);

    let list = request(
        &app.app,
        Method::GET,
        &format!("/api/agents/{owner}/memory"),
        None,
        None,
    )
    .await;
    assert_eq!(list.status(), StatusCode::OK);
    let list_body = read_json(list).await;
    assert_eq!(
        list_body["visibility"]["mem_http_contract"]["grantee_agent_ids"][0],
        grantee
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn path_scoped_memory_write_accepts_title_content_labels_and_search_terms() {
    let app = build_app();
    let path_agent = "memory_minimal_write_agent";
    create_agent(&app.app, path_agent, "Minimal Memory write agent").await;
    install_manager(&app, path_agent, 0).await;

    let write = request(
        &app.app,
        Method::POST,
        &format!("/api/agents/{path_agent}/memory/write"),
        Some(json!({
            "title": "Family photos location",
            "content": "User keeps family photos on the Windows PC.",
            "labels": ["family photos"],
            "search_terms": ["photo album"]
        })),
        None,
    )
    .await;

    assert_eq!(write.status(), StatusCode::OK);
    let body = read_json(write).await;
    assert_eq!(body["record"]["kind"], "fact");
    assert_eq!(body["record"]["scope"], "agent");
    assert!(body["record"]["terms"]
        .as_array()
        .expect("terms")
        .iter()
        .any(|term| term["term"] == "family photos" && term["term_kind"] == "label"));
    assert!(body["record"]["terms"]
        .as_array()
        .expect("terms")
        .iter()
        .any(|term| term["term"] == "photo album" && term["term_kind"] == "search_term"));
}
