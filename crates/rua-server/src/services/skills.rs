use std::collections::HashMap;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use rua_core::{Result, RuaError};
use rua_plugin_runtime::PluginSkillRegistry;
use rua_skills::pool_service::SkillPoolService;
use rua_skills::service::{
    ImportOptions, ImportOutcome, SaveOutcome, SkillService as CoreSkillService,
};
use rua_skills::types::{SkillInfo as CoreSkillInfo, SkillSource as CoreSkillSource};
use rua_skills::{
    builtin_skill_names, bump_manifest_version, ensure_skill_pool_initialized_in,
    import_builtin_skills_in, list_builtin_import_candidates_in, maybe_scan_skill,
    read_install_meta, reconcile_pool_manifest_with, reconcile_workspace_manifest_with,
    update_single_builtin_in, BuiltinCandidate, BuiltinStatus, SkillError, WorkspaceSkillCells,
    ALL_SKILL_ROUTING_CHANNELS,
};
use serde_json::{json, Map, Value};
use tokio::sync::Mutex as AsyncMutex;

use rua_skills_hub::{install_skill_from_hub, install_skill_to_pool_from_hub, HubError};

use crate::endpoints::dto;
use crate::endpoints::skills_hub::{shared_client, PoolBundleInstaller, WorkspaceBundleInstaller};
use crate::services::hub_install_task::{
    run_install_task, HubInstallRegistry, HubInstallTask, DEFAULT_INSTALL_TIMEOUT,
};

const DEFAULT_WORKSPACE_ID: &str = "default";

pub struct FileSkillService {
    data_dir: PathBuf,
    workspace_skill_cells: Option<Arc<WorkspaceSkillCells>>,

    workspace_locks: std::sync::Mutex<HashMap<String, Arc<AsyncMutex<()>>>>,
    plugin_skill_registry: Option<Arc<PluginSkillRegistry>>,

    hub_install_registry: Arc<HubInstallRegistry>,
}

impl FileSkillService {
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            data_dir,
            workspace_skill_cells: None,
            workspace_locks: std::sync::Mutex::new(HashMap::new()),
            plugin_skill_registry: None,
            hub_install_registry: Arc::new(HubInstallRegistry::new()),
        }
    }

    pub fn with_plugin_skill_registry(mut self, registry: Arc<PluginSkillRegistry>) -> Self {
        self.plugin_skill_registry = Some(registry);
        self
    }

    pub fn with_workspace_skill_cells(mut self, cells: Arc<WorkspaceSkillCells>) -> Self {
        self.workspace_skill_cells = Some(cells);
        self
    }

    pub fn with_builtin_skills_dir(self, _dir: PathBuf) -> Self {
        self
    }

    pub fn initialize_pool(&self) {
        let pool_dir = self.pool_dir();
        if let Err(e) = std::fs::create_dir_all(&pool_dir) {
            tracing::warn!("initialize_pool: create dir failed: {e}");
            return;
        }
        let report = crate::seed::skills::ensure_builtin_skill_pool_seeded(&self.data_dir);
        for outcome in report.outcomes {
            tracing::debug!(
                entry_id = %outcome.entry_id,
                status = ?outcome.status,
                "skill pool seed completed"
            );
        }
    }

    fn resolve_agent(agent_id: Option<&str>) -> &str {
        agent_id.unwrap_or(DEFAULT_WORKSPACE_ID)
    }

    fn workspace_dir(&self, agent_id: Option<&str>) -> PathBuf {
        self.data_dir
            .join("workspaces")
            .join(Self::resolve_agent(agent_id))
    }

    pub fn workspace_dir_for(&self, agent_id: Option<&str>) -> PathBuf {
        self.workspace_dir(agent_id)
    }

    fn pool_dir(&self) -> PathBuf {
        self.data_dir.join("skill_pool")
    }

    fn workspace_service(&self, agent_id: Option<&str>) -> Result<CoreSkillService> {
        CoreSkillService::new(self.workspace_dir(agent_id)).map_err(skill_err_to_rua)
    }

    async fn read_workspace_manifest_map(
        &self,
        agent_id: Option<&str>,
    ) -> Result<Map<String, Value>> {
        if let Some(cells) = &self.workspace_skill_cells {
            let agent_id = Self::resolve_agent(agent_id);
            let value = cells
                .cell(agent_id)
                .read_value()
                .await
                .map_err(skill_err_to_rua)?;
            return Ok(value
                .and_then(|v| v.as_object().cloned())
                .unwrap_or_default());
        }
        Ok(read_workspace_manifest_raw(&self.workspace_dir(agent_id)))
    }

    fn read_workspace_skill_install_meta(
        &self,
        agent_id: Option<&str>,
        skill_name: &str,
    ) -> Option<Value> {
        read_install_meta(&self.workspace_dir(agent_id).join("skills").join(skill_name))
            .and_then(|meta| serde_json::to_value(meta).ok())
    }

    async fn mutate_workspace_manifest_via_cells<F>(
        &self,
        agent_id: Option<&str>,
        mutator: F,
    ) -> Result<Value>
    where
        F: FnOnce(Option<Value>) -> Value + Send,
    {
        let Some(cells) = &self.workspace_skill_cells else {
            return Ok(mutator(None));
        };
        cells
            .cell(Self::resolve_agent(agent_id))
            .update_value(mutator)
            .await
            .map_err(skill_err_to_rua)
    }

    async fn set_skill_enabled_via_cells(
        &self,
        agent_id: Option<&str>,
        svc: &CoreSkillService,
        skill_name: &str,
        enabled: bool,
    ) -> Result<bool> {
        if self.workspace_skill_cells.is_none() {
            return if enabled {
                svc.enable_skill(skill_name).map_err(skill_err_to_rua)
            } else {
                svc.disable_skill(skill_name).map_err(skill_err_to_rua)
            };
        }

        let skill_dir = svc.workspace_dir().join("skills").join(skill_name);
        if enabled {
            if !skill_dir.exists() {
                return Ok(false);
            }
            maybe_scan_skill(svc.scanner(), &skill_dir, skill_name).map_err(skill_err_to_rua)?;
        }

        let target = skill_name.to_string();
        let updated_payload = self
            .mutate_workspace_manifest_via_cells(agent_id, move |current| {
                let mut payload = workspace_manifest_value_or_default(current);
                if let Some(entry) = payload
                    .get_mut("skills")
                    .and_then(Value::as_object_mut)
                    .and_then(|skills| skills.get_mut(&target))
                    .and_then(Value::as_object_mut)
                {
                    entry.insert("enabled".to_string(), Value::Bool(enabled));
                    if enabled && !entry.get("channels").map(|v| v.is_array()).unwrap_or(false) {
                        entry.insert(
                            "channels".to_string(),
                            Value::Array(vec![Value::String("all".to_string())]),
                        );
                    }
                    bump_manifest_version_value(&mut payload);
                }
                payload
            })
            .await?;
        Ok(workspace_manifest_has_skill(&updated_payload, skill_name))
    }

    async fn set_skill_channels_via_cells(
        &self,
        agent_id: Option<&str>,
        svc: &CoreSkillService,
        skill_name: &str,
        channels: Vec<String>,
    ) -> Result<bool> {
        if self.workspace_skill_cells.is_none() {
            return svc
                .set_skill_channels(skill_name, channels)
                .map_err(skill_err_to_rua);
        }

        let normalised = normalize_skill_channels(channels).map_err(skill_err_to_rua)?;
        let target = skill_name.to_string();
        let updated_payload = self
            .mutate_workspace_manifest_via_cells(agent_id, move |current| {
                let mut payload = workspace_manifest_value_or_default(current);
                if let Some(entry) = payload
                    .get_mut("skills")
                    .and_then(Value::as_object_mut)
                    .and_then(|skills| skills.get_mut(&target))
                    .and_then(Value::as_object_mut)
                {
                    entry.insert(
                        "channels".to_string(),
                        Value::Array(normalised.iter().cloned().map(Value::String).collect()),
                    );
                    bump_manifest_version_value(&mut payload);
                }
                payload
            })
            .await?;
        let updated = workspace_manifest_has_skill(&updated_payload, skill_name);

        if updated
            && svc
                .get_skill(skill_name)
                .map_err(skill_err_to_rua)?
                .is_none()
        {
            return Ok(false);
        }
        Ok(updated)
    }

    async fn set_skill_config_via_cells(
        &self,
        agent_id: Option<&str>,
        svc: &CoreSkillService,
        skill_name: &str,
        config: Map<String, Value>,
    ) -> Result<bool> {
        if self.workspace_skill_cells.is_none() {
            return svc
                .set_skill_config(skill_name, config)
                .map_err(skill_err_to_rua);
        }

        let target = skill_name.to_string();
        let updated_payload = self
            .mutate_workspace_manifest_via_cells(agent_id, move |current| {
                let mut payload = workspace_manifest_value_or_default(current);
                if let Some(entry) = payload
                    .get_mut("skills")
                    .and_then(Value::as_object_mut)
                    .and_then(|skills| skills.get_mut(&target))
                    .and_then(Value::as_object_mut)
                {
                    entry.insert("config".to_string(), Value::Object(config));
                    bump_manifest_version_value(&mut payload);
                }
                payload
            })
            .await?;
        let updated = workspace_manifest_has_skill(&updated_payload, skill_name);
        if updated
            && svc
                .get_skill(skill_name)
                .map_err(skill_err_to_rua)?
                .is_none()
        {
            return Ok(false);
        }
        Ok(updated)
    }

    fn pool_service(&self) -> Result<SkillPoolService> {
        SkillPoolService::new_in(self.pool_dir()).map_err(skill_err_to_rua)
    }

    fn workspace_lock(&self, agent_id: Option<&str>) -> Arc<AsyncMutex<()>> {
        let key = Self::resolve_agent(agent_id).to_string();
        let mut guard = self.workspace_locks.lock().expect("lock map poisoned");
        guard
            .entry(key)
            .or_insert_with(|| Arc::new(AsyncMutex::new(())))
            .clone()
    }

    fn enumerate_workspaces(&self) -> Vec<String> {
        let root = self.data_dir.join("workspaces");
        let Ok(entries) = std::fs::read_dir(&root) else {
            return Vec::new();
        };
        let mut out: Vec<String> = entries
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        out.sort();
        out
    }
}

