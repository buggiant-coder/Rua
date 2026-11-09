import {
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { MemoryRouter, Route, Routes, useLocation } from "react-router-dom";

import * as drafts from "../Chat/drafts";
import ProjectsPage, { loader } from "./index";
import { subscribeConversationNavigation } from "@/layouts/conversationNavigationEvents";
import {
  lastProjectChatId,
  lastProjectId,
  projectChatDraftKey,
  rememberProjectChat,
  setProjectLastSelectedAgent,
} from "./projectChatNavigation";

const { devicesApiMock, projectChatsApiMock, projectsApiMock, toastMock } =
  vi.hoisted(() => ({
    devicesApiMock: {
      listDevices: vi.fn(),
    },
    projectChatsApiMock: {
      getChat: vi.fn(),
    },
    projectsApiMock: {
      getSidebar: vi.fn(),
      updateSidebar: vi.fn(),
      list: vi.fn(),
      browseDirectory: vi.fn(),
      create: vi.fn(),
      rename: vi.fn(),
      archive: vi.fn(),
      getLifecyclePreview: vi.fn(),
      reopen: vi.fn(),
      permanentlyDelete: vi.fn(),
    },
    toastMock: {
      success: vi.fn(),
      warning: vi.fn(),
      error: vi.fn(),
    },
  }));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (_key: string, fallback?: unknown) =>
      typeof fallback === "string" ? fallback : _key,
  }),
}));

vi.mock("@/api/modules/projects", () => ({
  projectsApi: projectsApiMock,
}));

vi.mock("@/api/modules/projectChats", () => ({
  projectChatsApi: projectChatsApiMock,
}));

vi.mock("@/api/modules/sync", () => ({
  devicesApi: devicesApiMock,
}));

vi.mock("@/ui", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ui")>();
  return { ...actual, toast: toastMock };
});

const onlineDevice = {
  device_id: "device-a",
  public_key: "public",
  friendly_name: "Mac",
  label: "macOS",
  added_at_ms: 1,
  last_seen_ms: 1,
  is_self: true,
  connectivity_state: "connected" as const,
  session_path: null,
  session_rtt_ms: null,
};

