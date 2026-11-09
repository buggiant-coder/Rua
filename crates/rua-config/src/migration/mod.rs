pub mod atomic;
pub mod backup;
pub mod engine;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentMeta {
    pub schema_name: String,
    pub schema_version: u32,
    #[serde(default)]
    pub migrated_from: Option<String>,
    #[serde(default)]
    pub migrated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct MigrationPlan {
    pub source_path: PathBuf,
    pub target_path: PathBuf,
    pub schema_name: String,
    pub from_version: Option<u32>,
    pub to_version: u32,
    pub operations: Vec<MigrationOperation>,
    pub requires_backup: bool,
    pub destructive: bool,
}

#[derive(Debug, Clone)]
pub enum MigrationOperation {
    ReadFile {
        path: PathBuf,
    },

    DetectSchema {
        path: PathBuf,
    },

    Backup {
        source: PathBuf,
        backup_dir: PathBuf,
    },

    TransformJson {
        description: String,
    },

    AtomicWrite {
        path: PathBuf,
    },

    Verify {
        path: PathBuf,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationReport {
    pub schema_name: String,
    pub source_path: String,
    pub target_path: String,
    pub from_version: Option<u32>,
    pub to_version: u32,
    pub backup_path: Option<String>,
    pub operations_planned: Vec<String>,
    pub success: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetectedSchema {
    RootConfig,

    ProviderConfig,

    DefaultModel,

    JobsFile,

    AgentProfile,

    Unknown,
}

pub fn detect_schema(value: &serde_json::Value) -> DetectedSchema {
    let obj = match value.as_object() {
        Some(o) => o,
        None => return DetectedSchema::Unknown,
    };

    if obj.contains_key("version") && obj.get("jobs").is_some_and(|v| v.is_object()) {
        return DetectedSchema::JobsFile;
    }

    if obj.contains_key("provider_id") && obj.contains_key("model") && obj.len() <= 5 {
        return DetectedSchema::DefaultModel;
    }

    if obj.contains_key("id")
        && (obj.contains_key("base_url") || obj.contains_key("api_key"))
        && obj.get("models").is_some_and(|v| v.is_array())
    {
        return DetectedSchema::ProviderConfig;
    }

    if obj.contains_key("id") && obj.contains_key("name") && obj.contains_key("workspace_dir") {
        return DetectedSchema::AgentProfile;
    }

    if obj.contains_key("channels") || obj.contains_key("agents") || obj.contains_key("mcp") {
        return DetectedSchema::RootConfig;
    }

    DetectedSchema::Unknown
}

pub fn plan_migration(
    source_path: PathBuf,
    target_path: Option<PathBuf>,
    detected: DetectedSchema,
) -> MigrationPlan {
    let target = target_path.unwrap_or_else(|| source_path.clone());
    let schema_name = match detected {
        DetectedSchema::RootConfig => "config.json",
        DetectedSchema::ProviderConfig => "provider",
        DetectedSchema::DefaultModel => "default_model",
        DetectedSchema::JobsFile => "jobs_file",
        DetectedSchema::AgentProfile => "agent_profile",
        DetectedSchema::Unknown => "unknown",
    };

    let operations = vec![
        MigrationOperation::ReadFile {
            path: source_path.clone(),
        },
        MigrationOperation::DetectSchema {
            path: source_path.clone(),
        },
        MigrationOperation::Backup {
            source: source_path.clone(),
            backup_dir: source_path.parent().unwrap_or(&source_path).join(".backup"),
        },
        MigrationOperation::TransformJson {
            description: format!("Migrate {schema_name} to current schema"),
        },
        MigrationOperation::AtomicWrite {
            path: target.clone(),
        },
        MigrationOperation::Verify {
            path: target.clone(),
        },
    ];

    MigrationPlan {
        source_path,
        target_path: target,
        schema_name: schema_name.to_string(),
        from_version: None,
        to_version: 1,
        operations,
        requires_backup: true,
        destructive: false,
    }
}

pub fn dry_run(plan: &MigrationPlan) -> MigrationReport {
    let operations_planned: Vec<String> = plan
        .operations
        .iter()
        .map(|op| match op {
            MigrationOperation::ReadFile { path } => {
                format!("Read: {}", path.display())
            }
            MigrationOperation::DetectSchema { path } => {
                format!("Detect schema: {}", path.display())
            }
            MigrationOperation::Backup { source, backup_dir } => {
                format!("Backup: {} → {}", source.display(), backup_dir.display())
            }
            MigrationOperation::TransformJson { description } => {
                format!("Transform: {description}")
            }
            MigrationOperation::AtomicWrite { path } => {
                format!("Write (atomic): {}", path.display())
            }
            MigrationOperation::Verify { path } => {
                format!("Verify: {}", path.display())
            }
        })
        .collect();

    MigrationReport {
        schema_name: plan.schema_name.clone(),
        source_path: plan.source_path.display().to_string(),
        target_path: plan.target_path.display().to_string(),
        from_version: plan.from_version,
        to_version: plan.to_version,
        backup_path: Some(
            plan.source_path
                .parent()
                .unwrap_or(&plan.source_path)
                .join(".backup")
                .display()
                .to_string(),
        ),
        operations_planned,
        success: true,
        error: None,
    }
}
