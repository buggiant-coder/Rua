import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import SessionsPage from "./index";
import api from "../../../api";
import { saveFile } from "@/utils/saveFile";
import { useSessions } from "./useSessions";
import type { Session } from "./components";
import { projectsApi } from "@/api/modules/projects";
import { projectChatsApi } from "@/api/modules/projectChats";
import type { ProjectDescriptor } from "@/api/types/project";

vi.mock("@/stores/agentStore", () => ({
  useAgentStore: () => ({ selectedAgent: "default" }),
}));
vi.mock("@/api/modules/projects", () => ({ projectsApi: { list: vi.fn() } }));
vi.mock("@/api/modules/projectChats", () => ({
  projectChatsApi: { getChat: vi.fn() },
}));
vi.mock("@/components/ProjectSelector", () => ({
  default: ({
    projects,
    onSelect,
    onSelectIndependent,
  }: {
    projects: ProjectDescriptor[];
    onSelect: (project: ProjectDescriptor) => void;
    onSelectIndependent: () => void;
  }) => (
    <div>
      {projects.map((project) => (
        <button key={project.project_id} onClick={() => onSelect(project)}>
          {project.display_name}
        </button>
      ))}
      <button onClick={onSelectIndependent}>Independent</button>
    </div>
  ),
}));

const {
  batchDeleteSessionsMock,
  deleteSessionMock,
  fetchSessionsMock,
  getSessionMock,
  saveFileMock,
  toastMock,
  updateSessionMock,
  navigateMock,
} = vi.hoisted(() => ({
  batchDeleteSessionsMock: vi.fn(),
  deleteSessionMock: vi.fn(),
  fetchSessionsMock: vi.fn(),
  getSessionMock: vi.fn(),
  saveFileMock: vi.fn(),
  toastMock: {
    error: vi.fn(),
    success: vi.fn(),
    warning: vi.fn(),
  },
  updateSessionMock: vi.fn(),
  navigateMock: vi.fn(),
}));

vi.mock("react-router-dom", () => ({
  useNavigate: () => navigateMock,
}));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string, params?: Record<string, unknown>) => {
      if (key === "sessions.batchDeleteConfirm") {
        return `Delete ${String(params?.count ?? "")} sessions`;
      }
      if (key === "sessions.exportSuccess") {
        return `Exported ${String(params?.count ?? "")}`;
      }
      return (
        {
          "common.batchDelete": "Batch delete",
          "common.loading": "Loading",
          "common.refresh": "Refresh",
          "common.selectedN": `${String(params?.count ?? "")} selected`,
          "common.selectAll": "Select all",
          "common.clearSelection": "Clear",
          "cronJobs.cancelText": "Cancel",
          "cronJobs.deleteText": "Delete",
          "sessions.batchDeleteDisabledTip": "Select sessions to export",
          "sessions.batchExport": "Batch export",
          "sessions.confirmDelete": "Confirm delete",
          "sessions.deleteConfirm": "Delete this session?",
          "sessions.emptyText": "No sessions",
          "sessions.exportEmpty": "No sessions to export",
          "sessions.exportFailed": "Export failed",
          "sessions.openChat": "Open chat",
          "sessions.title": "Sessions",
        }[key] ?? key
      );
    },
  }),
}));

vi.mock("../../../api", () => ({
  default: {
    getChat: getSessionMock,
  },
}));

vi.mock("@/utils/saveFile", () => ({
  getDownloadFolderHint: () => "Downloads/",
  saveFile: saveFileMock,
}));

vi.mock("./useSessions", () => ({
  useSessions: vi.fn(),
}));

vi.mock("@/components/Page", () => ({
  Page: ({ children }: { children: React.ReactNode }) => <div>{children}</div>,
  PageBody: ({ children }: { children: React.ReactNode }) => (
    <div>{children}</div>
  ),
}));

