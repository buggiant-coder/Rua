use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

use rua_core::id::AgentId;

pub const MEMORY_TITLE_MAX_CHARS: usize = 60;

pub fn normalize_memory_title(title: &str) -> Result<String, String> {
    let title = title.trim();
    if title.is_empty() {
        return Err("memory title is empty".to_string());
    }
    if title.contains(['\r', '\n']) {
        return Err("memory title must be a single line".to_string());
    }
    if title.chars().count() > MEMORY_TITLE_MAX_CHARS {
        return Err(format!(
            "memory title exceeds {MEMORY_TITLE_MAX_CHARS} characters"
        ));
    }
    Ok(title.to_string())
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MemoryId(pub String);

impl MemoryId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

impl std::fmt::Display for MemoryId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryScope {
    User,
    Agent,
    Workspace,
    Project,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    Fact,
    Preference,
    Convention,
    ProjectFact,
    ToolQuirk,
    PersonalFact,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryRecordStatus {
    Active,
    Superseded,
    Expired,
    Deleted,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemorySourceEvidence {
    #[serde(default)]
    pub chat_id: Option<String>,
    #[serde(default)]
    pub turn_id: Option<String>,
    #[serde(default)]
    pub message_id: Option<String>,
    #[serde(default)]
    pub quote: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryTermKind {
    Label,
    SearchTerm,
    DerivedTerm,
    ResourceToken,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryTermSource {
    Agent,
    MemoryManager,
    Autodream,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryTerm {
    pub term: String,
    pub normalized_term: String,
    pub term_kind: MemoryTermKind,
    pub source: MemoryTermSource,
    #[serde(default)]
    pub language: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryResource {
    pub id: String,
    pub resource_kind: String,
    pub locator: String,
    pub normalized_locator: String,
    #[serde(default)]
    pub resource_device_id: Option<String>,
    #[serde(default)]
    pub resource_device_name_snapshot: Option<String>,
    #[serde(default)]
    pub host_profile: Option<String>,
    pub portable: bool,
    pub created_by: String,
    #[serde(default)]
    pub metadata: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemorySearchDoc {
    pub memory_id: MemoryId,
    pub lexical_text: String,
    pub fuzzy_text: String,
    pub semantic_text: String,
    pub resource_text: String,
    pub updated_at: DateTime<Utc>,
    pub content_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemorySlot {
    pub id: String,
    pub agent_id: String,
    pub slot_key: String,
    pub scope: String,
    pub size_limit: usize,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemorySlotItem {
    pub slot_id: String,
    pub memory_id: MemoryId,
    pub render_order: i64,
    pub render_mode: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemorySlotSnapshot {
    pub slot: MemorySlot,
    #[serde(default)]
    pub items: Vec<MemorySlotItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryLink {
    pub from_id: MemoryId,
    pub to_id: MemoryId,
    pub relation: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryRelation {
    pub id: String,
    pub from_id: MemoryId,
    pub to_id: MemoryId,
    pub relation: String,
    #[serde(default)]
    pub confidence: Option<f32>,
    pub created_by: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryVisibilityMode {
    Private,
    Selected,
    AllAgents,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryVisibilityGrant {
    pub memory_id: MemoryId,
    pub owner_agent_id: String,
    pub grantee_agent_id: String,
    pub permission: String,
    pub created_by: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryVisibility {
    pub memory_id: MemoryId,
    pub owner_agent_id: String,
    pub mode: MemoryVisibilityMode,
    #[serde(default)]
    pub grantee_agent_ids: Vec<String>,
    #[serde(default)]
    pub grants: Vec<MemoryVisibilityGrant>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryRecord {
    pub id: MemoryId,
    pub agent_id: String,
    pub scope: MemoryScope,
    pub kind: MemoryKind,
    pub title: String,
    pub content: String,
    pub status: MemoryRecordStatus,
    pub confidence: f32,
    #[serde(default)]
    pub importance: Option<f32>,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub updated_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub last_used_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub superseded_by: Option<MemoryId>,
    #[serde(default)]
    pub written_on_device_id: Option<String>,
    #[serde(default)]
    pub written_on_device_name_snapshot: Option<String>,
    #[serde(default)]
    pub written_on_host_profile: Option<String>,
    #[serde(default)]
    pub resource_device_id: Option<String>,
    #[serde(default)]
    pub resource_device_name_snapshot: Option<String>,
    #[serde(default)]
    pub metadata: serde_json::Value,
    #[serde(default)]
    pub sources: Vec<MemorySourceEvidence>,
    #[serde(default)]
    pub links: Vec<MemoryLink>,
    #[serde(default)]
    pub terms: Vec<MemoryTerm>,
    #[serde(default)]
    pub resources: Vec<MemoryResource>,
    #[serde(default)]
    pub relations: Vec<MemoryRelation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryGetRequest {
    pub ids: Vec<MemoryId>,
    #[serde(default)]
    pub include_inactive: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryWriteRequest {
    pub agent_id: String,
    pub kind: MemoryKind,
    pub scope: MemoryScope,
    pub title: String,
    pub content: String,
    #[serde(default)]
    pub confidence: Option<f32>,
    #[serde(default)]
    pub importance: Option<f32>,
    #[serde(default)]
    pub ttl_days: Option<i64>,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub search_terms: Vec<String>,
    #[serde(default)]
    pub evidence: Option<MemorySourceEvidence>,
    #[serde(default)]
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryUpdateRequest {
    pub id: MemoryId,
    pub agent_id: String,
    pub kind: MemoryKind,
    pub scope: MemoryScope,
    pub title: String,
    pub content: String,
    pub confidence: f32,
    #[serde(default)]
    pub importance: Option<f32>,
    #[serde(default)]
    pub search_terms: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryVisibilityUpdateRequest {
    pub mode: MemoryVisibilityMode,
    #[serde(default)]
    pub grantee_agent_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryWriteOutcome {
    pub rejected: bool,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub record: Option<MemoryRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryMaintenanceCandidateRequest {
    pub agent_id: String,
    #[serde(default = "default_maintenance_limit")]
    pub limit: usize,
}

fn default_maintenance_limit() -> usize {
    10
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryMaintenanceCluster {
    pub reason: String,
    pub records: Vec<MemoryRecord>,
    #[serde(default)]
    pub external_context: Vec<MemoryRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryMaintenancePacket {
    pub base_revision: String,
    pub clusters: Vec<MemoryMaintenanceCluster>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryBatchCreateRecord {
    #[serde(default)]
    pub id: Option<MemoryId>,
    pub kind: MemoryKind,
    pub scope: MemoryScope,
    pub title: String,
    pub content: String,
    #[serde(default)]
    pub confidence: Option<f32>,
    #[serde(default)]
    pub importance: Option<f32>,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub search_terms: Vec<String>,
    #[serde(default)]
    pub metadata: serde_json::Value,
    #[serde(default)]
    pub sources: Vec<MemorySourceEvidence>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryBatchRelation {
    pub from_id: MemoryId,
    pub to_id: MemoryId,
    pub relation: String,
    #[serde(default)]
    pub confidence: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum MemoryBatchOperation {
    Create {
        record: Box<MemoryBatchCreateRecord>,
        #[serde(default)]
        supersedes: Vec<MemoryId>,
    },
    Supersede {
        ids: Vec<MemoryId>,
        by: Vec<MemoryId>,
    },
    Relate {
        relations: Vec<MemoryBatchRelation>,
    },
    SetStatus {
        ids: Vec<MemoryId>,
        status: MemoryRecordStatus,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryBatchApplyRequest {
    pub agent_id: String,
    pub base_revision: String,
    pub ops: Vec<MemoryBatchOperation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryBatchApplyOutcome {
    pub applied: bool,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub records: Vec<MemoryRecord>,
}

#[derive(Debug, Clone)]
pub enum ReindexScope {
    Full,

    Agent(AgentId),

    DateRange { start: NaiveDate, end: NaiveDate },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReindexReport {
    pub documents_indexed: usize,
    pub errors: Vec<String>,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryHealth {
    pub index_ok: bool,
    pub source_files_ok: bool,
    pub document_count: usize,
    pub index_size_bytes: u64,
    #[serde(default)]
    pub issues: Vec<String>,
}
