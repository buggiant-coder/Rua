import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { beforeEach, it, expect, vi } from "vitest";
import { ProjectShellDrawer } from "./ProjectShellControls";
import { invalidateConversationNavigation } from "@/layouts/conversationNavigationEvents";
import { act } from "@testing-library/react";
import type { ProjectDescriptor } from "@/api/types/project";
import { projectsApi } from "@/api/modules/projects";
vi.mock("react-i18next", () => {
  const t = (key: string, options?: { path?: string; count?: number }) =>
    key === "projects.workspace.omittedEntries"
      ? `${key} ${options?.path} ${options?.count}`
      : key;
  return { useTranslation: () => ({ t }) };
});
vi.mock("@/desktop/runtime", () => ({ canOpenProjectDirectory: () => false }));
vi.mock("@/api/modules/projects", () => ({
  projectsApi: {
    get: vi.fn(),
    browseResource: vi.fn(async () => ({
      path: "/docs",
      entries: [],
      roots: [],
    })),
    getGitStatus: vi.fn(async () => ({ repository: false })),
    getGitHistory: vi.fn(async () => ({ items: [], total: 0 })),
  },
}));
it("browses the first resource and scopes Files and Git to the explicitly selected identity", async () => {
  render(
    <ProjectShellDrawer
      project={{
        project_id: "p",
        display_name: "Project",
        lifecycle: "active",
        created_at: "",
        updated_at: "",
        resources: {
          revision: 1,
          directories: [
            { directory_id: "app", device_id: "remote", resource_root: "/app" },
            {
              directory_id: "docs",
              device_id: "phone",
              resource_root: "/docs",
            },
          ],
        },
      }}
      devices={[]}
      lazy
      resourcePolicy={null}
      policyLoading={false}
      policyError={null}
      onClose={() => {}}
    />,
  );
  await waitFor(() =>
    expect(projectsApi.browseResource).toHaveBeenCalledWith(
      "p",
      { device_id: "phone", resource_root: "/docs" },
      "/docs",
      0,
      expect.any(AbortSignal),
    ),
  );
  expect(projectsApi.getGitStatus).not.toHaveBeenCalled();
  const drawer = screen.getByRole("dialog");
  const mask = drawer.parentElement;
  const selector = screen.getByRole("combobox");
  await waitFor(() => expect(mask).toHaveClass("rua-ui-drawer-mask--visible"));
  fireEvent.click(screen.getByRole("tab", { name: "projects.workspace.git" }));
  await waitFor(() =>
    expect(projectsApi.getGitStatus).toHaveBeenCalledWith(
      "p",
      { device_id: "phone", resource_root: "/docs" },
      expect.any(AbortSignal),
    ),
  );
  fireEvent.click(selector);
  fireEvent.click(await screen.findByRole("option", { name: /remote.*\/app/ }));
  await waitFor(() =>
    expect(projectsApi.getGitStatus).toHaveBeenCalledWith(
      "p",
      { device_id: "remote", resource_root: "/app" },
      expect.any(AbortSignal),
    ),
  );
  expect(projectsApi.browseResource).not.toHaveBeenCalledWith(
    "p",
    { device_id: "remote", resource_root: "/app" },
    "/app",
    0,
    expect.any(AbortSignal),
  );
  expect(screen.getByRole("dialog")).toBe(drawer);
  expect(drawer.parentElement).toBe(mask);
  expect(mask).toHaveClass("rua-ui-drawer-mask--visible");
  expect(screen.getByRole("combobox")).toBe(selector);
  expect(
    screen.getByRole("tab", { name: "projects.workspace.git" }),
  ).toHaveAttribute("aria-selected", "true");
  expect(
    screen.queryByText("projects.openResourceDirectory"),
  ).not.toBeInTheDocument();
});