vi.mock("@/components/PageHeader", () => ({
  PageHeader: ({
    afterBreadcrumb,
    extra,
    items,
    moreMenuItems,
    onRefresh,
    showMoreMenu = true,
  }: {
    afterBreadcrumb?: React.ReactNode;
    extra?: React.ReactNode;
    items?: Array<{ title: string }>;
    moreMenuItems?: Array<{
      disabled?: boolean;
      key: string;
      label: React.ReactNode;
      onClick?: () => void;
    }>;
    onRefresh?: () => void;
    showMoreMenu?: boolean;
  }) => (
    <header>
      <h1>{items?.[0]?.title}</h1>
      {afterBreadcrumb}
      {extra}
      {showMoreMenu && (
        <>
          <button type="button" onClick={onRefresh}>
            Refresh
          </button>
          {moreMenuItems?.map((item) => (
            <button
              disabled={item.disabled}
              key={item.key}
              type="button"
              onClick={item.onClick}
            >
              {item.label}
            </button>
          ))}
        </>
      )}
    </header>
  ),
}));

vi.mock("@/components/EmptyState", () => ({
  EmptyState: ({ description }: { description?: string }) => (
    <div>{description}</div>
  ),
}));

vi.mock("@/ui", () => ({
  Btn: ({
    children,
    disabled,
    onClick,
    title,
  }: {
    children?: React.ReactNode;
    disabled?: boolean;
    onClick?: () => void;
    title?: string;
  }) => (
    <button disabled={disabled} title={title} type="button" onClick={onClick}>
      {children ?? title ?? "button"}
    </button>
  ),
  Dropdown: ({
    children,
    menu,
  }: {
    children: React.ReactNode;
    menu?: {
      items?: Array<{ key: string; label: string; onClick?: () => void }>;
    };
  }) => (
    <div>
      {children}
      {menu?.items?.map((item) => (
        <button key={item.key} type="button" onClick={item.onClick}>
          {item.label}
        </button>
      ))}
    </div>
  ),
  Modal: {
    confirm: vi.fn(({ onOk }) => onOk?.()),
  },
  Pagination: ({
    current,
    onChange,
  }: {
    current: number;
    onChange: (page: number) => void;
  }) => (
    <button type="button" onClick={() => onChange(current + 1)}>
      Next page
    </button>
  ),
  Tooltip: ({ children }: { children: React.ReactNode }) => <>{children}</>,
  toast: toastMock,
}));

vi.mock("./components", () => ({
  FilterBar: ({
    onSearchTitleChange,
    onSortFieldChange,
    onSortOrderChange,
    searchTitle,
  }: {
    onSearchTitleChange: (value: string) => void;
    onSortFieldChange: (value: "name") => void;
    onSortOrderChange: (value: "desc") => void;
    searchTitle: string;
  }) => (
    <div>
      <input
        placeholder="Search title"
        value={searchTitle}
        onChange={(event) => onSearchTitleChange(event.target.value)}
      />
      <button
        type="button"
        onClick={() => {
          onSortFieldChange("name");
          onSortOrderChange("desc");
        }}
      >
        Sort name desc
      </button>
    </div>
  ),
  SessionDrawer: ({
    editingSession,
    onSubmit,
    open,
  }: {
    editingSession: { id: string } | null;
    onSubmit: (session: { id: string; name: string }) => void;
    open: boolean;
  }) =>
    open ? (
      <button
        type="button"
        onClick={() =>
          onSubmit({ id: editingSession?.id ?? "missing", name: "Renamed" })
        }
      >
        Submit session drawer
      </button>
    ) : null,
  SessionList: ({
    batchMode,
    onOpenChat,
    onDelete,
    onEdit,
    onSelectChange,
    selectedKeys,
    sessions,
  }: {
    batchMode: boolean;
    onOpenChat: (id: string) => void;
    onDelete: (id: string) => void;
    onEdit: (session: { id: string; name: string }) => void;
    onSelectChange: (ids: string[]) => void;
    selectedKeys: string[];
    sessions: Array<{ id: string; name: string }>;
  }) => (
    <div>
      {sessions.map((session) => (
        <div key={session.id}>
          <span>
            session:{session.id}:{session.name}:{String(batchMode)}:
            {String(selectedKeys.includes(session.id))}
          </span>
          <button type="button" onClick={() => onEdit(session)}>
            Edit {session.id}
          </button>
          <button type="button" onClick={() => onOpenChat(session.id)}>
            Open {session.id}
          </button>
          <button type="button" onClick={() => onDelete(session.id)}>
            Delete {session.id}
          </button>
          <button type="button" onClick={() => onSelectChange([session.id])}>
            Select {session.id}
          </button>
        </div>
      ))}
    </div>
  ),
}));

