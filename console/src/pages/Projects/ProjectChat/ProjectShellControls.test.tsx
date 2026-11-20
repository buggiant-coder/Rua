import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { beforeEach, it, expect, vi } from "vitest";
import { ProjectShellDrawer } from "./ProjectShellControls";
import { invalidateConversationNavigation } from "@/layouts/conversationNavigationEvents";
import { act } from "@testing-library/react";
import type { ProjectDescriptor } from "@/api/types/project";
import { projectsApi } from "@/api/modules/projects";
vi.mock("react-i18next", () => {
  const t = (key: string) => key;
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
    expect(projectsApi.browseResource).toHaveBeenCalledWith("p", "docs", ""),
  );
  expect(projectsApi.getGitStatus).not.toHaveBeenCalled();
  const drawer = screen.getByRole("dialog");
  const mask = drawer.parentElement;
  const selector = screen.getByRole("combobox");
  await waitFor(() => expect(mask).toHaveClass("rua-ui-drawer-mask--visible"));
  fireEvent.click(screen.getByRole("tab", { name: "projects.workspace.git" }));
  await waitFor(() =>
    expect(projectsApi.getGitStatus).toHaveBeenCalledWith("p", "docs"),
  );
  fireEvent.click(selector);
  fireEvent.click(await screen.findByRole("option", { name: /remote.*\/app/ }));
  await waitFor(() =>
    expect(projectsApi.getGitStatus).toHaveBeenCalledWith("p", "app"),
  );
  expect(projectsApi.browseResource).not.toHaveBeenCalledWith("p", "app", "");
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
    expect(projectsApi.browseResource).toHaveBeenCalledWith("p", "app", ""),
  );
  expect(projectsApi.browseResource).not.toHaveBeenCalledWith("p", "docs", "");
  fireEvent.click(screen.getByRole("combobox"));
  expect(
    screen.queryByRole("option", { name: /phone.*\/docs/ }),
  ).not.toBeInTheDocument();
});
it("reconciles a selected directory after a project edit while open", async () => {
  render(drawer());
  await waitFor(() =>
    expect(projectsApi.browseResource).toHaveBeenCalledWith("p", "docs", ""),
  );
  vi.mocked(projectsApi.get).mockResolvedValue(removed);
  vi.mocked(projectsApi.browseResource).mockClear();
  act(() => invalidateConversationNavigation());
  await waitFor(() =>
    expect(projectsApi.browseResource).toHaveBeenCalledWith("p", "app", ""),
  );
  expect(projectsApi.browseResource).not.toHaveBeenCalledWith("p", "docs", "");
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
    expect(projectsApi.browseResource).toHaveBeenCalledWith("p", "app", ""),
  );
  await act(async () => resolveOld(project));
  fireEvent.click(screen.getByRole("combobox"));
  expect(
    screen.queryByRole("option", { name: /phone.*\/docs/ }),
  ).not.toBeInTheDocument();
  expect(projectsApi.browseResource).not.toHaveBeenCalledWith("p", "docs", "");
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
