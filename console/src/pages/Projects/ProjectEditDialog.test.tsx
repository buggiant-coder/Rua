import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { describe, it, expect, vi, beforeEach } from "vitest";
import ProjectEditDialog from "./ProjectEditDialog";

const api = vi.hoisted(() => ({
  get: vi.fn(),
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
vi.mock("@/ui", async (load) => ({
  ...(await load<typeof import("@/ui")>()),
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
const one = {
  directory_id: "app",
  device_id: "desk",
  resource_root: "/shared",
  display_name: "Initial",
};
const project = {
  project_id: "project",
  display_name: "Photos",
  resources: { revision: 4, directories: [one] },
  lifecycle: "active" as const,
  created_at: "",
  updated_at: "",
};
const devices = [
  {
    device_id: "desk",
    friendly_name: "Desktop",
    is_self: true,
    connectivity_state: "connected",
  },
  {
    device_id: "phone",
    friendly_name: "Phone",
    is_self: false,
    connectivity_state: "connected",
  },
];
function open(online = devices) {
  const saved = vi.fn();
  render(
    <ProjectEditDialog
      project={project}
      devices={online as never}
      onSaved={saved}
      onClose={vi.fn()}
    />,
  );
  return saved;
}
const click = (name: string) =>
  fireEvent.click(screen.getByRole("button", { name }));
const save = () => click("common.save");
const folder = (name: string) => screen.getByRole("button", { name });
async function choose(path: string, device?: string, replace = false) {
  click(replace ? "projects.changeFolder" : "projects.addFolder");
  await waitFor(() => expect(api.browseDirectory).toHaveBeenCalled());
  if (device) {
    fireEvent.click(screen.getByRole("combobox"));
    fireEvent.click(await screen.findByRole("option", { name: device }));
  }
  fireEvent.change(
    screen.getByRole("textbox", { name: "projects.folderPath" }),
    { target: { value: path } },
  );
  const button = folder(
    replace ? "projects.changeFolder" : "projects.addFolder",
  );
  await waitFor(() => expect(button).toBeEnabled());
  fireEvent.click(button);
}
beforeEach(() => {
  vi.clearAllMocks();
  api.browseDirectory.mockImplementation(
    async (_device: string, path: string) => ({
      path: path === "/shared/." ? "/shared" : path,
      parent_path: null,
      roots: [],
      entries: [],
    }),
  );
  api.editResources.mockImplementation(async (_id, revision, dirs) => ({
    project: {
      ...project,
      resources: {
        revision: revision + 1,
        directories: dirs.map((d: any, i: number) => ({
          ...d,
          directory_id: d.directory_id ?? "new-" + i,
        })),
      },
    },
    overlap_warnings: [],
  }));
  api.rename.mockImplementation(async (_id, name) => ({
    ...project,
    display_name: name,
  }));
});
describe("Project resources", () => {
  it("adds the same absolute path on another device and accepts the saved association ID", async () => {
    const saved = open();
    await choose("/shared", "Phone");
    await waitFor(() =>
      expect(screen.getAllByTitle("/shared")).toHaveLength(2),
    );
    save();
    await waitFor(() => expect(saved).toHaveBeenCalled());
    expect(api.editResources).toHaveBeenCalledWith("project", 4, [
      one,
      { device_id: "phone", resource_root: "/shared" },
    ]);
    expect(saved.mock.calls[0][0].resources.directories[1].directory_id).toBe(
      "new-1",
    );
  });
  it("keeps the existing association and revision when the current folder is confirmed", async () => {
    const saved = open();
    click("projects.changeFolder");
    await waitFor(() =>
      expect(api.browseDirectory).toHaveBeenCalledWith("desk", "/shared"),
    );
    expect(
      screen.getByRole("textbox", { name: "projects.folderPath" }),
    ).toHaveValue("/shared");
    await waitFor(() => expect(folder("projects.changeFolder")).toBeEnabled());
    click("projects.changeFolder");
    await screen.findByTitle("/shared");
    save();
    await waitFor(() => expect(saved).toHaveBeenCalledWith(project));
    expect(api.editResources).not.toHaveBeenCalled();
  });

  it("retries the selected device after it comes online while the editor is open", async () => {
    let phoneOnline = false;
    open([devices[0], { ...devices[1], connectivity_state: "not_connected" }]);
    api.browseDirectory.mockImplementation(
      async (device: string, path: string) => {
        if (device === "phone" && !phoneOnline)
          throw new Error("Phone is offline");
        return { path, roots: [], entries: [], parent_path: null };
      },
    );
    await choose("/pictures", "Phone · projects.offline");
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Phone is offline",
    );
    expect(
      screen.getByRole("textbox", { name: "projects.folderPath" }),
    ).toHaveValue("/pictures");
    phoneOnline = true;
    click("projects.addFolder");
    await screen.findByTitle("/pictures");
    save();
    await waitFor(() =>
      expect(api.editResources).toHaveBeenCalledWith("project", 4, [
        one,
        { device_id: "phone", resource_root: "/pictures" },
      ]),
    );
    expect(api.browseDirectory).not.toHaveBeenCalledWith("desk", "/pictures");
  });

  it("replaces a path with a new association and retains the resource label", async () => {
    open();
    await choose("/replacement", undefined, true);
    await screen.findByTitle("/replacement");
    save();
    await waitFor(() =>
      expect(api.editResources).toHaveBeenCalledWith("project", 4, [
        {
          device_id: "desk",
          resource_root: "/replacement",
          display_name: "Initial",
        },
      ]),
    );
  });
  it("rejects a duplicate normalized location before saving and cancels without changing the set", async () => {
    open();
    await choose("/shared/.");
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "projects.folderAlreadyAdded",
    );
    click("Cancel");
    save();
    expect(api.editResources).not.toHaveBeenCalled();
  });
  it("renames an offline resource with its original id, without browsing or changing its location", async () => {
    open([]);
    fireEvent.change(
      screen.getByRole("textbox", { name: "projects.resourceName" }),
      { target: { value: "Films" } },
    );
    save();
    await waitFor(() =>
      expect(api.editResources).toHaveBeenCalledWith("project", 4, [
        { ...one, display_name: "Films" },
      ]),
    );
    expect(api.browseDirectory).not.toHaveBeenCalled();
  });
  it("detaches the last offline resource and preserves the Project", async () => {
    const saved = open([]);
    click("projects.detachFolder");
    save();
    await waitFor(() =>
      expect(saved).toHaveBeenCalledWith({
        ...project,
        resources: { revision: 5, directories: [] },
      }),
    );
    expect(api.browseDirectory).not.toHaveBeenCalled();
  });
  it("keeps pending changes available after a failed save", async () => {
    api.editResources.mockRejectedValueOnce(new Error("Device offline"));
    const saved = open();
    await choose("/new-folder");
    await screen.findByTitle("/new-folder");
    save();
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Device offline",
    );
    expect(screen.getByTitle("/new-folder")).toBeVisible();
    expect(saved).not.toHaveBeenCalled();
    save();
    await waitFor(() => expect(saved).toHaveBeenCalled());
    expect(api.editResources.mock.calls[0]).toEqual(
      api.editResources.mock.calls[1],
    );
  });
  it("reloads a conflicting revision explicitly before submitting against the latest set", async () => {
    api.editResources.mockRejectedValueOnce(
      new Error("stale resource revision"),
    );
    api.get.mockResolvedValue({
      ...project,
      resources: {
        revision: 6,
        directories: [{ ...one, display_name: "Remote" }],
      },
    });
    open();
    click("projects.detachFolder");
    save();
    await screen.findByRole("alert");
    expect(folder("common.save")).toBeDisabled();
    click("projects.reloadResources");
    await waitFor(() =>
      expect(
        screen.getByRole("textbox", { name: "projects.resourceName" }),
      ).toHaveValue("Remote"),
    );
    click("projects.detachFolder");
    save();
    await waitFor(() =>
      expect(api.editResources).toHaveBeenLastCalledWith("project", 6, []),
    );
  });
  it("preserves unsaved Project name changes when folder selection is cancelled", async () => {
    open();
    fireEvent.change(
      screen.getByRole("textbox", { name: "projects.displayName" }),
      { target: { value: "Trip" } },
    );
    click("projects.addFolder");
    click("Cancel");
    save();
    await waitFor(() =>
      expect(api.rename).toHaveBeenCalledWith("project", "Trip"),
    );
    expect(api.editResources).not.toHaveBeenCalled();
  });
});