fn skill_err_to_rua(err: SkillError) -> RuaError {
    match err {
        SkillError::Conflict(detail) => RuaError::Conflict {
            resource: "skill".to_string(),
            detail,
        },
        SkillError::InvalidFrontmatter(message) | SkillError::InvalidSkillName(message) => {
            RuaError::Validation { message }
        }
        SkillError::Io(e) => RuaError::Io {
            message: e.to_string(),
        },
        SkillError::Json(e) => RuaError::Config {
            message: format!("json: {e}"),
        },
        SkillError::Yaml(e) => RuaError::Config {
            message: format!("yaml: {e}"),
        },
        SkillError::Consistency(message) => RuaError::Config { message },





        SkillError::ScanBlocked {
            skill_name,
            max_severity,
            summary,
            ..
        } => RuaError::Security {
            message: format!(
                "skill '{skill_name}' blocked by security scan ({max_severity}): {summary}"
            ),
        },
        SkillError::SourceConflict {
            skill_name,
            existing_source,
            requested_source,
        } => RuaError::Conflict {
            resource: "skill".to_string(),
            detail: format!(
                "skill '{skill_name}' owned by {existing_source}, cannot be claimed by {requested_source}"
            ),
        },
    }
}

fn hub_err_to_rua(err: HubError) -> RuaError {
    match err {
        HubError::Cancelled => RuaError::Cancelled,
        HubError::UnsupportedSource(src) => RuaError::Validation {
            message: format!("source not supported: {src}"),
        },
        HubError::InvalidBundle(msg) => RuaError::Validation {
            message: format!("invalid bundle: {msg}"),
        },
        HubError::ZipLimit => RuaError::Validation {
            message: "bundle exceeded the size or entry-count limits".to_string(),
        },
        HubError::RateLimit => RuaError::Provider {
            provider: "skills_hub".to_string(),
            message: "rate limit exceeded".to_string(),
            retryable: true,
        },
        HubError::Http { status: 404, body } => RuaError::NotFound {
            resource: "hub_skill".to_string(),
            id: body,
        },
        HubError::Http { status, body } => RuaError::Io {
            message: format!("HTTP error {status}: {body}"),
        },
        HubError::Transport(msg) => RuaError::Io {
            message: format!("transport error: {msg}"),
        },
        HubError::Installer(msg) if msg.contains("already exists") => RuaError::Conflict {
            resource: "skill".to_string(),
            detail: msg,
        },
        HubError::Installer(msg) => RuaError::Io {
            message: format!("installer error: {msg}"),
        },
    }
}

fn not_found(resource: &str, id: &str) -> RuaError {
    RuaError::NotFound {
        resource: resource.to_string(),
        id: id.to_string(),
    }
}

fn hub_task_to_dto(task: &HubInstallTask) -> dto::SkillHubInstallStatus {
    let mut extra = Map::new();
    extra.insert(
        "bundle_url".to_string(),
        Value::String(task.bundle_url.clone()),
    );
    extra.insert("version".to_string(), Value::String(task.version.clone()));
    extra.insert("enable".to_string(), Value::Bool(task.enable));
    extra.insert("created_at".to_string(), json!(task.created_at));
    extra.insert("updated_at".to_string(), json!(task.updated_at));
    if let Some(result) = &task.result {
        extra.insert("result".to_string(), result.clone());
    }
    dto::SkillHubInstallStatus {
        task_id: task.task_id.clone(),
        status: task.status.as_str().to_string(),
        progress: None,
        error: task.error.clone(),
        extra,
    }
}

fn skill_source_to_string(src: CoreSkillSource) -> String {
    src.as_str().to_string()
}

fn build_skill_extra(info: &CoreSkillInfo) -> Map<String, Value> {
    let mut extra = Map::new();
    extra.insert(
        "version_text".to_string(),
        Value::String(info.version_text.clone()),
    );
    if !info.emoji.is_empty() {
        extra.insert("emoji".to_string(), Value::String(info.emoji.clone()));
    }
    extra.insert("references".to_string(), info.references.clone());
    extra.insert("scripts".to_string(), info.scripts.clone());
    extra
}

fn core_to_dto_info(
    info: CoreSkillInfo,
    enabled: bool,
    install_meta: Option<Value>,
) -> dto::SkillInfo {
    let extra = build_skill_extra(&info);
    dto::SkillInfo {
        id: info.name.clone(),
        name: info.name,
        description: info.description,
        enabled,
        source: skill_source_to_string(info.source),
        content_hash: None,
        tags: info.tags,
        install_meta,
        extra,
    }
}

fn core_to_dto_detail(
    info: CoreSkillInfo,
    enabled: bool,
    install_meta: Option<Value>,
) -> dto::SkillDetail {
    let extra = build_skill_extra(&info);
    dto::SkillDetail {
        id: info.name.clone(),
        name: info.name,
        description: info.description,
        enabled,
        source: skill_source_to_string(info.source),
        content: info.content,
        content_hash: None,
        tags: info.tags,
        install_meta,
        extra,
    }
}

fn core_to_workspace_spec(
    info: CoreSkillInfo,
    enabled: bool,
    channels: Vec<String>,
    config: Option<Value>,
    last_updated: Option<String>,
    install_meta: Option<Value>,
) -> dto::WorkspaceSkillSpec {
    let mut extra = Map::new();
    if !info.emoji.is_empty() {
        extra.insert("emoji".to_string(), Value::String(info.emoji.clone()));
    }
    extra.insert("references".to_string(), info.references);
    extra.insert("scripts".to_string(), info.scripts);
    extra.insert(
        "tags".to_string(),
        Value::Array(info.tags.into_iter().map(Value::String).collect()),
    );
    dto::WorkspaceSkillSpec {
        name: info.name,
        description: info.description,
        version_text: info.version_text,
        content: info.content,
        source: skill_source_to_string(info.source),
        enabled,
        channels,
        config,
        last_updated,
        install_meta,
        extra,
    }
}

