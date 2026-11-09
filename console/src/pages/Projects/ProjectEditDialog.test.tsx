import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { describe, it, expect, vi, beforeEach } from "vitest";
import ProjectEditDialog from "./ProjectEditDialog";

const api = vi.hoisted(() => ({
  setPrimaryDirectory: vi.fn(),
  rename: vi.fn(),
  editResources: vi.fn(),
  browseDirectory: vi.fn(),
}));
vi.mock("@/api/modules/projects", () => ({ projectsApi: api }));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string, options?: { path?: string }) =>
      options?.path ? `${key} ${options.path}` : key,
  }),
}));
vi.mock("@/ui", () => ({
  Btn: ({ children, variant: _variant, ...props }: any) => (
    <button {...props}>{children}</button>
  ),
  Modal: ({ children, onOk, onClose, okText, okButtonProps }: any) => (
    <div role="dialog">
      {children}
      <button onClick={onClose}>Cancel</button>
      <button onClick={onOk} {...okButtonProps}>
        {okText}
      </button>
    </div>
  ),
  toast: { warning: vi.fn() },
  FormField: ({ children }: any) => <div>{children}</div>,
  TextInput: (props: any) => <input {...props} />,
  InfoBox: ({ children, type }: any) => (
    <div role={type === "error" ? "alert" : "status"}>{children}</div>
  ),
}));
const project = {
  project_id: "project",
  display_name: "App",
  display_path: "/app",
  resource_device_id: "device",
  lifecycle: "active" as const,
  created_at: "now",
  updated_at: "now",
  resources: {
    revision: 4,
    primary_directory_id: "app",
    directories: [
      { directory_id: "app", display_path: "/app" },
      { directory_id: "docs", display_path: "/docs" },
    ],
  },
};
beforeEach(() => {
  vi.clearAllMocks();
  api.browseDirectory.mockImplementation(
    async (_device: string, path: string) => ({
      path,
      parent_path: "/",
      roots: [],
      entries: [],
    }),
  );
});
describe("Project editing", () => {
  it("falls back to Home when the old parent is missing and cancels without changing the binding", async () => {
    const saved = vi.fn();
    api.browseDirectory.mockImplementation(
      async (_device: string, path: string) => {
        if (path === "/gone") throw new Error("Directory unavailable");
        return {
          path: path === "~" ? "/home/user" : path,
          parent_path: "/",
          roots: [],
          entries: [],
        };
      },
    );
    render(
      <ProjectEditDialog
        project={{
          ...project,
          display_path: "/gone/app",
          resources: {
            ...project.resources,
            directories: [{ directory_id: "app", display_path: "/gone/app" }],
          },
        }}
        onClose={vi.fn()}
        onSaved={saved}
      />,
    );
    fireEvent.click(
      screen.getByRole("button", {
        name: "projects.changeResourceFolder /gone/app",
      }),
    );
    await waitFor(() =>
      expect(
        screen.getByRole("textbox", { name: "projects.folderPath" }),
      ).toHaveValue("/home/user"),
    );
    expect(api.browseDirectory).toHaveBeenCalledWith("device", "~");
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    fireEvent.click(screen.getByText("Cancel"));
    expect(screen.getByTitle("/gone/app")).toBeVisible();
    fireEvent.click(screen.getByRole("button", { name: "common.save" }));
    await waitFor(() => expect(saved).toHaveBeenCalled());
    expect(api.editResources).not.toHaveBeenCalled();
  });
  it("rebinds the only folder while preserving its identity and primary selection", async () => {
    const original = {
      ...project,
      resources: {
        ...project.resources,
        directories: [project.resources.directories[0]],
      },
    };
    const saved = vi.fn();
    api.editResources.mockResolvedValue({
      project: {
        ...original,
        display_path: "/renamed",
        resources: {
          ...original.resources,
          revision: 5,
          directories: [{ directory_id: "app", display_path: "/renamed" }],
        },
      },
      overlap_warnings: [],
    });
    render(
      <ProjectEditDialog
        project={original}
        onClose={vi.fn()}
        onSaved={saved}
      />,
    );
    fireEvent.click(
      screen.getByRole("button", {
        name: "projects.changeResourceFolder /app",
      }),
    );
    const input = await screen.findByRole("textbox", {
      name: "projects.folderPath",
    });
    await waitFor(() => expect(input).toHaveValue("/"));
    fireEvent.change(input, { target: { value: "/renamed" } });
    fireEvent.click(
      screen.getByRole("button", { name: "projects.changeFolder" }),
    );
    expect(await screen.findByTitle("/renamed")).toBeVisible();
    expect(
      screen.getByRole("radio", { name: "projects.useAsPrimary /renamed" }),
    ).toBeChecked();
    fireEvent.click(screen.getByRole("button", { name: "common.save" }));
    await waitFor(() =>
      expect(api.editResources).toHaveBeenCalledWith(
        "project",
        4,
        ["/renamed"],
        0,
        ["app"],
      ),
    );
    await waitFor(() => expect(saved).toHaveBeenCalled());
  });
  it("uses the displayed revision when choosing another primary directory", async () => {
    const saved = vi.fn();
    api.setPrimaryDirectory.mockResolvedValue({
      ...project,
      display_path: "/docs",
      resources: {
        ...project.resources,
        revision: 5,
        primary_directory_id: "docs",
      },
    });
    render(
      <ProjectEditDialog project={project} onClose={vi.fn()} onSaved={saved} />,
    );
    fireEvent.click(
      screen.getByRole("radio", { name: "projects.useAsPrimary /docs" }),
    );
    fireEvent.click(screen.getByRole("button", { name: "common.save" }));
    await waitFor(() => expect(saved).toHaveBeenCalled());
    expect(api.setPrimaryDirectory).toHaveBeenCalledWith("project", 4, "docs");
    expect(api.rename).not.toHaveBeenCalled();
  });
  it("detaches a folder with the expected revision without a file delete operation", async () => {
    const saved = vi.fn();
    api.editResources.mockResolvedValue({
      project: {
        ...project,
        resources: {
          ...project.resources,
          revision: 5,
          directories: [project.resources.directories[0]],
        },
      },
      overlap_warnings: [],
    });
    render(
      <ProjectEditDialog project={project} onClose={vi.fn()} onSaved={saved} />,
    );
    fireEvent.click(
      screen.getByRole("button", {
        name: "projects.removeResourceFolder /docs",
      }),
    );
    fireEvent.click(screen.getByRole("button", { name: "common.save" }));
    await waitFor(() => expect(saved).toHaveBeenCalled());
    expect(api.editResources).toHaveBeenCalledWith("project", 4, ["/app"], 0, [
      "app",
    ]);
  });
  it("requires an explicit replacement before detaching the primary", async () => {
    const saved = vi.fn();
    render(
      <ProjectEditDialog project={project} onClose={vi.fn()} onSaved={saved} />,
    );
    fireEvent.click(
      screen.getByRole("button", {
        name: "projects.removeResourceFolder /app",
      }),
    );
    fireEvent.click(screen.getByRole("button", { name: "common.save" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "projects.selectRemainingPrimary",
    );
    expect(api.editResources).not.toHaveBeenCalled();
  });
  it("keeps the edit open when resource authority rejects it", async () => {
    const saved = vi.fn();
    api.setPrimaryDirectory.mockRejectedValue(new Error("Project is busy"));
    render(
      <ProjectEditDialog project={project} onClose={vi.fn()} onSaved={saved} />,
    );
    fireEvent.click(
      screen.getByRole("radio", { name: "projects.useAsPrimary /docs" }),
    );
    fireEvent.click(screen.getByRole("button", { name: "common.save" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Project is busy",
    );
    expect(saved).not.toHaveBeenCalled();
  });
});

async function chooseFolder(path: string) {
  fireEvent.click(screen.getByRole("button", { name: "projects.addFolder" }));
  await waitFor(() => expect(api.browseDirectory).toHaveBeenCalled());
  const input = screen.getByRole("textbox", { name: "projects.folderPath" });
  await waitFor(() => expect(input).toHaveValue("/"));
  fireEvent.change(input, { target: { value: path } });
}
function withFolder() {
  return {
    project: {
      ...project,
      resources: {
        ...project.resources,
        revision: 5,
        directories: [
          ...project.resources.directories,
          { directory_id: "new", display_path: "/new-folder" },
        ],
      },
    },
    overlap_warnings: [],
  };
}
it("shows the selected folder in the list, saves it and retains it when reopened", async () => {
  const saved = vi.fn();
  api.editResources.mockResolvedValue(withFolder());
  const first = render(
    <ProjectEditDialog project={project} onClose={vi.fn()} onSaved={saved} />,
  );
  await chooseFolder("/new-folder");
  expect(
    screen.queryByRole("button", { name: "common.save" }),
  ).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "projects.addFolder" }));
  expect(await screen.findByTitle("/new-folder")).toBeVisible();
  expect(api.editResources).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "common.save" }));
  await waitFor(() =>
    expect(api.editResources).toHaveBeenCalledWith(
      "project",
      4,
      ["/app", "/docs", "/new-folder"],
      0,
      ["app", "docs", null],
    ),
  );
  await waitFor(() => expect(saved).toHaveBeenCalled());
  const result = saved.mock.calls[0][0];
  first.unmount();
  render(
    <ProjectEditDialog project={result} onClose={vi.fn()} onSaved={vi.fn()} />,
  );
  expect(screen.getByTitle("/new-folder")).toBeVisible();
  expect(screen.getAllByRole("radio")).toHaveLength(3);
});
it("does not discard a name edit or add the browsed folder when folder selection is cancelled", async () => {
  const saved = vi.fn(),
    closed = vi.fn();
  api.rename.mockResolvedValue({ ...project, display_name: "Renamed" });
  render(
    <ProjectEditDialog project={project} onClose={closed} onSaved={saved} />,
  );
  fireEvent.change(
    screen.getByRole("textbox", { name: "projects.displayName" }),
    { target: { value: "Renamed" } },
  );
  await chooseFolder("/new-folder");
  fireEvent.click(screen.getByText("Cancel"));
  expect(closed).not.toHaveBeenCalled();
  expect(screen.queryByTitle("/new-folder")).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "common.save" }));
  await waitFor(() =>
    expect(api.rename).toHaveBeenCalledWith("project", "Renamed"),
  );
  expect(api.editResources).not.toHaveBeenCalled();
});
it("rejects a normalized duplicate before changing the selected folder list", async () => {
  render(
    <ProjectEditDialog project={project} onClose={vi.fn()} onSaved={vi.fn()} />,
  );
  await chooseFolder("/app/.");
  api.browseDirectory.mockResolvedValue({
    path: "/app",
    parent_path: "/",
    roots: [],
    entries: [],
  });
  fireEvent.click(screen.getByRole("button", { name: "projects.addFolder" }));
  expect(await screen.findByRole("alert")).toHaveTextContent(
    "projects.folderAlreadyAdded",
  );
  expect(api.editResources).not.toHaveBeenCalled();
  expect(
    screen.queryByRole("button", { name: "common.save" }),
  ).not.toBeInTheDocument();
});
it("keeps the added folder visible after a rejected save so it can be retried", async () => {
  const saved = vi.fn();
  api.editResources
    .mockRejectedValueOnce(new Error("Project is busy"))
    .mockResolvedValueOnce(withFolder());
  render(
    <ProjectEditDialog project={project} onClose={vi.fn()} onSaved={saved} />,
  );
  await chooseFolder("/new-folder");
  fireEvent.click(screen.getByRole("button", { name: "projects.addFolder" }));
  await screen.findByTitle("/new-folder");
  fireEvent.click(screen.getByRole("button", { name: "common.save" }));
  expect(await screen.findByRole("alert")).toHaveTextContent("Project is busy");
  expect(saved).not.toHaveBeenCalled();
  expect(screen.getByTitle("/new-folder")).toBeVisible();
  fireEvent.click(screen.getByRole("button", { name: "common.save" }));
  await waitFor(() => expect(saved).toHaveBeenCalled());
});
it("cannot confirm a folder while a new browse destination is still loading", async () => {
  render(
    <ProjectEditDialog project={project} onClose={vi.fn()} onSaved={vi.fn()} />,
  );
  await chooseFolder("/new-folder");
  let finish!: (value: unknown) => void;
  api.browseDirectory.mockImplementation(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  fireEvent.click(
    screen.getByRole("button", { name: "projects.closeBrowser" }),
  );
  fireEvent.click(screen.getByRole("button", { name: "projects.browse" }));
  await waitFor(() =>
    expect(
      screen.getByRole("button", { name: "projects.addFolder" }),
    ).toBeDisabled(),
  );
  finish({ path: "/new-folder", parent_path: "/", roots: [], entries: [] });
  await waitFor(() =>
    expect(
      screen.getByRole("button", { name: "projects.addFolder" }),
    ).toBeEnabled(),
  );
});
