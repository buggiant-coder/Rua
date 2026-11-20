import type { ExecutionPosition } from "../types/chat";
import { request } from "../request";
import type {
  ProjectBulkArchiveResult,
  ProjectSidebarSnapshot,
  ProjectSidebarAction,
  ProjectArchiveResult,
  ProjectCreationOutcome,
  ProjectDescriptor,
  ProjectResourceInput,
  ProjectResourceDirectory,
  ProjectDirectoryListing,
  ProjectGitStatus,
  ProjectGitHistoryPage,
  ProjectLifecycle,
  ProjectLifecyclePreview,
  ProjectPermanentDeleteResult,
  ProjectReopenResult,
} from "../types/project";

type ResourceTarget = Pick<
  ProjectResourceDirectory,
  "device_id" | "resource_root"
>;

function workspacePath(
  projectId: string,
  operation: string,
  target: ResourceTarget,
  fields?: Record<string, string>,
) {
  const query = new URLSearchParams({
    device_id: target.device_id,
    resource_root: target.resource_root,
    ...fields,
  });
  return `/projects/${encodeURIComponent(
    projectId,
  )}/workspace/${operation}?${query}`;
}

export const projectsApi = {
  archiveIdleChats: (projectId: string, chatIds?: string[]) =>
    request<ProjectBulkArchiveResult>(
      `/projects/${encodeURIComponent(projectId)}/chats/archive-idle`,
      { method: "POST", body: JSON.stringify({ chat_ids: chatIds }) },
    ),
  getSidebar: () => request<ProjectSidebarSnapshot>("/project-sidebar"),
  updateSidebar: (expectedRevision: string, action: ProjectSidebarAction) =>
    request<ProjectSidebarSnapshot>("/project-sidebar", {
      method: "PATCH",
      body: JSON.stringify({ expected_revision: expectedRevision, ...action }),
    }),
  list: (lifecycle: ProjectLifecycle | "all" = "active") =>
    request<{ projects: ProjectDescriptor[] }>(
      `/projects?lifecycle=${encodeURIComponent(lifecycle)}`,
    ).then((response) => response.projects),

  get: (projectId: string) =>
    request<ProjectDescriptor>(`/projects/${encodeURIComponent(projectId)}`),

  browseResource: (
    projectId: string,
    target: ResourceTarget,
    path: string,
    offset = 0,
    signal?: AbortSignal,
  ) =>
    request<ProjectDirectoryListing>(
      workspacePath(projectId, "files", target, {
        path,
        offset: String(offset),
      }),
      { signal },
    ),

  getGitStatus: (
    projectId: string,
    target: ResourceTarget,
    signal?: AbortSignal,
  ) =>
    request<ProjectGitStatus>(workspacePath(projectId, "git", target), {
      signal,
    }),

  getGitHistory: (
    projectId: string,
    target: ResourceTarget,
    offset = 0,
    signal?: AbortSignal,
  ) =>
    request<ProjectGitHistoryPage>(
      workspacePath(projectId, "git/history", target, {
        offset: String(offset),
        limit: "30",
      }),
      { signal },
    ),

  browseDirectory: (deviceId: string, path: string) =>
    request<ProjectDirectoryListing>("/projects/directory", {
      method: "POST",
      body: JSON.stringify({ device_id: deviceId, path }),
    }),

  createDirectory: (deviceId: string, parent: string, name: string) =>
    request<ProjectDirectoryListing>("/projects/directory/create", {
      method: "POST",
      body: JSON.stringify({ device_id: deviceId, parent, name }),
    }),

  create: (
    displayName: string,
    directories: ProjectResourceInput[],
    executionDefault: ExecutionPosition | null = null,
  ) =>
    request<ProjectCreationOutcome>("/projects", {
      method: "POST",
      body: JSON.stringify({
        display_name: displayName,
        directories,
        execution_default: executionDefault,
      }),
    }),

  setExecutionDefault: (
    projectId: string,
    position: ExecutionPosition | null,
  ) =>
    request<ProjectDescriptor>(
      `/projects/${encodeURIComponent(projectId)}/execution-default`,
      {
        method: "PUT",
        body: JSON.stringify({ execution_default: position }),
      },
    ),

  editResources: (
    projectId: string,
    expectedRevision: number,
    directories: ProjectResourceInput[],
  ) =>
    request<ProjectCreationOutcome>(
      `/projects/${encodeURIComponent(projectId)}/resources`,
      {
        method: "PATCH",
        body: JSON.stringify({
          expected_revision: expectedRevision,
          directories,
        }),
      },
    ),

  rename: (projectId: string, displayName: string) =>
    request<ProjectDescriptor>(`/projects/${encodeURIComponent(projectId)}`, {
      method: "PATCH",
      body: JSON.stringify({ display_name: displayName }),
    }),

  getLifecyclePreview: (projectId: string) =>
    request<ProjectLifecyclePreview>(
      `/projects/${encodeURIComponent(projectId)}/lifecycle-preview`,
    ),

  archive: (projectId: string) =>
    request<ProjectArchiveResult>(
      `/projects/${encodeURIComponent(projectId)}/archive`,
      {
        method: "POST",
        body: JSON.stringify({ confirmed: true }),
      },
    ),

  reopen: (projectId: string) =>
    request<ProjectReopenResult>(
      `/projects/${encodeURIComponent(projectId)}/reopen`,
      { method: "POST" },
    ),

  permanentlyDelete: (projectId: string) =>
    request<ProjectPermanentDeleteResult>(
      `/projects/${encodeURIComponent(projectId)}/permanent-delete`,
      {
        method: "POST",
        body: JSON.stringify({ confirmed: true }),
      },
    ),
};
