import { request } from "../request";
import type {
  ProjectBulkArchiveResult,
  ProjectSidebarSnapshot,
  ProjectSidebarAction,
  ProjectArchiveResult,
  ProjectCreationOutcome,
  ProjectDescriptor,
  ProjectDirectoryListing,
  ProjectGitStatus,
  ProjectGitHistoryPage,
  ProjectLifecycle,
  ProjectLifecyclePreview,
  ProjectPermanentDeleteResult,
  ProjectReopenResult,
} from "../types/project";

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

  browseResource: (projectId: string, directoryId: string, path = "") =>
    request<ProjectDirectoryListing>(
      `/projects/${encodeURIComponent(
        projectId,
      )}/workspace/files?directory_id=${encodeURIComponent(
        directoryId,
      )}&path=${encodeURIComponent(path)}`,
    ),

  getGitStatus: (projectId: string, directoryId?: string) =>
    request<ProjectGitStatus>(
      `/projects/${encodeURIComponent(projectId)}/workspace/git${
        directoryId ? `?directory_id=${encodeURIComponent(directoryId)}` : ""
      }`,
    ),

  getGitHistory: (
    projectId: string,
    offset = 0,
    limit = 30,
    directoryId?: string,
  ) =>
    request<ProjectGitHistoryPage>(
      `/projects/${encodeURIComponent(
        projectId,
      )}/workspace/git/history?offset=${offset}&limit=${limit}${
        directoryId ? `&directory_id=${encodeURIComponent(directoryId)}` : ""
      }`,
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
    resourceDeviceId: string,
    resourceRoot: string | string[],
    displayName?: string,
    primaryIndex = 0,
  ) =>
    request<ProjectCreationOutcome>("/projects", {
      method: "POST",
      body: JSON.stringify({
        resource_device_id: resourceDeviceId,
        ...(Array.isArray(resourceRoot)
          ? { resource_roots: resourceRoot, primary_index: primaryIndex }
          : { resource_root: resourceRoot }),
        display_name: displayName,
      }),
    }),

  editResources: (
    projectId: string,
    expectedRevision: number,
    resourceRoots: string[],
    primaryIndex: number,
    directoryIds?: Array<string | null>,
  ) =>
    request<ProjectCreationOutcome>(
      `/projects/${encodeURIComponent(projectId)}/resources`,
      {
        method: "PATCH",
        body: JSON.stringify({
          expected_revision: expectedRevision,
          resource_roots: resourceRoots,
          primary_index: primaryIndex,
          ...(directoryIds ? { directory_ids: directoryIds } : {}),
        }),
      },
    ),

  setPrimaryDirectory: (
    projectId: string,
    expectedRevision: number,
    directoryId: string,
  ) =>
    request<ProjectDescriptor>(
      `/projects/${encodeURIComponent(projectId)}/resources/primary`,
      {
        method: "PATCH",
        body: JSON.stringify({
          expected_revision: expectedRevision,
          directory_id: directoryId,
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
