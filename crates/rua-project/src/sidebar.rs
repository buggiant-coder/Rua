use crate::{ProjectError, ProjectResult};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::Mutex;

// Adapters and HTTP handlers may open the same file independently. Reload under
// this process-wide lock before every mutation so they never overwrite a stale cache.
static SIDEBAR_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectSidebarSort {
    Manual,
    Name,
    CreatedAt,
    #[default]
    LastActivityAt,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectSidebarItem {
    pub project_id: String,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub hidden: bool,
    #[serde(default)]
    pub section_id: Option<String>,
    #[serde(default)]
    pub order: i64,
    #[serde(default)]
    pub pin_order: i64,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectSidebarSection {
    pub section_id: String,
    pub name: String,
    pub order: i64,
    #[serde(default)]
    pub deleted: bool,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProjectSidebarValue {
    Section {
        section: ProjectSidebarSection,
    },
    Project {
        item: ProjectSidebarItem,
    },
    Settings {
        sort: ProjectSidebarSort,
        #[serde(default, flatten)]
        extra: BTreeMap<String, serde_json::Value>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectSidebarRecord {
    pub id: String,
    pub version: String,
    pub updated_at_ms: i64,
    pub value: ProjectSidebarValue,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectSidebarSnapshot {
    pub revision: String,
    pub sort: ProjectSidebarSort,
    pub items: BTreeMap<String, ProjectSidebarItem>,
    pub sections: BTreeMap<String, ProjectSidebarSection>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ProjectSidebarAction {
    CreateSection {
        name: String,
    },
    RenameSection {
        section_id: String,
        name: String,
    },
    DeleteSection {
        section_id: String,
    },
    ReorderSection {
        section_id: String,
        ordered_section_ids: Vec<String>,
    },
    MoveToSection {
        project_id: String,
        section_id: Option<String>,
    },
    SetHidden {
        project_id: String,
        hidden: bool,
    },
    SetSort {
        sort: ProjectSidebarSort,
    },
    SetPinned {
        project_id: String,
        pinned: bool,
    },
    Reorder {
        project_id: String,
        ordered_project_ids: Vec<String>,
    },
}

#[derive(Debug, Serialize, Deserialize)]
struct SidebarDocument {
    schema_version: u32,
    revision: String,
    records: BTreeMap<String, ProjectSidebarRecord>,
    #[serde(default, flatten)]
    extra: BTreeMap<String, serde_json::Value>,
}
impl Default for SidebarDocument {
    fn default() -> Self {
        Self {
            schema_version: 1,
            revision: "empty".into(),
            records: BTreeMap::new(),
            extra: BTreeMap::new(),
        }
    }
}
impl SidebarDocument {
    fn snapshot(&self) -> ProjectSidebarSnapshot {
        let mut snapshot = ProjectSidebarSnapshot {
            revision: self.revision.clone(),
            sort: ProjectSidebarSort::default(),
            items: BTreeMap::new(),
            sections: BTreeMap::new(),
        };
        for record in self.records.values() {
            match &record.value {
                ProjectSidebarValue::Section { section } => {
                    if !section.deleted {
                        snapshot
                            .sections
                            .insert(section.section_id.clone(), section.clone());
                    }
                }
                ProjectSidebarValue::Project { item } => {
                    snapshot.items.insert(item.project_id.clone(), item.clone());
                }
                ProjectSidebarValue::Settings { sort, .. } => snapshot.sort = *sort,
            }
        }
        snapshot
    }
    fn put(&mut self, id: String, value: ProjectSidebarValue) {
        if self.records.get(&id).is_some_and(|old| old.value == value) {
            return;
        }
        let extra = self
            .records
            .get(&id)
            .map(|old| old.extra.clone())
            .unwrap_or_default();
        self.records.insert(
            id.clone(),
            ProjectSidebarRecord {
                id,
                version: uuid::Uuid::new_v4().to_string(),
                updated_at_ms: chrono::Utc::now().timestamp_millis(),
                value,
                extra,
            },
        );
    }
}

pub struct ProjectSidebarStore {
    path: PathBuf,
}
impl ProjectSidebarStore {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            path: data_dir.into().join("sidebar/project-preferences.json"),
        }
    }
    fn load(&self) -> ProjectResult<SidebarDocument> {
        match std::fs::read(&self.path) {
            Ok(bytes) => {
                let doc: SidebarDocument = serde_json::from_slice(&bytes)?;
                if doc.schema_version != 1 {
                    return Err(ProjectError::InvalidResources(
                        "Unsupported sidebar schema".into(),
                    ));
                }
                for (id, record) in &doc.records {
                    validate_record(record)?;
                    if id != &record.id {
                        return Err(ProjectError::InvalidResources(
                            "Sidebar record key mismatch".into(),
                        ));
                    }
                }
                Ok(doc)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(SidebarDocument::default())
            }
            Err(error) => Err(error.into()),
        }
    }
    fn save(&self, doc: &mut SidebarDocument) -> ProjectResult<()> {
        doc.revision = uuid::Uuid::new_v4().to_string();
        std::fs::create_dir_all(self.path.parent().expect("sidebar parent"))?;
        rua_config::atomic_write(&self.path, &serde_json::to_vec_pretty(doc)?)?;
        Ok(())
    }
    pub fn snapshot(&self) -> ProjectResult<ProjectSidebarSnapshot> {
        let _guard = SIDEBAR_LOCK
            .lock()
            .map_err(|_| ProjectError::LockPoisoned)?;
        Ok(self.load()?.snapshot())
    }
    pub fn records(&self) -> ProjectResult<Vec<ProjectSidebarRecord>> {
        let _guard = SIDEBAR_LOCK
            .lock()
            .map_err(|_| ProjectError::LockPoisoned)?;
        Ok(self.load()?.records.into_values().collect())
    }
    pub fn apply_record(&self, record: ProjectSidebarRecord) -> ProjectResult<()> {
        validate_record(&record)?;
        let _guard = SIDEBAR_LOCK
            .lock()
            .map_err(|_| ProjectError::LockPoisoned)?;
        let mut doc = self.load()?;
        if doc.records.get(&record.id) == Some(&record) {
            return Ok(());
        }
        if doc
            .records
            .get(&record.id)
            .is_some_and(ProjectSidebarRecord::is_tombstone)
            && !record.is_tombstone()
        {
            return Ok(());
        }
        doc.records.insert(record.id.clone(), record);
        self.save(&mut doc)
    }
    pub fn update(
        &self,
        expected_revision: &str,
        action: ProjectSidebarAction,
        project_ids: &[String],
    ) -> ProjectResult<(ProjectSidebarSnapshot, Vec<ProjectSidebarRecord>)> {
        let _guard = SIDEBAR_LOCK
            .lock()
            .map_err(|_| ProjectError::LockPoisoned)?;
        let mut doc = self.load()?;
        if doc.revision != expected_revision {
            return Err(ProjectError::InvalidResources(
                "Sidebar changed; reload before editing".into(),
            ));
        }
        let before = doc.records.clone();
        let snapshot = doc.snapshot();
        let valid = |id: &str| -> ProjectResult<()> {
            if project_ids.iter().any(|value| value == id) {
                Ok(())
            } else {
                Err(ProjectError::InvalidResources(format!(
                    "Project not found: {id}"
                )))
            }
        };
        let item_for = |id: &str| {
            snapshot
                .items
                .get(id)
                .cloned()
                .unwrap_or_else(|| ProjectSidebarItem {
                    project_id: id.into(),
                    order: next_rank(snapshot.items.values().map(|item| item.order)),
                    ..Default::default()
                })
        };
        let section_for =
            |id: &str| -> ProjectResult<ProjectSidebarSection> {
                snapshot.sections.get(id).cloned().ok_or_else(|| {
                    ProjectError::InvalidResources("Section no longer exists".into())
                })
            };
        let section_name = |name: &str, except: Option<&str>| -> ProjectResult<String> {
            let name = name.trim();
            if name.is_empty()
                || snapshot.sections.values().any(|section| {
                    Some(section.section_id.as_str()) != except
                        && section.name.to_lowercase() == name.to_lowercase()
                })
            {
                return Err(ProjectError::InvalidResources(
                    "Choose a nonempty, unique section name".into(),
                ));
            }
            Ok(name.to_string())
        };
        match action {
            ProjectSidebarAction::CreateSection { name } => {
                let section = ProjectSidebarSection {
                    section_id: uuid::Uuid::new_v4().to_string(),
                    name: section_name(&name, None)?,
                    order: next_rank(snapshot.sections.values().map(|section| section.order)),
                    deleted: false,
                    extra: Default::default(),
                };
                doc.put(
                    format!("section:{}", section.section_id),
                    ProjectSidebarValue::Section { section },
                );
            }
            ProjectSidebarAction::RenameSection { section_id, name } => {
                let mut section = section_for(&section_id)?;
                section.name = section_name(&name, Some(&section_id))?;
                doc.put(
                    format!("section:{section_id}"),
                    ProjectSidebarValue::Section { section },
                );
            }
            ProjectSidebarAction::DeleteSection { section_id } => {
                let mut section = section_for(&section_id)?;
                section.deleted = true;
                doc.put(
                    format!("section:{section_id}"),
                    ProjectSidebarValue::Section { section },
                );
            }
            ProjectSidebarAction::ReorderSection {
                section_id,
                ordered_section_ids,
            } => {
                section_for(&section_id)?;
                let mut seen = HashSet::new();
                for id in &ordered_section_ids {
                    section_for(id)?;
                    if !seen.insert(id) {
                        return Err(ProjectError::InvalidResources(
                            "Duplicate section in order".into(),
                        ));
                    }
                }
                let at = ordered_section_ids
                    .iter()
                    .position(|id| id == &section_id)
                    .ok_or_else(|| {
                        ProjectError::InvalidResources("Moved section missing from order".into())
                    })?;
                let lower = at
                    .checked_sub(1)
                    .map(|index| snapshot.sections[&ordered_section_ids[index]].order);
                let upper = ordered_section_ids
                    .get(at + 1)
                    .map(|id| snapshot.sections[id].order);
                let rank = between_ranks(lower, upper);
                for (index, id) in ordered_section_ids.iter().enumerate() {
                    if rank.is_some() && id != &section_id {
                        continue;
                    }
                    let mut section = section_for(id)?;
                    section.order = rank.unwrap_or(index as i64 * 1024);
                    doc.put(
                        format!("section:{id}"),
                        ProjectSidebarValue::Section { section },
                    );
                }
            }
            ProjectSidebarAction::MoveToSection {
                project_id,
                section_id,
            } => {
                valid(&project_id)?;
                if let Some(id) = section_id.as_deref() {
                    section_for(id)?;
                }
                let mut item = item_for(&project_id);
                item.section_id = section_id;
                item.pinned = false;
                item.order = next_rank(
                    snapshot
                        .items
                        .values()
                        .filter(|other| other.section_id == item.section_id)
                        .map(|other| other.order),
                );
                doc.put(
                    format!("project:{project_id}"),
                    ProjectSidebarValue::Project { item },
                );
            }
            ProjectSidebarAction::SetHidden { project_id, hidden } => {
                valid(&project_id)?;
                let mut item = item_for(&project_id);
                item.hidden = hidden;
                doc.put(
                    format!("project:{project_id}"),
                    ProjectSidebarValue::Project { item },
                );
            }
            ProjectSidebarAction::SetSort { sort } => {
                doc.put("settings".into(), settings_value(&doc, sort))
            }
            ProjectSidebarAction::SetPinned { project_id, pinned } => {
                valid(&project_id)?;
                let mut item = item_for(&project_id);
                if pinned && !item.pinned {
                    item.pin_order = next_rank(
                        snapshot
                            .items
                            .values()
                            .filter(|item| item.pinned)
                            .map(|item| item.pin_order),
                    );
                }
                item.pinned = pinned;
                doc.put(
                    format!("project:{project_id}"),
                    ProjectSidebarValue::Project { item },
                );
            }
            ProjectSidebarAction::Reorder {
                project_id,
                ordered_project_ids,
            } => {
                valid(&project_id)?;
                let mut seen = HashSet::new();
                let selected = item_for(&project_id);
                for id in &ordered_project_ids {
                    valid(id)?;
                    if !seen.insert(id) {
                        return Err(ProjectError::InvalidResources(
                            "Reorder must contain unique Projects".into(),
                        ));
                    }
                }
                let area_ids = ordered_project_ids
                    .iter()
                    .filter(|id| {
                        let item = item_for(id);
                        item.pinned == selected.pinned
                            && (selected.pinned
                                || effective_section(&item, &snapshot.sections)
                                    == effective_section(&selected, &snapshot.sections))
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                let at = area_ids
                    .iter()
                    .position(|id| id == &project_id)
                    .ok_or_else(|| {
                        ProjectError::InvalidResources("Moved Project is missing from order".into())
                    })?;
                let rank = |id: &str| {
                    let item = item_for(id);
                    if selected.pinned {
                        item.pin_order
                    } else {
                        item.order
                    }
                };
                let lower = at.checked_sub(1).map(|index| rank(&area_ids[index]));
                let upper = area_ids.get(at + 1).map(|id| rank(id));
                let new_rank = between_ranks(lower, upper);
                let snapshot_order = snapshot.sort != ProjectSidebarSort::Manual
                    || ordered_project_ids
                        .iter()
                        .any(|id| !snapshot.items.contains_key(id))
                    || new_rank.is_none();
                for (index, id) in ordered_project_ids.iter().enumerate() {
                    if !snapshot_order && id != &project_id {
                        continue;
                    }
                    let mut item = item_for(id);
                    let position = if snapshot_order {
                        (index as i64) * 1024
                    } else {
                        new_rank.expect("checked rank")
                    };
                    if item.pinned {
                        item.pin_order = position;
                    } else {
                        item.order = position;
                    }
                    doc.put(
                        format!("project:{id}"),
                        ProjectSidebarValue::Project { item },
                    );
                }
                doc.put(
                    "settings".into(),
                    settings_value(&doc, ProjectSidebarSort::Manual),
                );
            }
        }
        let changes = doc
            .records
            .values()
            .filter(|record| before.get(&record.id) != Some(record))
            .cloned()
            .collect::<Vec<_>>();
        for record in &changes {
            validate_record(record)?;
        }
        if !changes.is_empty() {
            self.save(&mut doc)?;
        }
        Ok((doc.snapshot(), changes))
    }
}
fn next_rank(values: impl Iterator<Item = i64>) -> i64 {
    values.max().unwrap_or(-1024).saturating_add(1024)
}
fn settings_value(doc: &SidebarDocument, sort: ProjectSidebarSort) -> ProjectSidebarValue {
    let extra = match doc.records.get("settings").map(|record| &record.value) {
        Some(ProjectSidebarValue::Settings { extra, .. }) => extra.clone(),
        _ => BTreeMap::new(),
    };
    ProjectSidebarValue::Settings { sort, extra }
}
fn validate_record(record: &ProjectSidebarRecord) -> ProjectResult<()> {
    let valid = match &record.value {
        ProjectSidebarValue::Section { section } => {
            !section.section_id.is_empty()
                && !section.name.trim().is_empty()
                && record.id == format!("section:{}", section.section_id)
                && section.order.unsigned_abs() <= 1_000_000_000_000_000
        }
        ProjectSidebarValue::Project { item } => {
            !item.project_id.is_empty()
                && record.id == format!("project:{}", item.project_id)
                && item.order.unsigned_abs() <= 1_000_000_000_000_000
                && item.pin_order.unsigned_abs() <= 1_000_000_000_000_000
        }
        ProjectSidebarValue::Settings { .. } => record.id == "settings",
    };
    if !valid || record.version.is_empty() {
        return Err(ProjectError::InvalidResources(
            "Invalid sidebar record identity".into(),
        ));
    }
    Ok(())
}

impl ProjectSidebarRecord {
    pub fn is_tombstone(&self) -> bool {
        matches!(&self.value,ProjectSidebarValue::Section{section} if section.deleted)
    }
}
fn between_ranks(lower: Option<i64>, upper: Option<i64>) -> Option<i64> {
    match (lower, upper) {
        (Some(low), Some(high)) if high.saturating_sub(low) > 1 => Some(low + (high - low) / 2),
        (None, Some(high)) => high.checked_sub(1024),
        (Some(low), None) => low.checked_add(1024),
        (None, None) => Some(0),
        _ => None,
    }
}
fn effective_section<'a>(
    item: &'a ProjectSidebarItem,
    sections: &BTreeMap<String, ProjectSidebarSection>,
) -> Option<&'a str> {
    item.section_id
        .as_deref()
        .filter(|id| sections.contains_key(*id))
}
