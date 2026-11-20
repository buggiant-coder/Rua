mod git;
mod model;
mod path;
mod resources;
mod store;

pub use git::{
    inspect_project_git, inspect_project_git_history, ProjectGitChange, ProjectGitCommit,
    ProjectGitHead, ProjectGitHistoryPage, ProjectGitStatus,
};
pub use model::{
    ProjectCreateRequest, ProjectCreateResourcesRequest, ProjectCreationOutcome, ProjectDescriptor,
    ProjectDirectoryEntry, ProjectDirectoryEntryKind, ProjectDirectoryListing, ProjectLifecycle,
    ProjectOverlapRelation, ProjectOverlapWarning,
};
pub use path::{
    create_project_directory, list_project_directory, normalize_project_root,
    propose_project_display_name, validate_project_directory,
};
pub use store::{
    project_tombstone_exists, ProjectAdmissionGuard, ProjectCatalog, ProjectDeletePreparation,
    ProjectError, ProjectExclusiveAdmissionGuard, ProjectResult, ProjectTombstone,
};

pub use resources::{ProjectResourceDirectory, ProjectResourceDirectoryInput, ProjectResources};

mod sidebar;
pub use sidebar::{
    ProjectSidebarAction, ProjectSidebarItem, ProjectSidebarRecord, ProjectSidebarSection,
    ProjectSidebarSnapshot, ProjectSidebarSort, ProjectSidebarStore, ProjectSidebarValue,
};
