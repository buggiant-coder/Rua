pub mod android_runtime;
pub mod atomic;
pub mod cells;
pub mod copy;
pub mod error;
pub mod frontmatter;
pub mod install_meta;
pub mod lock;
pub mod manifest;
pub mod scanner_integration;
pub mod service;
pub mod signature;
pub mod types;

pub mod builtin;
pub mod channels;
pub mod env;
pub mod plugin_skill;
pub mod pool_service;
pub mod project;
pub mod reconcile;

pub use android_runtime::{
    scan_android_skill_runtime, scan_android_skill_runtime_value,
    scan_android_skill_runtime_value_with_authority, scan_android_skill_runtime_with_authority,
    AndroidRuntimeProbe, AndroidRuntimeProbePlan, AndroidRuntimeProbeStatus, AndroidRuntimeScan,
    AndroidRuntimeSignal, AndroidSkillRuntime, SkillRuntimePathContract,
};
pub use atomic::{
    bump_manifest_version, next_manifest_version, write_json_atomic, write_manifest_atomic,
};
pub use builtin::{
    builtin_signature, builtin_signatures, builtin_skill_names, default_pool_dir,
    ensure_skill_pool_initialized, ensure_skill_pool_initialized_in, extract_builtin_to,
    import_builtin_skills, import_builtin_skills_in, install_default_builtin_signature_provider,
    list_builtin_import_candidates, list_builtin_import_candidates_in, update_single_builtin,
    update_single_builtin_in, BuiltinCandidate, BuiltinStatus, ImportBuiltinReport, BUILTIN_SKILLS,
};
pub use cells::{WorkspaceSkillCell, WorkspaceSkillCells};
pub use channels::{channel_matches, resolve_effective_skills, ALL_SKILL_ROUTING_CHANNELS};
pub use copy::{build_signature_skip_artifacts, copy_skill_dir};
pub use env::{is_main_thread, skill_config_env_var_name, EnvEntry, SkillEnvOverrides};
pub use error::SkillError;
pub use frontmatter::{
    extract_requirements, parse_skill_md, parse_skill_md_file, parse_skill_md_safe, ParsedSkillMd,
    REQUIREMENTS_METADATA_NAMESPACES,
};
pub use install_meta::{compute_content_hash, read_install_meta, write_install_meta, InstallMeta};
pub use lock::{lock_path_for, ManifestLock};
pub use manifest::{
    PoolManifest, PoolSkillEntry, SkillMetadata, WorkspaceManifest, WorkspaceSkillEntry,
    POOL_SCHEMA_VERSION, WORKSPACE_SCHEMA_VERSION,
};
pub use plugin_skill::{build_plugin_skill_info, parse_plugin_skill_id, plugin_skill_id};
pub use pool_service::{
    suggest_conflict_name as suggest_pool_conflict_name, DownloadResult, PoolBuiltinNotice,
    PoolBuiltinNoticeItem, SkillPoolService, SyncStatus,
};
pub use project::{
    discover_project_skills, discover_project_skills_with_limits, project_skill_watch_plan,
    ProjectSkill, ProjectSkillDiagnostic, ProjectSkillDiagnosticKind, ProjectSkillDiscovery,
    ProjectSkillDiscoveryLimits, ProjectSkillWatchPlan, ProjectSkillWatcher,
    PROJECT_SKILLS_RELATIVE_ROOT,
};
pub use reconcile::{
    clear_builtin_signature_provider, install_builtin_signature_provider, reconcile_pool_manifest,
    reconcile_pool_manifest_with, reconcile_workspace_manifest, reconcile_workspace_manifest_with,
    BuiltinSignatureProvider,
};
pub use scanner_integration::{maybe_scan_skill, InMemoryScannerHandle, ScanCall, ScannerHandle};
pub use service::{
    ImportConflict, ImportOptions, ImportOutcome, SaveOutcome, SkillBundleWrite, SkillService,
};
pub use signature::{build_signature, is_ignored_skill_path, IGNORED_ARTIFACTS};
pub use types::{SkillInfo, SkillRequirements, SkillSource};