fn core_to_pool_entry(
    info: CoreSkillInfo,
    sync_status: Option<BuiltinStatus>,
    config: Option<Value>,
    protected: bool,
    commit_text: String,
    install_meta: Option<Value>,
) -> dto::SkillPoolEntry {
    let mut extra = Map::new();
    extra.insert(
        "description".to_string(),
        Value::String(info.description.clone()),
    );
    extra.insert(
        "source".to_string(),
        Value::String(skill_source_to_string(info.source)),
    );
    extra.insert("commit_text".to_string(), Value::String(commit_text));
    extra.insert("protected".to_string(), Value::Bool(protected));
    extra.insert("references".to_string(), info.references);
    extra.insert("scripts".to_string(), info.scripts);
    if !info.emoji.is_empty() {
        extra.insert("emoji".to_string(), Value::String(info.emoji.clone()));
    }
    extra.insert(
        "sync_status".to_string(),
        Value::String(
            sync_status
                .map(|s| s.as_str().to_string())
                .unwrap_or_default(),
        ),
    );
    extra.insert(
        "latest_version_text".to_string(),
        Value::String(String::new()),
    );
    if let Some(cfg) = config {
        extra.insert("config".to_string(), cfg);
    }
    extra.insert("content".to_string(), Value::String(info.content));
    dto::SkillPoolEntry {
        id: info.name.clone(),
        name: info.name,
        description: info.description,
        author: String::new(),
        version: info.version_text,
        downloads: 0,
        tags: info.tags,
        install_meta,
        extra,
    }
}

fn read_workspace_manifest_raw(workspace_dir: &std::path::Path) -> Map<String, Value> {
    let path = workspace_dir.join("skill.json");
    let Ok(bytes) = std::fs::read(&path) else {
        return Map::new();
    };
    if bytes.is_empty() {
        return Map::new();
    }
    serde_json::from_slice(&bytes).unwrap_or_default()
}

fn workspace_manifest_value_or_default(current: Option<Value>) -> Value {
    match current {
        Some(Value::Object(mut payload)) => {
            payload
                .entry("schema_version".to_string())
                .or_insert_with(|| Value::String(rua_skills::WORKSPACE_SCHEMA_VERSION.to_string()));
            payload
                .entry("version".to_string())
                .or_insert_with(|| Value::Number(0u64.into()));
            payload
                .entry("skills".to_string())
                .or_insert_with(|| Value::Object(Map::new()));
            Value::Object(payload)
        }
        _ => Value::Object(rua_skills::WorkspaceManifest::default_payload()),
    }
}

fn bump_manifest_version_value(value: &mut Value) {
    if let Value::Object(payload) = value {
        bump_manifest_version(payload);
    }
}

fn workspace_manifest_has_skill(value: &Value, skill_name: &str) -> bool {
    value
        .get("skills")
        .and_then(Value::as_object)
        .is_some_and(|skills| skills.contains_key(skill_name))
}

fn normalize_skill_channels(channels: Vec<String>) -> std::result::Result<Vec<String>, SkillError> {
    if channels.is_empty() {
        return Ok(vec!["all".to_string()]);
    }
    for channel in &channels {
        if channel != "all" && !ALL_SKILL_ROUTING_CHANNELS.contains(&channel.as_str()) {
            return Err(SkillError::InvalidSkillName(format!(
                "unknown channel '{channel}' (allowed: 'all' or one of {ALL_SKILL_ROUTING_CHANNELS:?})"
            )));
        }
    }
    Ok(channels)
}

fn read_pool_manifest_raw(pool_dir: &std::path::Path) -> Map<String, Value> {
    let path = pool_dir.join("skill.json");
    let Ok(bytes) = std::fs::read(&path) else {
        return Map::new();
    };
    if bytes.is_empty() {
        return Map::new();
    }
    serde_json::from_slice(&bytes).unwrap_or_default()
}