const project: ProjectDescriptor = {
  project_id: "p",
  display_name: "Project",
  lifecycle: "active",
  created_at: "",
  updated_at: "",
  resources: {
    revision: 1,
    directories: [
      { directory_id: "docs", device_id: "phone", resource_root: "/docs" },
      { directory_id: "app", device_id: "remote", resource_root: "/app" },
    ],
  },
};
const removed = {
  ...project,
  resources: {
    ...project.resources!,
    revision: 2,
    directories: [
      project.resources!.directories.find((d) => d.directory_id === "app")!,
    ],
  },
};
beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(projectsApi.get).mockResolvedValue(project);
});
function drawer() {
  return (
    <ProjectShellDrawer
      project={project}
      devices={[]}
      lazy
      resourcePolicy={null}
      policyLoading={false}
      policyError={null}
      onClose={() => {}}
    />
  );
}
it("loads current directories before browsing an old page snapshot", async () => {
  vi.mocked(projectsApi.get).mockResolvedValue(removed);
  render(drawer());
  await waitFor(() =>
    expect(projectsApi.browseResource).toHaveBeenCalledWith(
      "p",
      { device_id: "remote", resource_root: "/app" },
      "/app",
      0,
      expect.any(AbortSignal),
    ),
  );
  expect(projectsApi.browseResource).not.toHaveBeenCalledWith(
    "p",
    { device_id: "phone", resource_root: "/docs" },
    "/docs",
    0,
    expect.any(AbortSignal),
  );
  fireEvent.click(screen.getByRole("combobox"));
  expect(
    screen.queryByRole("option", { name: /phone.*\/docs/ }),
  ).not.toBeInTheDocument();
});
it("reconciles a selected directory after a project edit while open", async () => {
  render(drawer());
  await waitFor(() =>
    expect(projectsApi.browseResource).toHaveBeenCalledWith(
      "p",
      { device_id: "phone", resource_root: "/docs" },
      "/docs",
      0,
      expect.any(AbortSignal),
    ),
  );
  vi.mocked(projectsApi.get).mockResolvedValue(removed);
  vi.mocked(projectsApi.browseResource).mockClear();
  act(() => invalidateConversationNavigation());
  await waitFor(() =>
    expect(projectsApi.browseResource).toHaveBeenCalledWith(
      "p",
      { device_id: "remote", resource_root: "/app" },
      "/app",
      0,
      expect.any(AbortSignal),
    ),
  );
  expect(projectsApi.browseResource).not.toHaveBeenCalledWith(
    "p",
    { device_id: "phone", resource_root: "/docs" },
    "/docs",
    0,
    expect.any(AbortSignal),
  );
  expect(projectsApi.getGitStatus).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("combobox"));
  expect(
    screen.queryByRole("option", { name: /phone.*\/docs/ }),
  ).not.toBeInTheDocument();
});

it("does not expose stale directories when the current project cannot be loaded", async () => {
  vi.mocked(projectsApi.get).mockRejectedValue(
    new Error("Resource device offline"),
  );
  render(drawer());
  expect(
    await screen.findByText("Resource device offline"),
  ).toBeInTheDocument();
  expect(screen.queryByRole("combobox")).not.toBeInTheDocument();
  expect(projectsApi.browseResource).not.toHaveBeenCalled();
  expect(projectsApi.getGitStatus).not.toHaveBeenCalled();
});
it("ignores an older project response arriving after a resource edit", async () => {
  let resolveOld!: (value: ProjectDescriptor) => void;
  vi.mocked(projectsApi.get).mockReturnValueOnce(
    new Promise((resolve) => {
      resolveOld = resolve;
    }),
  );
  render(drawer());
  vi.mocked(projectsApi.get).mockResolvedValue(removed);
  act(() => invalidateConversationNavigation());
  await waitFor(() =>
    expect(projectsApi.browseResource).toHaveBeenCalledWith(
      "p",
      { device_id: "remote", resource_root: "/app" },
      "/app",
      0,
      expect.any(AbortSignal),
    ),
  );
  await act(async () => resolveOld(project));
  fireEvent.click(screen.getByRole("combobox"));
  expect(
    screen.queryByRole("option", { name: /phone.*\/docs/ }),
  ).not.toBeInTheDocument();
  expect(projectsApi.browseResource).not.toHaveBeenCalledWith(
    "p",
    { device_id: "phone", resource_root: "/docs" },
    "/docs",
    0,
    expect.any(AbortSignal),
  );
});

it("shows inherited Agent permissions and approvals for an empty Project without querying any paths", async () => {
  const empty = { ...project, resources: { revision: 2, directories: [] } };
  vi.mocked(projectsApi.get).mockResolvedValue(empty);
  render(
    <ProjectShellDrawer
      project={empty}
      devices={[]}
      lazy={false}
      resourcePolicy={{
        policy: null,
        pending_approval_count: 1,
        chat_rule_count: 2,
        agent_rule_count: 3,
      }}
      policyLoading={false}
      policyError={null}
      onClose={() => {}}
    />,
  );
  expect(await screen.findByRole("tabpanel")).toHaveTextContent(
    "projects.noResourceFolders",
  );
  fireEvent.click(
    screen.getByRole("tab", { name: "projects.workspace.context" }),
  );
  expect(screen.getByText("projects.workspace.agentPermissions")).toBeVisible();
  expect(screen.getByText("projects.workspace.approvalsSummary")).toBeVisible();
  expect(projectsApi.browseResource).not.toHaveBeenCalled();
  expect(projectsApi.getGitStatus).not.toHaveBeenCalled();
});