describe("ProjectsPage", () => {
  const navigationChanged = vi.fn();
  let unsubscribeNavigation = () => {};
  afterEach(() => unsubscribeNavigation());
  beforeEach(() => {
    vi.restoreAllMocks();
    vi.clearAllMocks();
    unsubscribeNavigation = subscribeConversationNavigation(navigationChanged);
    sessionStorage.clear();
    localStorage.clear();
    devicesApiMock.listDevices.mockResolvedValue([onlineDevice]);
    projectsApiMock.getSidebar.mockResolvedValue({
      revision: "saved",
      sort: "manual",
      items: {},
    });
    projectsApiMock.list.mockResolvedValue([]);
    projectChatsApiMock.getChat.mockResolvedValue({ messages: [], meta: {} });
    projectsApiMock.browseDirectory.mockResolvedValue({
      device_id: "device-a",
      path: "/repos/rua",
      parent_path: "/repos",
      proposed_name: "rua",
      roots: [],
      entries: [
        {
          name: "src",
          path: "/repos/rua/src",
          kind: "directory",
          is_symlink: false,
        },
      ],
    });
    projectsApiMock.create.mockResolvedValue({
      project: {
        project_id: "project-1",
        display_name: "rua",
        resource_device_id: "device-a",
        display_path: "/repos/rua",
        lifecycle: "active",
        created_at: "2026-08-01T00:00:00Z",
        updated_at: "2026-08-01T00:00:00Z",
      },
      overlap_warnings: [],
    });
    projectsApiMock.reopen.mockImplementation(async () => ({
      project: {
        project_id: "project-archived",
        display_name: "Archived repo",
        resource_device_id: "device-a",
        display_path: "/repos/archived",
        lifecycle: "active",
        created_at: "2026-08-01T00:00:00Z",
        updated_at: "2026-08-01T01:00:00Z",
      },
      changed: true,
    }));
    projectsApiMock.getLifecyclePreview.mockResolvedValue({
      resource_device_revoked: false,
      impact: {
        affected_chat_count: 0,
        active_turn_count: 0,
        queued_input_count: 0,
        background_task_count: 0,
        pending_approval_count: 0,
        pending_prompt_count: 0,
        active_acp_count: 0,
      },
    });
    projectsApiMock.archive.mockImplementation(async (projectId: string) => ({
      project: {
        project_id: projectId,
        display_name: "Active repo",
        resource_device_id: "device-a",
        display_path: "/repos/active",
        lifecycle: "archived",
        created_at: "2026-08-01T00:00:00Z",
        updated_at: "2026-08-01T01:00:00Z",
      },
      changed: true,
    }));
    projectsApiMock.permanentlyDelete.mockResolvedValue({
      project_id: "project-archived",
      deleted_chat_count: 1,
      changed: true,
    });
  });

  it("creates a Project from one dialog and derives the editable name", async () => {
    vi.spyOn(crypto, "randomUUID").mockReturnValue("lazy-project-chat");
    renderProjects({ projects: [], devices: [onlineDevice] });

    fireEvent.click(screen.getAllByRole("button", { name: "New Project" })[0]);
    const pathInput = screen.getByPlaceholderText("~");
    fireEvent.change(pathInput, { target: { value: "/repos/rua" } });
    fireEvent.click(screen.getByRole("button", { name: "Browse directory" }));

    await waitFor(() => {
      expect(projectsApiMock.browseDirectory).toHaveBeenCalledWith(
        "device-a",
        "/repos/rua",
      );
    });
    expect(screen.getAllByRole("textbox")[0]).toHaveValue("rua");
    expect(screen.getByText("src")).toBeVisible();

    fireEvent.click(screen.getByRole("button", { name: "Create Project" }));

    await waitFor(() => {
      expect(projectsApiMock.create).toHaveBeenCalledWith(
        "device-a",
        "/repos/rua",
        "rua",
        0,
      );
    });
    expect(await screen.findByText("rua")).toBeVisible();
    expect(toastMock.success).toHaveBeenCalledWith("Project created");
    expect(navigationChanged).toHaveBeenCalledExactlyOnceWith(undefined);
    await waitFor(() =>
      expect(screen.getByTestId("location")).toHaveTextContent(
        "/projects/project-1/chats/lazy-project-chat",
      ),
    );
    expect(
      sessionStorage.getItem(
        "rua_project_lazy_chat_project-1:lazy-project-chat",
      ),
    ).toBeNull();
  });

  it("browses from home when the directory input is empty", async () => {
    renderProjects({ projects: [], devices: [onlineDevice] });

    fireEvent.click(screen.getAllByRole("button", { name: "New Project" })[0]);
    const browseButton = screen.getByRole("button", {
      name: "Browse directory",
    });

    expect(screen.getByPlaceholderText("~")).toHaveValue("");
    expect(browseButton).toBeEnabled();
    fireEvent.click(browseButton);

    await waitFor(() => {
      expect(projectsApiMock.browseDirectory).toHaveBeenCalledWith(
        "device-a",
        "~",
      );
    });
  });

  it("places the display name before the directory and completes paths on the resource Device", async () => {
    projectsApiMock.browseDirectory.mockResolvedValueOnce({
      device_id: "device-a",
      path: "/repos",
      parent_path: "/",
      proposed_name: "repos",
      roots: [],
      entries: [
        {
          name: "rua",
          path: "/repos/rua",
          kind: "directory",
          is_symlink: false,
        },
        {
          name: "notes.txt",
          path: "/repos/notes.txt",
          kind: "file",
          is_symlink: false,
        },
      ],
    });
    renderProjects({ projects: [], devices: [onlineDevice] });

    fireEvent.click(screen.getAllByRole("button", { name: "New Project" })[0]);
    const displayNameInput = screen.getAllByRole("textbox")[0];
    const pathInput = screen.getByPlaceholderText("~");
    expect(
      displayNameInput.compareDocumentPosition(pathInput) &
        Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();

    fireEvent.change(pathInput, { target: { value: "/repos/ru" } });
    const suggestion = await screen.findByRole("option", { name: /rua/ });
    expect(projectsApiMock.browseDirectory).toHaveBeenCalledWith(
      "device-a",
      "/repos",
    );
    expect(screen.queryByRole("option", { name: /notes\.txt/ })).toBeNull();

    fireEvent.click(suggestion);
    expect(pathInput).toHaveValue("/repos/rua");
    expect(displayNameInput).toHaveValue("rua");
  });

  it("keeps duplicate navigation explicit", async () => {
    vi.spyOn(crypto, "randomUUID").mockReturnValue("duplicate-lazy-chat");
    projectsApiMock.create.mockRejectedValueOnce(
      new Error(
        'This directory is already bound - {"detail":"This directory is already bound","code":"project_binding_duplicate","project":{"project_id":"project-existing","display_name":"Existing","resource_device_id":"device-a","display_path":"/repos/rua","lifecycle":"active","created_at":"2026-08-01T00:00:00Z","updated_at":"2026-08-01T00:00:00Z"},"actions":["view"]}',
      ),
    );
    renderProjects({ projects: [], devices: [onlineDevice] });

    fireEvent.click(screen.getAllByRole("button", { name: "New Project" })[0]);
    fireEvent.change(screen.getByPlaceholderText("~"), {
      target: { value: "/repos/rua" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Create Project" }));

    expect(await screen.findByText("Project already exists")).toBeVisible();
    expect(screen.getByText("Existing")).toBeVisible();
    fireEvent.click(screen.getByRole("button", { name: "View" }));
    await waitFor(() =>
      expect(screen.getByTestId("location")).toHaveTextContent(
        "/projects/project-existing/chats/duplicate-lazy-chat",
      ),
    );
  });

  it("renders the shared Device Session connectivity in Project rows", () => {
    renderProjects({
      devices: [onlineDevice],
      projects: [
        {
          project_id: "project-missing",
          display_name: "Missing repo",
          resource_device_id: "device-a",
          display_path: "/repos/missing",
          lifecycle: "active",
          created_at: "2026-08-01T00:00:00Z",
          updated_at: "2026-08-01T00:00:00Z",
        },
        {
          project_id: "project-revoked",
          display_name: "Removed device repo",
          resource_device_id: "device-removed",
          display_path: "/repos/removed",
          lifecycle: "active",
          created_at: "2026-08-01T00:00:00Z",
          updated_at: "2026-08-01T00:00:00Z",
        },
      ],
    });

    expect(screen.getAllByText("Connected")).toHaveLength(2);
    expect(screen.getAllByText("Not connected")).toHaveLength(2);
  });

  it("opens an active Project with its validated browser-local Chat hint", async () => {
    const activeProject = {
      project_id: "project-active",
      display_name: "Active repo",
      resource_device_id: "device-a",
      display_path: "/repos/active",
      lifecycle: "active" as const,
      created_at: "2026-08-01T00:00:00Z",
      updated_at: "2026-08-01T00:00:00Z",
    };
    rememberProjectChat(activeProject.project_id, "chat-saved");
    renderProjects({ projects: [activeProject], devices: [onlineDevice] });

    fireEvent.click(screen.getByRole("button", { name: /Active repo/ }));

    await waitFor(() =>
      expect(screen.getByTestId("location")).toHaveTextContent(
        "/projects/project-active/chats/chat-saved",
      ),
    );
    expect(projectChatsApiMock.getChat).not.toHaveBeenCalled();
  });

  it("archives an active Project from Overview", async () => {
    const activeProject = {
      project_id: "project-active",
      display_name: "Active repo",
      resource_device_id: "device-a",
      display_path: "/repos/active",
      lifecycle: "active" as const,
      created_at: "2026-08-01T00:00:00Z",
      updated_at: "2026-08-01T00:00:00Z",
    };
    renderProjects({ projects: [activeProject], devices: [onlineDevice] });

    fireEvent.click(screen.getByRole("button", { name: "Archive Project" }));
    expect(
      screen.getByRole("heading", { name: "Archive Project" }),
    ).toBeVisible();
    await waitFor(() =>
      expect(
        within(screen.getByRole("dialog")).getByRole("button", {
          name: "Archive Project",
        }),
      ).toBeEnabled(),
    );
    fireEvent.click(
      within(screen.getByRole("dialog")).getByRole("button", {
        name: "Archive Project",
      }),
    );

    await waitFor(() =>
      expect(projectsApiMock.archive).toHaveBeenCalledWith("project-active"),
    );
    expect((await screen.findAllByText("Archived")).length).toBeGreaterThan(0);
    expect(navigationChanged).toHaveBeenCalledExactlyOnceWith(undefined);
  });

  it("restores the last valid Project and Chat from the unqualified entry", async () => {
    const activeProject = {
      project_id: "project-active",
      display_name: "Active repo",
      resource_device_id: "device-a",
      display_path: "/repos/active",
      lifecycle: "active" as const,
      created_at: "2026-08-01T00:00:00Z",
      updated_at: "2026-08-01T00:00:00Z",
    };
    rememberProjectChat(activeProject.project_id, "chat-saved");
    projectsApiMock.list.mockResolvedValue([activeProject]);

    const response = await loader({
      request: new Request("http://localhost/projects"),
      params: {},
    } as never);

    expect(response).toBeInstanceOf(Response);
    expect((response as Response).status).toBe(302);
    expect((response as Response).headers.get("Location")).toBe(
      "/projects/project-active/chats/chat-saved",
    );
    expect(devicesApiMock.listDevices).not.toHaveBeenCalled();
  });

  it("opens an unmaterialized Chat for the first active Project when no hint is valid", async () => {
    const activeProject = {
      project_id: "project-active",
      display_name: "Active repo",
      resource_device_id: "device-a",
      display_path: "/repos/active",
      lifecycle: "active" as const,
      created_at: "2026-08-01T00:00:00Z",
      updated_at: "2026-08-01T00:00:00Z",
    };
    projectsApiMock.list.mockResolvedValue([activeProject]);
    vi.spyOn(crypto, "randomUUID").mockReturnValue("restored-lazy-chat");

    const response = await loader({
      request: new Request("http://localhost/projects"),
      params: {},
    } as never);

    expect((response as Response).headers.get("Location")).toBe(
      "/projects/project-active/chats/restored-lazy-chat",
    );
  });

  it("shows Overview only when no active Project remains", async () => {
    const archivedProject = {
      project_id: "project-archived",
      display_name: "Archived repo",
      resource_device_id: "device-a",
      display_path: "/repos/archived",
      lifecycle: "archived" as const,
      created_at: "2026-08-01T00:00:00Z",
      updated_at: "2026-08-01T00:00:00Z",
    };
    projectsApiMock.list.mockResolvedValue([archivedProject]);

    const data = await loader({
      request: new Request("http://localhost/projects"),
      params: {},
    } as never);

    expect(data).toEqual({
      projects: [archivedProject],
      devices: [onlineDevice],
    });
  });

  it("reopens from the Archived section and uses the normal last-Chat route", async () => {
    const archivedProject = {
      project_id: "project-archived",
      display_name: "Archived repo",
      resource_device_id: "device-a",
      display_path: "/repos/archived",
      lifecycle: "archived" as const,
      created_at: "2026-08-01T00:00:00Z",
      updated_at: "2026-08-01T00:00:00Z",
    };
    rememberProjectChat(archivedProject.project_id, "chat-saved");
    renderProjects({ projects: [archivedProject], devices: [onlineDevice] });

    fireEvent.click(screen.getByRole("button", { name: "Reopen" }));

    await waitFor(() =>
      expect(projectsApiMock.reopen).toHaveBeenCalledWith("project-archived"),
    );
    await waitFor(() =>
      expect(screen.getByTestId("location")).toHaveTextContent(
        "/projects/project-archived/chats/chat-saved",
      ),
    );
    expect(screen.queryByText(/Restore/)).not.toBeInTheDocument();
  });

  it("permanently deletes from Archived without typed-name ceremony", async () => {
    const archivedProject = {
      project_id: "project-archived",
      display_name: "Archived repo",
      resource_device_id: "device-a",
      display_path: "/repos/archived",
      lifecycle: "archived" as const,
      created_at: "2026-08-01T00:00:00Z",
      updated_at: "2026-08-01T00:00:00Z",
    };
    rememberProjectChat(archivedProject.project_id, "chat-saved");
    setProjectLastSelectedAgent(archivedProject.project_id, "code");
    drafts.save(
      projectChatDraftKey(archivedProject.project_id, "chat-saved"),
      "delete me",
    );
    renderProjects({ projects: [archivedProject], devices: [onlineDevice] });

    fireEvent.click(
      screen.getByRole("button", { name: "Permanently Delete Project" }),
    );

    expect(
      screen.getByText(
        "Project Chats and Rua-owned data will be permanently deleted. The directory and its files will remain untouched.",
      ),
    ).toBeVisible();
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Permanently Delete" }));

    await waitFor(() =>
      expect(projectsApiMock.permanentlyDelete).toHaveBeenCalledWith(
        "project-archived",
      ),
    );
    await waitFor(() =>
      expect(screen.queryByText("Archived repo")).not.toBeInTheDocument(),
    );
    expect(lastProjectId()).toBeNull();
    expect(lastProjectChatId("project-archived")).toBeNull();
    expect(
      drafts.load(projectChatDraftKey("project-archived", "chat-saved")),
    ).toBe("");
  });
});

function LocationProbe() {
  return <div data-testid="location">{useLocation().pathname}</div>;
}

function renderProjects(
  initialData: Parameters<typeof ProjectsPage>[0]["initialData"],
) {
  return render(
    <MemoryRouter initialEntries={["/projects"]}>
      <LocationProbe />
      <Routes>
        <Route path="*" element={<ProjectsPage initialData={initialData} />} />
      </Routes>
    </MemoryRouter>,
  );
}