const sessions: Session[] = [
  {
    id: "a",
    user_id: "u1",
    channel: "console",
    name: "Alpha",
    created_at: "2026-06-18T00:00:00Z",
    updated_at: "2026-06-19T00:00:00Z",
  },
  {
    id: "b",
    user_id: "u2",
    channel: "dingtalk",
    name: "Beta",
    created_at: "2026-06-17T00:00:00Z",
    updated_at: "2026-06-17T00:00:00Z",
  },
];

const mockedUseSessions = vi.mocked(useSessions);
const mockedApi = vi.mocked(api);
const mockedSaveFile = vi.mocked(saveFile);

describe("SessionsPage", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(projectsApi.list).mockResolvedValue([
      {
        project_id: "project-1",
        display_name: "Project One",
        resource_device_id: "device",
        display_path: "/project",
        lifecycle: "active",
        created_at: "",
        updated_at: "",
      },
    ]);
    vi.mocked(projectChatsApi.getChat).mockResolvedValue({
      messages: [],
    } as Awaited<ReturnType<typeof projectChatsApi.getChat>>);
    localStorage.clear();
    batchDeleteSessionsMock.mockResolvedValue(true);
    deleteSessionMock.mockResolvedValue(true);
    fetchSessionsMock.mockResolvedValue(undefined);
    getSessionMock.mockResolvedValue({ messages: [{ role: "user" }] });
    mockedSaveFile.mockResolvedValue({ ok: true });
    updateSessionMock.mockResolvedValue(true);
    mockedUseSessions.mockReturnValue({
      sessions,
      loading: false,
      page: 1,
      pageSize: 1,
      unpinnedTotal: 2,
      updateSession: updateSessionMock,
      deleteSession: deleteSessionMock,
      fetchSessions: fetchSessionsMock,
      batchDeleteSessions: batchDeleteSessionsMock,
    } as ReturnType<typeof useSessions>);
  });

  it("filters current Rust session rows by title and persists sort state", async () => {
    render(<SessionsPage />);
    await screen.findByRole("button", { name: "Project One" });

    fireEvent.change(screen.getByPlaceholderText("Search title"), {
      target: { value: "beta" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Sort name desc" }));

    expect(screen.queryByText(/session:a/)).not.toBeInTheDocument();
    expect(screen.getByText(/session:b:Beta/)).toBeVisible();
    expect(localStorage.getItem("rua.sessions.sortField")).toBe("name");
    expect(localStorage.getItem("rua.sessions.sortOrder")).toBe("desc");
  });

  it("switches history scope, resets selection and editing, and discards the loader seed", async () => {
    const initialData = { agentId: "default", sessions, total: 2 };
    render(<SessionsPage initialData={initialData} />);
    await screen.findByRole("button", { name: "Project One" });
    fireEvent.click(screen.getByRole("button", { name: "Edit a" }));
    fireEvent.click(screen.getByRole("button", { name: "Batch delete" }));
    fireEvent.click(screen.getByRole("button", { name: "Select all" }));
    fireEvent.click(screen.getByRole("button", { name: "Project One" }));
    expect(mockedUseSessions).toHaveBeenLastCalledWith(undefined, "project-1");
    expect(screen.queryByText("2 selected")).not.toBeInTheDocument();
    expect(screen.queryByText("Submit session drawer")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Open b" }));
    expect(navigateMock).toHaveBeenLastCalledWith(
      "/projects/project-1/chats/b",
    );
    fireEvent.click(screen.getByRole("button", { name: "Independent" }));
    expect(mockedUseSessions).toHaveBeenLastCalledWith(undefined, undefined);
    fireEvent.click(screen.getByRole("button", { name: "Open b" }));
    expect(navigateMock).toHaveBeenLastCalledWith("/agents/default/chats/b");
  });

  it("exports Project history through its owning namespace", async () => {
    render(<SessionsPage />);
    fireEvent.click(await screen.findByRole("button", { name: "Project One" }));
    fireEvent.click(screen.getByRole("button", { name: "Batch export" }));
    fireEvent.click(screen.getByRole("button", { name: "Select b" }));
    fireEvent.click(screen.getAllByRole("button", { name: "button" })[1]);
    await waitFor(() => expect(mockedSaveFile).toHaveBeenCalled());
    expect(projectChatsApi.getChat).toHaveBeenCalledWith("project-1", "b");
    expect(mockedApi.getChat).not.toHaveBeenCalled();
  });

  it("edits and deletes sessions through the hook boundary", async () => {
    render(<SessionsPage />);

    fireEvent.click(screen.getByRole("button", { name: "Edit a" }));
    fireEvent.click(
      screen.getByRole("button", { name: "Submit session drawer" }),
    );
    fireEvent.click(screen.getByRole("button", { name: "Delete b" }));

    await waitFor(() => {
      expect(updateSessionMock).toHaveBeenCalledWith("a", {
        name: "Renamed",
      });
      expect(deleteSessionMock).toHaveBeenCalledWith("b");
    });
  });

  it("opens a selected session in the chat page", async () => {
    render(<SessionsPage />);
    await screen.findByRole("button", { name: "Project One" });

    fireEvent.click(screen.getByRole("button", { name: "Open b" }));

    expect(navigateMock).toHaveBeenCalledWith("/agents/default/chats/b");
  });

  it("refreshes and paginates via Rust session list pagination", async () => {
    render(<SessionsPage />);
    await screen.findByRole("button", { name: "Project One" });

    fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
    fireEvent.click(screen.getByRole("button", { name: "Next page" }));

    expect(fetchSessionsMock).toHaveBeenCalledWith(1);
    expect(fetchSessionsMock).toHaveBeenCalledWith(2);
  });

  it("batch-deletes all visible session ids from batch mode", async () => {
    render(<SessionsPage />);

    fireEvent.click(screen.getByRole("button", { name: "Batch delete" }));
    fireEvent.click(screen.getByRole("button", { name: "Select all" }));
    expect(screen.getByText("2 selected")).toBeVisible();
    fireEvent.click(screen.getAllByRole("button", { name: "button" })[1]);

    await waitFor(() => {
      expect(batchDeleteSessionsMock).toHaveBeenCalledWith(["a", "b"]);
    });
  });

  it("exports selected sessions with fetched detail payloads", async () => {
    render(<SessionsPage />);

    fireEvent.click(screen.getByRole("button", { name: "Batch export" }));
    fireEvent.click(screen.getByRole("button", { name: "Select b" }));
    fireEvent.click(screen.getAllByRole("button", { name: "button" })[1]);

    await waitFor(() => {
      expect(mockedApi.getChat).toHaveBeenCalledWith("b", "default");
      expect(mockedSaveFile).toHaveBeenCalledWith(
        expect.any(Blob),
        expect.stringMatching(/^sessions-.*\.json$/),
      );
      expect(toastMock.success).toHaveBeenCalledWith(
        expect.stringContaining("Exported 1"),
      );
    });
  });
});