it("reports omitted entries for each expanded folder even when none can be listed", async () => {
  const common = {
    device_id: "phone",
    parent_path: null,
    roots: [],
    proposed_name: "",
  };
  vi.mocked(projectsApi.browseResource)
    .mockResolvedValueOnce({
      ...common,
      path: "/docs",
      omitted_entries: 2,
      entries: [
        {
          name: "private",
          path: "/docs/private",
          kind: "directory",
          is_symlink: false,
        },
      ],
    })
    .mockResolvedValueOnce({
      ...common,
      path: "/docs/private",
      entries: [],
      omitted_entries: 3,
    });
  render(drawer());
  expect(
    await screen.findByText("projects.workspace.omittedEntries /docs 2"),
  ).toBeVisible();
  const directory = screen.getByRole("button", { name: "private" });
  fireEvent.click(directory);
  expect(
    await screen.findByText(
      "projects.workspace.omittedEntries /docs/private 3",
    ),
  ).toBeVisible();
  fireEvent.click(directory);
  expect(
    screen.queryByText("projects.workspace.omittedEntries /docs/private 3"),
  ).not.toBeInTheDocument();
  expect(
    screen.getByText("projects.workspace.omittedEntries /docs 2"),
  ).toBeVisible();
});

it("refreshing files discards an older page and retains the selected Device and real path", async () => {
  const target = { device_id: "phone", resource_root: "/docs" };
  function listing(name: string, next_offset: number | null) {
    return {
      ...target,
      path: "/docs",
      parent_path: "/",
      entries: [
        {
          name,
          path: "/docs/" + name,
          kind: "file" as const,
          is_symlink: false,
        },
      ],
      roots: [],
      proposed_name: "docs",
      next_offset,
    };
  }
  let finish!: (result: ReturnType<typeof listing>) => void;
  vi.mocked(projectsApi.browseResource)
    .mockResolvedValueOnce(listing("first.txt", 1))
    .mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve;
        }),
    )
    .mockResolvedValueOnce(listing("new.txt", null));
  render(drawer());
  await screen.findByText("first.txt");
  fireEvent.click(
    screen.getByRole("button", { name: "projects.workspace.loadMore" }),
  );
  expect(projectsApi.browseResource).toHaveBeenNthCalledWith(
    2,
    "p",
    target,
    "/docs",
    1,
    expect.any(AbortSignal),
  );
  fireEvent.click(
    screen.getByRole("button", { name: "projects.workspace.refreshFiles" }),
  );
  await screen.findByText("new.txt");
  await act(async () => finish(listing("old.txt", null)));
  expect(screen.queryByText("old.txt")).not.toBeInTheDocument();
  expect(screen.queryByText("first.txt")).not.toBeInTheDocument();
  expect(
    screen.queryByRole("button", { name: "projects.workspace.loadMore" }),
  ).not.toBeInTheDocument();
});

it("loads only Git history for a next page and rejects its late response after refresh", async () => {
  function history(oid: string, summary: string, total: number, offset = 0) {
    return {
      items: [
        {
          oid: oid.repeat(40),
          parent_oids: [],
          summary,
          authored_at: "2026-11-20T12:00:00Z",
          refs: [],
        },
      ],
      offset,
      limit: 30,
      total,
    };
  }
  vi.mocked(projectsApi.getGitStatus).mockResolvedValue({
    repository: true,
    branch: "phone",
    detached: false,
    ahead: 0,
    behind: 0,
    changes: [],
  });
  let finish!: (result: ReturnType<typeof history>) => void;
  vi.mocked(projectsApi.getGitHistory)
    .mockResolvedValueOnce(history("a", "first commit", 3))
    .mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve;
        }),
    )
    .mockResolvedValueOnce(history("c", "refreshed commit", 1));
  render(drawer());
  await screen.findByRole("tab", { name: "projects.workspace.git" });
  fireEvent.click(screen.getByRole("tab", { name: "projects.workspace.git" }));
  await screen.findByText("first commit");
  fireEvent.click(
    screen.getByRole("button", { name: "projects.workspace.loadMore" }),
  );
  expect(projectsApi.getGitHistory).toHaveBeenNthCalledWith(
    2,
    "p",
    { device_id: "phone", resource_root: "/docs" },
    1,
    expect.any(AbortSignal),
  );
  expect(projectsApi.getGitStatus).toHaveBeenCalledTimes(1);
  fireEvent.click(
    screen.getByRole("button", { name: "projects.workspace.refreshGit" }),
  );
  await screen.findByText("refreshed commit");
  await act(async () => finish(history("b", "stale history", 3, 1)));
  expect(screen.queryByText("stale history")).not.toBeInTheDocument();
  expect(projectsApi.getGitStatus).toHaveBeenCalledTimes(2);
});
