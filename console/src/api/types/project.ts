export type ProjectLifecycle = "active" | "archived";

export interface ProjectDescriptor {
  project_id: string;
  display_name: string;
  resource_device_id: string;
  display_path: string;
  resources?: {
    revision: number;
    primary_directory_id: string;
    directories: Array<{ directory_id: string; display_path: string }>;
  };
  lifecycle: ProjectLifecycle;
  created_at: string;
  updated_at: string;
  /** List-only projection of the latest activity in the project Chat Index. */
  last_activity_at?: string | null;
  [key: string]: unknown;
}

export type ProjectOverlapRelation =
  | "shared_directory"
  | "existing_ancestor"
  | "existing_descendant";

export interface ProjectOverlapWarning {
  project_id: string;
  display_name: string;
  display_path: string;
  lifecycle: ProjectLifecycle;
  relation: ProjectOverlapRelation;
}

export interface ProjectCreationOutcome {
  project: ProjectDescriptor;
  overlap_warnings: ProjectOverlapWarning[];
}

export interface ProjectLifecycleImpact {
  affected_chat_count: number;
  active_turn_count: number;
  queued_input_count: number;
  pending_approval_count: number;
  pending_prompt_count: number;
  active_acp_count: number;
  background_task_count: number;
}

export interface ProjectLifecyclePreview {
  project: ProjectDescriptor;
  impact?: ProjectLifecycleImpact | null;
  resource_device_revoked: boolean;
}

export interface ProjectArchiveResult extends ProjectLifecyclePreview {
  changed: boolean;
}

export interface ProjectReopenResult {
  project: ProjectDescriptor;
  changed: boolean;
}

export interface ProjectPermanentDeleteResult {
  project_id: string;
  deleted_chat_count: number;
  changed: boolean;
}

export type ProjectDirectoryEntryKind = "directory" | "file" | "other";

export interface ProjectDirectoryEntry {
  name: string;
  path: string;
  kind: ProjectDirectoryEntryKind;
  is_symlink: boolean;
}

export interface ProjectDirectoryListing {
  device_id: string;
  path: string;
  parent_path: string | null;
  entries: ProjectDirectoryEntry[];
  roots: ProjectDirectoryEntry[];
  proposed_name: string;
}

export interface ProjectGitChange {
  path: string;
  original_path?: string | null;
  status: string;
  staged: boolean;
}

export interface ProjectGitHead {
  oid: string;
  summary: string;
  authored_at: string;
}

export interface ProjectGitCommit {
  oid: string;
  parent_oids: string[];
  summary: string;
  authored_at: string;
  refs: string[];
}

export interface ProjectGitStatus {
  repository: boolean;
  branch?: string | null;
  detached: boolean;
  ahead: number;
  behind: number;
  changes: ProjectGitChange[];
  head?: ProjectGitHead | null;
}

export interface ProjectGitHistoryPage {
  items: ProjectGitCommit[];
  offset: number;
  limit: number;
  total: number;
}

export interface ProjectDuplicateResponse {
  detail: string;
  code: "project_binding_duplicate";
  project: ProjectDescriptor;
  actions: Array<"view" | "reopen">;
}

export interface ProjectChatAgentSelection {
  agent_id: string;
  selected_at: string;
  revision: number;
}

export type ProjectAgentSwitchBlocker =
  | "active_turn"
  | "queued_input"
  | "unresolved_approval"
  | "unanswered_prompt"
  | "active_acp_binding";

export interface ProjectChatAgentState {
  selection: ProjectChatAgentSelection;
  selected_agent_available: boolean;
  switchable: boolean;
  blockers: ProjectAgentSwitchBlocker[];
}

export type ProjectSidebarSort =
  | "manual"
  | "name"
  | "created_at"
  | "last_activity_at";
export interface ProjectSidebarItem {
  project_id: string;
  pinned: boolean;
  order: number;
  pin_order: number;
  hidden?: boolean;
  section_id?: string | null;
}
export interface ProjectSidebarSection {
  section_id: string;
  name: string;
  order: number;
}
export interface ProjectSidebarSnapshot {
  sections?: Record<string, ProjectSidebarSection>;
  revision: string;
  sort: ProjectSidebarSort;
  items: Record<string, ProjectSidebarItem>;
}
export type ProjectSidebarAction =
  | { action: "create_section"; name: string }
  | { action: "rename_section"; section_id: string; name: string }
  | { action: "delete_section"; section_id: string }
  | {
      action: "reorder_section";
      section_id: string;
      ordered_section_ids: string[];
    }
  | { action: "move_to_section"; project_id: string; section_id: string | null }
  | { action: "set_hidden"; project_id: string; hidden: boolean }
  | { action: "set_sort"; sort: ProjectSidebarSort }
  | { action: "set_pinned"; project_id: string; pinned: boolean }
  | { action: "reorder"; project_id: string; ordered_project_ids: string[] };

export interface ProjectBulkArchiveResult {
  items: Array<{
    chat_id: string;
    status: "archived" | "skipped" | "failed";
    reason: string | null;
  }>;
}