fn workspace_entry_enabled(payload: &Map<String, Value>, skill_name: &str) -> bool {
    payload
        .get("skills")
        .and_then(|v| v.as_object())
        .and_then(|o| o.get(skill_name))
        .and_then(|v| v.as_object())
        .and_then(|o| o.get("enabled"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

fn workspace_entry_install_meta(payload: &Map<String, Value>, skill_name: &str) -> Option<Value> {
    payload
        .get("skills")
        .and_then(|v| v.as_object())
        .and_then(|o| o.get(skill_name))
        .and_then(|v| v.as_object())
        .and_then(|o| o.get("install_meta"))
        .cloned()
}

fn workspace_entry_channels(payload: &Map<String, Value>, skill_name: &str) -> Vec<String> {
    payload
        .get("skills")
        .and_then(|v| v.as_object())
        .and_then(|o| o.get(skill_name))
        .and_then(|v| v.as_object())
        .and_then(|o| o.get("channels"))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .filter(|v: &Vec<String>| !v.is_empty())
        .unwrap_or_else(|| vec!["all".to_string()])
}

fn workspace_entry_config(payload: &Map<String, Value>, skill_name: &str) -> Option<Value> {
    payload
        .get("skills")
        .and_then(|v| v.as_object())
        .and_then(|o| o.get(skill_name))
        .and_then(|v| v.as_object())
        .and_then(|o| o.get("config"))
        .cloned()
}

fn workspace_entry_updated_at(payload: &Map<String, Value>, skill_name: &str) -> Option<String> {
    payload
        .get("skills")
        .and_then(|v| v.as_object())
        .and_then(|o| o.get(skill_name))
        .and_then(|v| v.as_object())
        .and_then(|o| o.get("updated_at"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

fn pool_entry_config(payload: &Map<String, Value>, skill_name: &str) -> Option<Value> {
    payload
        .get("skills")
        .and_then(|v| v.as_object())
        .and_then(|o| o.get(skill_name))
        .and_then(|v| v.as_object())
        .and_then(|o| o.get("config"))
        .cloned()
}

fn pool_entry_protected(payload: &Map<String, Value>, skill_name: &str) -> bool {
    payload
        .get("skills")
        .and_then(|v| v.as_object())
        .and_then(|o| o.get(skill_name))
        .and_then(|v| v.as_object())
        .and_then(|o| o.get("protected"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

fn pool_entry_commit_text(payload: &Map<String, Value>, skill_name: &str) -> String {
    payload
        .get("skills")
        .and_then(|v| v.as_object())
        .and_then(|o| o.get(skill_name))
        .and_then(|v| v.as_object())
        .and_then(|o| o.get("commit_text"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_default()
}

fn extract_config_from_dto(config: dto::SkillConfig) -> Map<String, Value> {
    if let Some(Value::Object(inner)) = config.inner.get("config") {
        inner.clone()
    } else {
        config.inner
    }
}

fn extract_pool_config_from_dto(config: dto::SkillPoolConfig) -> Map<String, Value> {
    if let Some(Value::Object(inner)) = config.inner.get("config") {
        inner.clone()
    } else {
        config.inner
    }
}

fn builtin_candidate_to_dto(c: BuiltinCandidate) -> dto::BuiltinImportSpec {
    let status = c.status.as_str().to_string();
    let current_source = if c.current_source.is_empty() {
        None
    } else {
        Some(c.current_source)
    };
    dto::BuiltinImportSpec {
        name: c.name,
        description: c.description,
        version_text: c.version_text,
        current_version_text: Some(c.current_version_text),
        current_source,
        status,
        extra: Map::new(),
    }
}

fn extract_create_config(spec: &dto::SkillCreate) -> Option<Map<String, Value>> {
    match spec.extra.get("config") {
        Some(Value::Object(o)) => Some(o.clone()),
        _ => None,
    }
}

fn extract_create_enable(spec: &dto::SkillCreate) -> bool {
    spec.extra
        .get("enable")
        .and_then(|v| v.as_bool())
        .unwrap_or(true)
}

fn build_create_skill_md(spec: &dto::SkillCreate) -> String {
    if !spec.content.is_empty() {
        spec.content.clone()
    } else {
        let description = if spec.description.is_empty() {
            spec.name.clone()
        } else {
            spec.description.clone()
        };
        format!(
            "---\nname: {}\ndescription: {}\n---\n",
            spec.name, description
        )
    }
}

fn extract_save_config(req: &dto::SkillSaveRequest) -> Option<Map<String, Value>> {
    match req.config.as_ref() {
        Some(Value::Object(o)) => Some(o.clone()),
        _ => None,
    }
}

impl FileSkillService {
    pub async fn list_skills(&self, agent_id: Option<&str>) -> Result<Vec<dto::SkillInfo>> {
        let svc = self.workspace_service(agent_id)?;
        let infos = svc.list_all_skills().map_err(skill_err_to_rua)?;
        let payload = self.read_workspace_manifest_map(agent_id).await?;
        let mut out: Vec<dto::SkillInfo> = infos
            .into_iter()
            .map(|info| {
                let enabled = workspace_entry_enabled(&payload, &info.name);
                let meta = workspace_entry_install_meta(&payload, &info.name)
                    .or_else(|| self.read_workspace_skill_install_meta(agent_id, &info.name));
                core_to_dto_info(info, enabled, meta)
            })
            .collect();

        if let Some(ref sr) = self.plugin_skill_registry {
            for info in sr.list_all() {
                out.push(core_to_dto_info(info, true, None));
            }
        }

        Ok(out)
    }

    pub async fn get_skill(
        &self,
        agent_id: Option<&str>,
        skill_id: &str,
    ) -> Result<Option<dto::SkillDetail>> {
        if let Some(ref sr) = self.plugin_skill_registry {
            if PluginSkillRegistry::is_plugin_skill(skill_id) {
                return Ok(sr
                    .get(skill_id)
                    .map(|info| core_to_dto_detail(info, true, None)));
            }
        }

        let svc = self.workspace_service(agent_id)?;
        let Some(info) = svc.get_skill(skill_id).map_err(skill_err_to_rua)? else {
            return Ok(None);
        };
        let payload = self.read_workspace_manifest_map(agent_id).await?;
        let enabled = workspace_entry_enabled(&payload, skill_id);
        let meta = workspace_entry_install_meta(&payload, skill_id)
            .or_else(|| self.read_workspace_skill_install_meta(agent_id, skill_id));
        Ok(Some(core_to_dto_detail(info, enabled, meta)))
    }

    pub async fn create_skill(
        &self,
        agent_id: Option<&str>,
        spec: dto::SkillCreate,
    ) -> Result<dto::SkillDetail> {
        let lock = self.workspace_lock(agent_id);
        let _guard = lock.lock().await;

        let svc = self.workspace_service(agent_id)?;
        let content = build_create_skill_md(&spec);
        let config = extract_create_config(&spec);
        let enable = extract_create_enable(&spec);
        let created = svc
            .create_skill(&spec.name, &content, config, enable)
            .map_err(skill_err_to_rua)?;
        let Some(name) = created else {
            return Err(RuaError::Conflict {
                resource: "skill".to_string(),
                detail: format!("skill '{}' already exists", spec.name),
            });
        };
        let info = svc
            .get_skill(&name)
            .map_err(skill_err_to_rua)?
            .ok_or_else(|| not_found("skill", &name))?;
        let payload = self.read_workspace_manifest_map(agent_id).await?;
        let enabled = workspace_entry_enabled(&payload, &name);
        let meta = workspace_entry_install_meta(&payload, &name);
        Ok(core_to_dto_detail(info, enabled, meta))
    }

    pub async fn update_skill(
        &self,
        agent_id: Option<&str>,
        skill_id: &str,
        update: dto::SkillUpdate,
    ) -> Result<dto::SkillDetail> {
        let lock = self.workspace_lock(agent_id);
        let _guard = lock.lock().await;

        let svc = self.workspace_service(agent_id)?;
        if svc.get_skill(skill_id).map_err(skill_err_to_rua)?.is_none() {
            return Err(not_found("skill", skill_id));
        }
        if let Some(content) = update.content.as_ref() {
            match svc
                .save_skill(skill_id, content, None, None, false)
                .map_err(skill_err_to_rua)?
            {
                SaveOutcome::NotFound => return Err(not_found("skill", skill_id)),
                SaveOutcome::Conflict { suggested_name } => {
                    return Err(RuaError::Conflict {
                        resource: "skill".to_string(),
                        detail: format!("rename target taken; suggested='{suggested_name}'"),
                    });
                }
                SaveOutcome::Edit { .. }
                | SaveOutcome::Rename { .. }
                | SaveOutcome::Noop { .. } => {}
            }
        }
        if let Some(enabled) = update.enabled {
            if enabled {
                self.set_skill_enabled_via_cells(agent_id, &svc, skill_id, true)
                    .await?;
            } else {
                self.set_skill_enabled_via_cells(agent_id, &svc, skill_id, false)
                    .await?;
            }
        }
        let info = svc
            .get_skill(skill_id)
            .map_err(skill_err_to_rua)?
            .ok_or_else(|| not_found("skill", skill_id))?;
        let payload = self.read_workspace_manifest_map(agent_id).await?;
        let enabled = workspace_entry_enabled(&payload, skill_id);
        let meta = workspace_entry_install_meta(&payload, skill_id);
        Ok(core_to_dto_detail(info, enabled, meta))
    }

    pub async fn delete_skill(&self, agent_id: Option<&str>, skill_id: &str) -> Result<()> {
        let lock = self.workspace_lock(agent_id);
        let _guard = lock.lock().await;

        let svc = self.workspace_service(agent_id)?;

        let _ = svc.disable_skill(skill_id).map_err(skill_err_to_rua)?;
        let deleted = svc.delete_skill(skill_id).map_err(skill_err_to_rua)?;
        if !deleted {
            return Err(not_found("skill", skill_id));
        }
        Ok(())
    }

    pub async fn toggle_skill(
        &self,
        agent_id: Option<&str>,
        skill_id: &str,
        enabled: bool,
    ) -> Result<(dto::SkillInfo, bool)> {
        let lock = self.workspace_lock(agent_id);
        let _guard = lock.lock().await;

        let svc = self.workspace_service(agent_id)?;
        let previous = self.read_workspace_manifest_map(agent_id).await?;
        let changed = workspace_entry_enabled(&previous, skill_id) != enabled;
        let updated = self
            .set_skill_enabled_via_cells(agent_id, &svc, skill_id, enabled)
            .await?;
        if !updated {
            return Err(not_found("skill", skill_id));
        }
        let info = svc
            .get_skill(skill_id)
            .map_err(skill_err_to_rua)?
            .ok_or_else(|| not_found("skill", skill_id))?;
        let payload = self.read_workspace_manifest_map(agent_id).await?;
        let enabled_now = workspace_entry_enabled(&payload, skill_id);
        let meta = workspace_entry_install_meta(&payload, skill_id);
        Ok((core_to_dto_info(info, enabled_now, meta), changed))
    }

    pub async fn list_pool(&self) -> Result<Vec<dto::SkillPoolEntry>> {
        let pool = self.pool_service()?;
        let infos = pool.list_skills().map_err(skill_err_to_rua)?;
        let sync = pool
            .get_pool_builtin_sync_status()
            .map_err(skill_err_to_rua)?;
        let payload = read_pool_manifest_raw(pool.pool_dir());
        let pool_dir = pool.pool_dir().to_path_buf();
        let out: Vec<dto::SkillPoolEntry> = infos
            .into_iter()
            .map(|info| {
                let name = info.name.clone();
                let status = sync.get(&name).copied();
                let config = pool_entry_config(&payload, &name);
                let protected = pool_entry_protected(&payload, &name);
                let commit_text = pool_entry_commit_text(&payload, &name);
                let meta = read_install_meta(&pool_dir.join(&name))
                    .and_then(|m| serde_json::to_value(m).ok());
                core_to_pool_entry(info, status, config, protected, commit_text, meta)
            })
            .collect();
        Ok(out)
    }

    pub async fn install_from_pool(
        &self,
        _agent_id: Option<&str>,
        _req: dto::SkillPoolInstallRequest,
    ) -> Result<dto::SkillDetail> {
        Err(RuaError::NotImplemented {
            feature: "install_from_pool (legacy trait method) — use pool_download instead"
                .to_string(),
        })
    }

    pub async fn list_hub(&self) -> Result<Vec<dto::SkillHubEntry>> {
        Ok(Vec::new())
    }

    pub async fn get_hub(&self, _hub_id: &str) -> Result<Option<dto::SkillHubEntry>> {
        Ok(None)
    }

    pub async fn hub_search(
        &self,
        _query: dto::SkillHubSearchQuery,
    ) -> Result<Vec<dto::SkillHubEntry>> {
        Err(RuaError::NotImplemented {
            feature: "skills hub collection search — use GET/POST /api/market/* instead"
                .to_string(),
        })
    }

    pub async fn hub_install_start(
        &self,
        agent_id: &str,
        req: dto::SkillHubInstallStartRequest,
    ) -> Result<dto::SkillHubInstallStatus> {
        let workspace_dir = self.workspace_dir_for(Some(agent_id));
        let bundle_url = req.bundle_url.trim().to_string();
        if bundle_url.is_empty() {
            return Err(RuaError::Validation {
                message: "hub install requires a non-empty bundle_url".to_string(),
            });
        }
        let version = req.version.clone();
        let enable = req.enable;
        let target_name = {
            let trimmed = req.target_name.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        };

        let install_meta = {
            use crate::endpoints::skills_hub::build_install_meta_from_body;
            build_install_meta_from_body(
                &bundle_url,
                &version,
                req.installed_from.as_deref(),
                req.market_meta.clone(),
            )
        };

        let (task_id, cancel) = self.hub_install_registry.create(
            agent_id.to_string(),
            bundle_url.clone(),
            version.clone(),
            enable,
        );

        let registry = self.hub_install_registry.clone();
        let task_id_for_task = task_id.clone();
        tokio::spawn(async move {
            run_install_task(
                registry,
                task_id_for_task,
                cancel,
                DEFAULT_INSTALL_TIMEOUT,
                move |token| async move {
                    let installer =
                        WorkspaceBundleInstaller::new(&workspace_dir).map_err(|e| e.to_string())?;
                    let client = shared_client().await.map_err(|e| e.to_string())?;
                    let result = install_skill_from_hub(
                        client.as_ref(),
                        &installer,
                        &bundle_url,
                        &version,
                        enable,
                        target_name.as_deref(),
                        Some(token),
                        Some(install_meta),
                    )
                    .await
                    .map_err(|e| e.to_string())?;
                    let sigs = std::collections::BTreeMap::new();
                    if let Err(e) = reconcile_workspace_manifest_with(&workspace_dir, &sigs) {
                        tracing::warn!("reconcile after hub install failed: {e}");
                    }
                    Ok(json!({
                        "installed": true,
                        "name": result.name,
                        "enabled": result.enabled,
                        "source_url": result.source_url,
                    }))
                },
            )
            .await;
        });

        let snapshot = self
            .hub_install_registry
            .snapshot(&task_id)
            .ok_or_else(|| not_found("hub_install_task", &task_id))?;
        Ok(hub_task_to_dto(&snapshot))
    }

    pub async fn hub_install_status(
        &self,
        agent_id: &str,
        task_id: &str,
    ) -> Result<dto::SkillHubInstallStatus> {
        let snapshot = self
            .hub_install_registry
            .snapshot(task_id)
            .filter(|task| task.agent_id == agent_id)
            .ok_or_else(|| not_found("hub_install_task", task_id))?;
        Ok(hub_task_to_dto(&snapshot))
    }

    pub fn mark_hub_install_sync_published(&self, agent_id: &str, task_id: &str) -> bool {
        self.hub_install_registry
            .mark_sync_published(agent_id, task_id)
    }

    pub async fn hub_install_cancel(&self, agent_id: &str, task_id: &str) -> Result<()> {
        match self.hub_install_registry.cancel(agent_id, task_id) {
            Some(_) => Ok(()),
            None => Err(not_found("hub_install_task", task_id)),
        }
    }

    pub async fn refresh_skills(
        &self,
        agent_id: Option<&str>,
    ) -> Result<dto::SkillRefreshResponse> {
        let workspace_dir = self.workspace_dir(agent_id);
        std::fs::create_dir_all(&workspace_dir).map_err(|e| RuaError::Io {
            message: format!("create workspace dir: {e}"),
        })?;
        let sigs = std::collections::BTreeMap::new();
        reconcile_workspace_manifest_with(&workspace_dir, &sigs).map_err(skill_err_to_rua)?;
        let svc = self.workspace_service(agent_id)?;
        let count = svc.list_all_skills().map_err(skill_err_to_rua)?.len() as u32;
        Ok(dto::SkillRefreshResponse {
            success: true,
            count,
            extra: Map::new(),
        })
    }

    pub async fn refresh_pool(&self) -> Result<dto::SkillRefreshResponse> {
        let pool_dir = self.pool_dir();
        std::fs::create_dir_all(&pool_dir).map_err(|e| RuaError::Io {
            message: format!("create pool dir: {e}"),
        })?;
        ensure_skill_pool_initialized_in(&pool_dir).map_err(skill_err_to_rua)?;
        let sigs = std::collections::BTreeMap::new();
        reconcile_pool_manifest_with(&pool_dir, &sigs).map_err(skill_err_to_rua)?;
        let pool = self.pool_service()?;
        let count = pool.list_skills().map_err(skill_err_to_rua)?.len() as u32;
        Ok(dto::SkillRefreshResponse {
            success: true,
            count,
            extra: Map::new(),
        })
    }

    pub async fn list_workspaces(&self) -> Result<Vec<dto::WorkspaceSkillSummary>> {
        let mut out = Vec::new();
        for agent_id in self.enumerate_workspaces() {
            let svc = self.workspace_service(Some(&agent_id))?;
            let infos = svc.list_all_skills().map_err(skill_err_to_rua)?;
            let payload = self.read_workspace_manifest_map(Some(&agent_id)).await?;
            let skills: Vec<dto::WorkspaceSkillSpec> = infos
                .into_iter()
                .map(|info| {
                    let name = info.name.clone();
                    let enabled = workspace_entry_enabled(&payload, &name);
                    let channels = workspace_entry_channels(&payload, &name);
                    let config = workspace_entry_config(&payload, &name);
                    let last_updated = workspace_entry_updated_at(&payload, &name);
                    let install_meta = workspace_entry_install_meta(&payload, &name)
                        .or_else(|| self.read_workspace_skill_install_meta(Some(&agent_id), &name));
                    core_to_workspace_spec(
                        info,
                        enabled,
                        channels,
                        config,
                        last_updated,
                        install_meta,
                    )
                })
                .collect();
            out.push(dto::WorkspaceSkillSummary {
                agent_id: agent_id.clone(),
                agent_name: agent_id.clone(),
                workspace_dir: svc.workspace_dir().to_string_lossy().to_string(),
                skills,
                extra: Map::new(),
            });
        }
        Ok(out)
    }

    pub async fn pool_builtin_sources(&self) -> Result<dto::SkillPoolBuiltinSources> {
        let pool_dir = self.pool_dir();
        std::fs::create_dir_all(&pool_dir).map_err(|e| RuaError::Io {
            message: format!("create pool dir: {e}"),
        })?;
        let candidates = list_builtin_import_candidates_in(&pool_dir).map_err(skill_err_to_rua)?;
        let sources = candidates
            .into_iter()
            .map(builtin_candidate_to_dto)
            .collect();
        Ok(dto::SkillPoolBuiltinSources {
            sources,
            extra: Map::new(),
        })
    }

    pub async fn pool_builtin_notice(&self) -> Result<dto::SkillPoolBuiltinNotice> {
        let pool = self.pool_service()?;
        let notice = pool.get_pool_builtin_notice().map_err(skill_err_to_rua)?;
        Ok(dto::SkillPoolBuiltinNotice {
            has_updates: notice.has_updates,
            fingerprint: notice.fingerprint,
            total_changes: notice.total_changes as u32,
            actionable_skill_names: notice.actionable_skill_names,
            added: notice
                .added
                .into_iter()
                .map(|i| dto::BuiltinSkillInfo {
                    name: i.name,
                    version_text: i.version_text,
                    extra: Map::new(),
                })
                .collect(),
            missing: notice
                .missing
                .into_iter()
                .map(|i| dto::BuiltinSkillInfo {
                    name: i.name,
                    version_text: i.version_text,
                    extra: Map::new(),
                })
                .collect(),
            updated: notice
                .updated
                .into_iter()
                .map(|i| dto::BuiltinSkillInfo {
                    name: i.name,
                    version_text: i.version_text,
                    extra: Map::new(),
                })
                .collect(),
            removed: notice
                .removed
                .into_iter()
                .map(|i| dto::BuiltinSkillInfo {
                    name: i.name,
                    version_text: i.version_text,
                    extra: Map::new(),
                })
                .collect(),
            extra: Map::new(),
        })
    }

    pub async fn pool_update_tags(
        &self,
        skill_name: &str,
        req: dto::SkillPoolTagsRequest,
    ) -> Result<()> {
        let pool = self.pool_service()?;
        pool.update_pool_skill_tags(skill_name, req.tags)
            .map_err(skill_err_to_rua)
    }

    pub async fn pool_create(
        &self,
        req: dto::SkillPoolCreateRequest,
    ) -> Result<dto::SkillPoolEntry> {
        let pool = self.pool_service()?;
        let content = if req.content.is_empty() {
            let description = if req.description.is_empty() {
                req.name.clone()
            } else {
                req.description.clone()
            };
            format!(
                "---\nname: {}\ndescription: {}\n---\n",
                req.name, description
            )
        } else {
            req.content.clone()
        };
        let created = pool
            .create_skill(
                &req.name,
                &content,
                None,
                rua_skills::SkillSource::Customized,
            )
            .map_err(skill_err_to_rua)?;
        let Some(name) = created else {
            return Err(RuaError::Conflict {
                resource: "pool_skill".to_string(),
                detail: format!("pool skill '{}' already exists", req.name),
            });
        };
        let info = pool
            .get_skill(&name)
            .map_err(skill_err_to_rua)?
            .ok_or_else(|| not_found("pool_skill", &name))?;
        let sync = pool
            .get_pool_builtin_sync_status()
            .map_err(skill_err_to_rua)?;
        let payload = read_pool_manifest_raw(pool.pool_dir());
        let status = sync.get(&name).copied();
        let config = pool_entry_config(&payload, &name);
        let protected = pool_entry_protected(&payload, &name);
        let commit_text = pool_entry_commit_text(&payload, &name);
        let meta = read_install_meta(&pool.pool_dir().join(&name))
            .and_then(|m| serde_json::to_value(m).ok());
        Ok(core_to_pool_entry(
            info,
            status,
            config,
            protected,
            commit_text,
            meta,
        ))
    }

    pub async fn pool_save(&self, req: dto::SkillPoolSaveRequest) -> Result<dto::SkillPoolEntry> {
        let pool = self.pool_service()?;
        if pool
            .get_skill(&req.name)
            .map_err(skill_err_to_rua)?
            .is_some()
        {
            return Err(RuaError::NotImplemented {
                feature: "pool_save edit-in-place".to_string(),
            });
        }
        let create_req = dto::SkillPoolCreateRequest {
            name: req.name,
            content: req.content,
            description: req.description,
            extra: req.extra,
        };
        self.pool_create(create_req).await
    }

    pub async fn pool_upload_zip(
        &self,
        data: bytes::Bytes,
        options: ImportOptions,
    ) -> Result<dto::SkillUploadResponse> {
        let staging = tempfile::tempdir().map_err(|error| RuaError::Io {
            message: format!("create pool import staging dir: {error}"),
        })?;
        let staged_service = CoreSkillService::new(staging.path()).map_err(skill_err_to_rua)?;
        let staged = staged_service
            .import_from_zip_with_options(
                &data,
                ImportOptions {
                    enable: false,
                    overwrite: false,
                    ..options.clone()
                },
            )
            .map_err(skill_err_to_rua)?;
        if !staged.conflicts.is_empty() {
            return Ok(dto::SkillUploadResponse {
                imported: Vec::new(),
                count: 0,
                enabled: false,
                conflicts: staged
                    .conflicts
                    .into_iter()
                    .map(|conflict| dto::SkillUploadConflict {
                        skill_name: conflict.skill_name,
                        suggested_name: conflict.suggested_name,
                        reason: "duplicate skill name in archive".to_string(),
                    })
                    .collect(),
            });
        }

        let pool = self.pool_service()?;
        let existing_names: Vec<String> = pool
            .list_skills()
            .map_err(skill_err_to_rua)?
            .into_iter()
            .map(|skill| skill.name)
            .collect();
        if !options.overwrite {
            let conflicts: Vec<_> = staged
                .imported
                .iter()
                .filter(|name| existing_names.contains(name))
                .map(|name| dto::SkillUploadConflict {
                    skill_name: name.clone(),
                    suggested_name: rua_skills::suggest_pool_conflict_name(name, &existing_names),
                    reason: "skill already exists in pool".to_string(),
                })
                .collect();
            if !conflicts.is_empty() {
                return Ok(dto::SkillUploadResponse {
                    imported: Vec::new(),
                    count: 0,
                    enabled: false,
                    conflicts,
                });
            }
        }

        let mut imported = Vec::new();
        for name in staged.imported {
            let result = pool
                .upload_from_workspace(staging.path(), &name, Some(&name), options.overwrite)
                .map_err(skill_err_to_rua)?;
            imported.push(result.name);
        }
        Ok(dto::SkillUploadResponse {
            count: imported.len(),
            imported,
            enabled: false,
            conflicts: Vec::new(),
        })
    }

    pub async fn pool_import(
        &self,
        req: dto::SkillPoolImportRequest,
    ) -> Result<dto::SuccessResponse> {
        let bundle_url = req
            .url
            .as_deref()
            .or_else(|| req.extra.get("bundle_url").and_then(Value::as_str))
            .or_else(|| req.extra.get("source_url").and_then(Value::as_str))
            .unwrap_or_default()
            .trim()
            .to_string();
        if bundle_url.is_empty() {
            return Err(RuaError::Validation {
                message: "pool import requires a non-empty bundle_url".to_string(),
            });
        }

        let version = req
            .extra
            .get("version")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let target_name = req
            .extra
            .get("target_name")
            .and_then(Value::as_str)
            .or(req.name.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let installed_from = req
            .extra
            .get("installed_from")
            .and_then(Value::as_str)
            .or(Some("hub"));
        let market_meta = req.extra.get("market_meta").cloned();

        std::fs::create_dir_all(self.pool_dir()).map_err(|e| RuaError::Io {
            message: format!("create pool dir: {e}"),
        })?;
        let installer = PoolBundleInstaller::new(self.pool_dir());
        let client = shared_client().await.map_err(hub_err_to_rua)?;
        let install_meta = {
            use crate::endpoints::skills_hub::build_install_meta_from_body;
            build_install_meta_from_body(&bundle_url, &version, installed_from, market_meta)
        };
        let result = install_skill_to_pool_from_hub(
            client.as_ref(),
            &installer,
            &bundle_url,
            &version,
            target_name,
            None,
            Some(install_meta),
        )
        .await
        .map_err(hub_err_to_rua)?;

        let sigs = std::collections::BTreeMap::new();
        reconcile_pool_manifest_with(&self.pool_dir(), &sigs).map_err(skill_err_to_rua)?;

        let mut extra = Map::new();
        extra.insert("installed".to_string(), Value::Bool(true));
        extra.insert("name".to_string(), Value::String(result.name));
        extra.insert("enabled".to_string(), Value::Bool(result.enabled));
        extra.insert("source_url".to_string(), Value::String(result.source_url));
        Ok(dto::SuccessResponse {
            success: true,
            message: None,
            extra,
        })
    }

    pub async fn pool_upload(&self, data: bytes::Bytes) -> Result<dto::SuccessResponse> {
        #[derive(serde::Deserialize)]
        struct Body {
            workspace_id: String,
            skill_name: String,
            #[serde(default)]
            target_name: Option<String>,
            #[serde(default)]
            overwrite: bool,
            #[serde(default)]
            preview_only: bool,
        }
        let body: Body = serde_json::from_slice(&data).map_err(|e| RuaError::Validation {
            message: format!("pool_upload body: {e}"),
        })?;
        if body.preview_only {
            let workspace_dir = self.workspace_dir(Some(&body.workspace_id));
            let exists = workspace_dir.join("skills").join(&body.skill_name).exists();
            return Ok(dto::SuccessResponse {
                success: exists,
                message: None,
                extra: {
                    let mut m = Map::new();
                    m.insert("preview_only".to_string(), Value::Bool(true));
                    m.insert("exists".to_string(), Value::Bool(exists));
                    m
                },
            });
        }
        let pool = self.pool_service()?;
        let workspace_dir = self.workspace_dir(Some(&body.workspace_id));
        let result = pool
            .upload_from_workspace(
                &workspace_dir,
                &body.skill_name,
                body.target_name.as_deref(),
                body.overwrite,
            )
            .map_err(skill_err_to_rua)?;
        Ok(dto::SuccessResponse {
            success: true,
            message: None,
            extra: {
                let mut m = Map::new();
                m.insert("name".to_string(), Value::String(result.name));
                m.insert("signature".to_string(), Value::String(result.signature));
                m.insert(
                    "source".to_string(),
                    Value::String(result.source.as_str().to_string()),
                );
                m
            },
        })
    }

    pub async fn pool_download(&self, req: dto::SkillPoolDownloadRequest) -> Result<Vec<u8>> {
        let extra = &req.extra;
        let overwrite = extra
            .get("overwrite")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let preview_only = extra
            .get("preview_only")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let target_name = extra
            .get("target_name")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let all_workspaces = extra
            .get("all_workspaces")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let mut targets: Vec<String> = extra
            .get("targets")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| {
                        v.as_object()
                            .and_then(|o| o.get("workspace_id"))
                            .and_then(|w| w.as_str())
                            .map(|s| s.to_string())
                    })
                    .collect()
            })
            .unwrap_or_default();
        if all_workspaces {
            targets = self.enumerate_workspaces();
        }
        if targets.is_empty() {
            return Err(RuaError::Validation {
                message: "pool_download: no workspace targets provided".to_string(),
            });
        }
        if preview_only {
            let body = json!({"downloaded": []});
            return Ok(serde_json::to_vec(&body).unwrap_or_default());
        }
        let pool = self.pool_service()?;
        let mut downloaded = Vec::new();
        for workspace_id in targets {
            let workspace_dir = self.workspace_dir(Some(&workspace_id));
            let result = pool
                .download_to_workspace(
                    &workspace_dir,
                    &req.skill_name,
                    target_name.as_deref(),
                    overwrite,
                )
                .map_err(skill_err_to_rua)?;
            downloaded.push(json!({
                "workspace_id": workspace_id,
                "name": result.name,
                "signature": result.signature,
            }));
        }
        let body = json!({"downloaded": downloaded});
        Ok(serde_json::to_vec(&body).unwrap_or_default())
    }

    pub async fn pool_import_builtin(
        &self,
        req: dto::SkillPoolImportBuiltinRequest,
    ) -> Result<dto::SuccessResponse> {
        let pool_dir = self.pool_dir();
        std::fs::create_dir_all(&pool_dir).map_err(|e| RuaError::Io {
            message: format!("create pool dir: {e}"),
        })?;
        let names: Vec<String> = if req.skill_names.is_empty() {
            builtin_skill_names()
        } else {
            req.skill_names.clone()
        };
        let report = import_builtin_skills_in(&pool_dir, Some(&names), req.overwrite_conflicts)
            .map_err(skill_err_to_rua)?;
        let imported_value = serde_json::to_value(&report.imported).unwrap_or(Value::Null);
        let conflicts_value = serde_json::to_value(&report.conflicts).unwrap_or(Value::Null);
        let mut extra = Map::new();
        extra.insert("imported".to_string(), imported_value);
        extra.insert("conflicts".to_string(), conflicts_value);
        Ok(dto::SuccessResponse {
            success: report.conflicts.is_empty(),
            message: None,
            extra,
        })
    }

    pub async fn pool_update_builtin(&self, skill_name: &str) -> Result<dto::SuccessResponse> {
        let pool_dir = self.pool_dir();
        std::fs::create_dir_all(&pool_dir).map_err(|e| RuaError::Io {
            message: format!("create pool dir: {e}"),
        })?;
        let candidate =
            update_single_builtin_in(&pool_dir, skill_name).map_err(skill_err_to_rua)?;
        let mut extra = Map::new();
        extra.insert(
            "candidate".to_string(),
            serde_json::to_value(builtin_candidate_to_dto(candidate)).unwrap_or(Value::Null),
        );
        Ok(dto::SuccessResponse {
            success: true,
            message: None,
            extra,
        })
    }

    pub async fn pool_delete(&self, skill_name: &str) -> Result<()> {
        let pool = self.pool_service()?;
        let deleted = pool.delete_skill(skill_name).map_err(skill_err_to_rua)?;
        if !deleted {
            return Err(not_found("pool_skill", skill_name));
        }
        Ok(())
    }

    pub async fn pool_get_config(&self, skill_name: &str) -> Result<dto::SkillPoolConfig> {
        let pool = self.pool_service()?;
        if pool
            .get_skill(skill_name)
            .map_err(skill_err_to_rua)?
            .is_none()
        {
            return Err(not_found("pool_skill", skill_name));
        }
        let payload = read_pool_manifest_raw(pool.pool_dir());
        let config = pool_entry_config(&payload, skill_name).unwrap_or(Value::Object(Map::new()));
        let mut inner = Map::new();
        inner.insert("config".to_string(), config);
        Ok(dto::SkillPoolConfig { inner })
    }

    pub async fn pool_update_config(
        &self,
        skill_name: &str,
        config: dto::SkillPoolConfig,
    ) -> Result<dto::SkillPoolConfig> {
        let pool = self.pool_service()?;
        if pool
            .get_skill(skill_name)
            .map_err(skill_err_to_rua)?
            .is_none()
        {
            return Err(not_found("pool_skill", skill_name));
        }
        let new_config = extract_pool_config_from_dto(config);
        write_pool_skill_field(
            pool.pool_dir(),
            skill_name,
            "config",
            Value::Object(new_config),
        )?;
        self.pool_get_config(skill_name).await
    }

    pub async fn pool_delete_config(&self, skill_name: &str) -> Result<()> {
        let pool = self.pool_service()?;
        if pool
            .get_skill(skill_name)
            .map_err(skill_err_to_rua)?
            .is_none()
        {
            return Err(not_found("pool_skill", skill_name));
        }
        clear_pool_skill_field(pool.pool_dir(), skill_name, "config")
    }

    pub async fn pool_batch_delete(&self, req: dto::BatchDeleteRequest) -> Result<()> {
        let pool = self.pool_service()?;
        for name in &req.names {
            let _ = pool.delete_skill(name).map_err(skill_err_to_rua)?;
        }
        Ok(())
    }

    pub async fn upload_skill(
        &self,
        agent_id: Option<&str>,
        data: bytes::Bytes,
        options: ImportOptions,
    ) -> Result<dto::SkillUploadResponse> {
        let lock = self.workspace_lock(agent_id);
        let _guard = lock.lock().await;

        let svc = self.workspace_service(agent_id)?;
        let outcome: ImportOutcome = svc
            .import_from_zip_with_options(&data, options)
            .map_err(skill_err_to_rua)?;
        let conflicts: Vec<dto::SkillUploadConflict> = outcome
            .conflicts
            .iter()
            .map(|c| dto::SkillUploadConflict {
                skill_name: c.skill_name.clone(),
                suggested_name: c.suggested_name.clone(),
                reason: "skill already exists in workspace".to_string(),
            })
            .collect();
        Ok(dto::SkillUploadResponse {
            count: outcome.imported.len(),
            imported: outcome.imported,
            enabled: outcome.enabled,
            conflicts,
        })
    }

    pub async fn batch_delete(
        &self,
        agent_id: Option<&str>,
        req: dto::BatchDeleteRequest,
    ) -> Result<()> {
        for name in &req.names {
            match self.delete_skill(agent_id, name).await {
                Ok(()) | Err(RuaError::NotFound { .. }) => continue,
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    pub async fn batch_disable(
        &self,
        agent_id: Option<&str>,
        req: dto::BatchToggleRequest,
    ) -> Result<()> {
        for name in &req.names {
            match self.toggle_skill(agent_id, name, false).await {
                Ok(_) | Err(RuaError::NotFound { .. }) => continue,
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    pub async fn batch_enable(
        &self,
        agent_id: Option<&str>,
        req: dto::BatchToggleRequest,
    ) -> Result<()> {
        for name in &req.names {
            match self.toggle_skill(agent_id, name, true).await {
                Ok(_) | Err(RuaError::NotFound { .. }) => continue,
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    pub async fn get_skill_file(
        &self,
        agent_id: Option<&str>,
        skill_name: &str,
        file_path: &str,
    ) -> Result<Option<dto::FileContent>> {
        let normalized = file_path.replace('\\', "/");
        if normalized.contains("..") || normalized.starts_with('/') {
            return Ok(None);
        }
        if !normalized.starts_with("references/") && !normalized.starts_with("scripts/") {
            return Ok(None);
        }
        let svc = self.workspace_service(agent_id)?;
        if svc
            .get_skill(skill_name)
            .map_err(skill_err_to_rua)?
            .is_none()
        {
            return Ok(None);
        }
        let base = svc.workspace_dir().join("skills").join(skill_name);
        if !base.exists() {
            return Ok(None);
        }
        let full = base.join(&normalized);
        let resolved = full.canonicalize().unwrap_or_else(|_| full.clone());
        let base_resolved = base.canonicalize().unwrap_or_else(|_| base.clone());
        if !resolved.starts_with(&base_resolved) {
            return Ok(None);
        }
        if !full.exists() || !full.is_file() {
            return Ok(None);
        }
        let content = std::fs::read_to_string(&full).map_err(|e| RuaError::Io {
            message: format!("read {}: {e}", full.display()),
        })?;
        let size = content.len() as u64;
        let name = full
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        Ok(Some(dto::FileContent {
            name,
            path: normalized,
            content,
            size,
            extra: Map::new(),
        }))
    }

    pub async fn save_skill(
        &self,
        agent_id: Option<&str>,
        req: dto::SkillSaveRequest,
    ) -> Result<(dto::SkillDetail, bool)> {
        let lock = self.workspace_lock(agent_id);
        let _guard = lock.lock().await;

        let svc = self.workspace_service(agent_id)?;
        let source_name = req
            .source_name
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or(&req.name);
        let target_name = if req
            .source_name
            .as_deref()
            .map(|s| !s.is_empty())
            .unwrap_or(false)
        {
            Some(req.name.as_str())
        } else {
            None
        };
        let config = extract_save_config(&req);
        let overwrite = req.overwrite.unwrap_or(false);
        let outcome = svc
            .save_skill(source_name, &req.content, target_name, config, overwrite)
            .map_err(skill_err_to_rua)?;
        let changed = !matches!(outcome, SaveOutcome::Noop { .. });
        let final_name = match outcome {
            SaveOutcome::NotFound => return Err(not_found("skill", source_name)),
            SaveOutcome::Conflict { suggested_name } => {
                return Err(RuaError::Conflict {
                    resource: "skill".to_string(),
                    detail: format!("rename target taken; suggested='{suggested_name}'"),
                });
            }
            SaveOutcome::Edit { name } => name,
            SaveOutcome::Rename { name } => name,
            SaveOutcome::Noop { name } => name,
        };
        let info = svc
            .get_skill(&final_name)
            .map_err(skill_err_to_rua)?
            .ok_or_else(|| not_found("skill", &final_name))?;
        let payload = self.read_workspace_manifest_map(agent_id).await?;
        let enabled = workspace_entry_enabled(&payload, &final_name);
        let meta = workspace_entry_install_meta(&payload, &final_name);
        Ok((core_to_dto_detail(info, enabled, meta), changed))
    }

    pub async fn update_skill_channels(
        &self,
        agent_id: Option<&str>,
        skill_name: &str,
        req: dto::SkillChannelsRequest,
    ) -> Result<()> {
        let lock = self.workspace_lock(agent_id);
        let _guard = lock.lock().await;

        let svc = self.workspace_service(agent_id)?;
        if !self
            .set_skill_channels_via_cells(agent_id, &svc, skill_name, req.channels)
            .await?
        {
            return Err(not_found("skill", skill_name));
        }
        Ok(())
    }

    pub async fn get_skill_config(
        &self,
        agent_id: Option<&str>,
        skill_name: &str,
    ) -> Result<dto::SkillConfig> {
        let svc = self.workspace_service(agent_id)?;
        if svc
            .get_skill(skill_name)
            .map_err(skill_err_to_rua)?
            .is_none()
        {
            return Err(not_found("skill", skill_name));
        }
        let payload = self.read_workspace_manifest_map(agent_id).await?;
        let config =
            workspace_entry_config(&payload, skill_name).unwrap_or(Value::Object(Map::new()));
        let mut inner = Map::new();
        inner.insert("config".to_string(), config);
        Ok(dto::SkillConfig { inner })
    }

    pub async fn update_skill_config(
        &self,
        agent_id: Option<&str>,
        skill_name: &str,
        config: dto::SkillConfig,
    ) -> Result<dto::SkillConfig> {
        let lock = self.workspace_lock(agent_id);
        let _guard = lock.lock().await;

        let svc = self.workspace_service(agent_id)?;
        let new_config = extract_config_from_dto(config);
        if !self
            .set_skill_config_via_cells(agent_id, &svc, skill_name, new_config)
            .await?
        {
            return Err(not_found("skill", skill_name));
        }
        self.get_skill_config(agent_id, skill_name).await
    }

    pub async fn delete_skill_config(
        &self,
        agent_id: Option<&str>,
        skill_name: &str,
    ) -> Result<()> {
        let lock = self.workspace_lock(agent_id);
        let _guard = lock.lock().await;

        let svc = self.workspace_service(agent_id)?;
        if !self
            .set_skill_config_via_cells(agent_id, &svc, skill_name, Map::new())
            .await?
        {
            return Err(not_found("skill", skill_name));
        }
        Ok(())
    }

    pub fn ai_optimize_stream(
        &self,
        _req: dto::SkillOptimizeRequest,
    ) -> Pin<
        Box<
            dyn futures::Stream<Item = std::result::Result<dto::SkillOptimizeEvent, String>>
                + Send
                + 'static,
        >,
    > {
        Box::pin(futures::stream::once(async {
            Err("ai_optimize_stream not implemented".to_string())
        }))
    }
}

fn write_pool_skill_field(
    pool_dir: &std::path::Path,
    skill_name: &str,
    field: &str,
    value: Value,
) -> Result<()> {
    use rua_skills::lock::{lock_path_for, ManifestLock};
    use rua_skills::write_manifest_atomic;

    let manifest_path = pool_dir.join("skill.json");
    if let Some(parent) = manifest_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| RuaError::Io {
            message: format!("create pool manifest parent: {e}"),
        })?;
    }
    let _lock =
        ManifestLock::acquire(&lock_path_for(&manifest_path)).map_err(|e| RuaError::Io {
            message: format!("manifest lock: {e}"),
        })?;
    let mut payload = read_pool_manifest_raw(pool_dir);
    let entries = payload
        .entry("skills".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    let entry = entries
        .as_object_mut()
        .and_then(|m| m.get_mut(skill_name))
        .and_then(|v| v.as_object_mut())
        .ok_or_else(|| not_found("pool_skill", skill_name))?;
    entry.insert(field.to_string(), value);
    write_manifest_atomic(&manifest_path, &mut payload).map_err(skill_err_to_rua)?;
    Ok(())
}

fn clear_pool_skill_field(pool_dir: &std::path::Path, skill_name: &str, field: &str) -> Result<()> {
    use rua_skills::lock::{lock_path_for, ManifestLock};
    use rua_skills::write_manifest_atomic;

    let manifest_path = pool_dir.join("skill.json");
    if let Some(parent) = manifest_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| RuaError::Io {
            message: format!("create pool manifest parent: {e}"),
        })?;
    }
    let _lock =
        ManifestLock::acquire(&lock_path_for(&manifest_path)).map_err(|e| RuaError::Io {
            message: format!("manifest lock: {e}"),
        })?;
    let mut payload = read_pool_manifest_raw(pool_dir);
    let entries = payload
        .entry("skills".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    let entry = entries
        .as_object_mut()
        .and_then(|m| m.get_mut(skill_name))
        .and_then(|v| v.as_object_mut())
        .ok_or_else(|| not_found("pool_skill", skill_name))?;
    entry.remove(field);
    write_manifest_atomic(&manifest_path, &mut payload).map_err(skill_err_to_rua)?;
    Ok(())
}
