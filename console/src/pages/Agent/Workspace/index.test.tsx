import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import WorkspacePage from "./index";

const {
  agentsApiMock,
  fetchFilesMock,
  handleFileClickMock,
  handleSubdirFileClickMock,
  promptLibraryApiMock,
  saveFileMock,
  toastMock,
} = vi.hoisted(() => ({
  agentsApiMock: {
    deleteAgentFile: vi.fn(),
    deleteAgentSubdirFile: vi.fn(),
    downloadAgentWorkspace: vi.fn(),
    listAgents: vi.fn(),
    uploadAgentWorkspace: vi.fn(),
  },
  fetchFilesMock: vi.fn(),
  handleFileClickMock: vi.fn(),
  handleSubdirFileClickMock: vi.fn(),
  promptLibraryApiMock: {
    broadcast: vi.fn(),
    sync: vi.fn(),
  },
  saveFileMock: vi.fn(),
  toastMock: {
    error: vi.fn(),
    success: vi.fn(),
    warning: vi.fn(),
  },
}));

vi.mock("../../../api/modules/agents", () => ({
  agentsApi: agentsApiMock,
}));

vi.mock("../../../api/modules/promptLibrary", () => ({
  promptLibraryApi: promptLibraryApiMock,
}));

vi.mock("../../../stores/agentStore", () => ({
  useAgentStore: () => ({
    agents: [],
    selectedAgent: "workspace-agent",
    setAgents: vi.fn(),
    setSelectedAgent: vi.fn(),
  }),
}));

vi.mock("../../../hooks/useHistoryDismiss", () => ({
  useHistoryDismiss: vi.fn(),
}));

vi.mock("@/utils/isMobile", () => ({ isMobile: () => false }));

vi.mock("@/utils/saveFile", () => ({
  getDownloadFolderHint: () => "Downloads/",
  saveFile: saveFileMock,
}));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string, params?: unknown) => {
      if (key === "common.selectedN") {
        const count =
          params && typeof params === "object" && "count" in params
            ? String((params as { count: unknown }).count)
            : "0";
        return `Selected ${count}`;
      }
      if (key === "workspace.fileSizeExceeded") {
        return "File too large";
      }
      return (
        {
          "common.cancel": "Cancel",
          "common.create": "Create",
          "common.export": "Export",
          "common.import": "Import",
          "common.refresh": "Refresh",
          "promptLibrary.conflictMessage": "Conflict",
          "promptLibrary.conflictTitle": "Conflict",
          "promptLibrary.overwrite": "Overwrite",
          "promptLibrary.renameCreate": "Rename",
          "workspace.batchDelete": "Batch delete",
          "workspace.deleteEnabledWarning": "Enabled files cannot be deleted",
          "workspace.deleteFailed": "Delete failed",
          "workspace.deleteSuccess": "Deleted",
          "workspace.downloadFailed": "Download failed",
          "workspace.downloadSuccess": "Downloaded",
          "workspace.loadFromLibrary": "Load from library",
          "workspace.syncToLibrary": "Sync to library",
          "workspace.title": "Workspace",
          "workspace.uploadFailed": "Upload failed",
          "workspace.uploadSuccess": "Uploaded",
          "workspace.zipOnly": "ZIP only",
          "workspace.zipUploadHint": "Upload zip",
        }[key] ?? key
      );
    },
  }),
}));

vi.mock("@/ui", () => ({
  Btn: ({
    children,
    disabled,
    onClick,
    variant,
  }: {
    children?: React.ReactNode;
    disabled?: boolean;
    onClick?: () => void;
    variant?: string;
  }) => (
    <button disabled={disabled} type="button" onClick={onClick}>
      {children ?? variant ?? "action"}
    </button>
  ),
  Drawer: ({ children, open }: { children: React.ReactNode; open: boolean }) =>
    open ? <div>{children}</div> : null,
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
  Modal: ({
    children,
    footer,
    open,
    title,
  }: {
    children?: React.ReactNode;
    footer?: React.ReactNode;
    open?: boolean;
    title?: React.ReactNode;
  }) =>
    open ? (
      <div>
        <h2>{title}</h2>
        {children}
        {footer}
      </div>
    ) : null,
  Popover: ({
    children,
  }: {
    children: React.ReactNode;
    content?: React.ReactNode;
  }) => <>{children}</>,
  Tooltip: ({ children }: { children: React.ReactNode }) => <>{children}</>,
  toast: toastMock,
}));

vi.mock("./components", () => ({
  FileEditor: ({
    isFileEnabled,
    onDelete,
    selectedFile,
  }: {
    isFileEnabled?: boolean;
    onDelete?: () => void;
    selectedFile: { filename: string } | null;
  }) => (
    <div>
      <span>editor:{selectedFile?.filename ?? "none"}</span>
      <span>enabled:{String(isFileEnabled)}</span>
      {onDelete && (
        <button type="button" onClick={onDelete}>
          Delete selected
        </button>
      )}
    </div>
  ),
  FileListPanel: ({
    batchDeleteMode,
    batchSelected,
    onFileClick,
    onSubdirFileClick,
    onToggleBatchSelect,
  }: {
    batchDeleteMode: boolean;
    batchSelected: Set<string>;
    onFileClick: (file: { filename: string }) => void;
    onSubdirFileClick: (
      parentFilename: string,
      file: { filename: string; subdir: string },
    ) => void;
    onToggleBatchSelect: (filename: string) => void;
  }) => (
    <div>
      <span>batch:{String(batchDeleteMode)}</span>
      <span>selected:{Array.from(batchSelected).join(",")}</span>
      <button
        type="button"
        onClick={() => onFileClick({ filename: "ROOT.md" })}
      >
        Open root
      </button>
      <button
        type="button"
        onClick={() =>
          onSubdirFileClick("AGENT.md", {
            filename: "zh.md",
            subdir: "AGENT",
          })
        }
      >
        Open subdir
      </button>
      <button type="button" onClick={() => onToggleBatchSelect("ROOT.md")}>
        Select root
      </button>
      <button type="button" onClick={() => onToggleBatchSelect("AGENT/zh.md")}>
        Select subdir
      </button>
    </div>
  ),
  LoadFromLibraryModal: () => null,
  SyncToLibraryModal: () => null,
  useAgentsData: () => ({
    autoRename: false,
    cancelCreateFile: vi.fn(),
    clearSelection: vi.fn(),
    commitNewFileName: vi.fn(),
    displayedLoadedFiles: ["ROOT.md", "AGENT/zh.md"],
    enabledFiles: ["ENABLED.md"],
    expandedDirs: new Set(),
    fetchFiles: fetchFilesMock,
    fetchSubdirFiles: vi.fn(),
    fileContent: "content",
    files: [
      { filename: "ROOT.md", size: 10 },
      { filename: "ENABLED.md", size: 10 },
      { filename: "AGENT.md", size: 20 },
    ],
    filesWithSubdirs: new Set(["AGENT.md"]),
    handleCreateSubdirFile: vi.fn(),
    handleFileClick: handleFileClickMock,
    handleReorderFiles: vi.fn(),
    handleReset: vi.fn(),
    handleSave: vi.fn(),
    handleSubdirFileClick: handleSubdirFileClickMock,
    handleToggleExpandAll: vi.fn(),
    handleToggleFileEnabled: vi.fn(),
    hasChanges: false,
    isCreatingNew: false,
    loading: false,
    selectedFile: { filename: "ROOT.md", size: 10 },
    setAutoRename: vi.fn(),
    setFileContent: vi.fn(),
    setSortMode: vi.fn(),
    sortMode: "name-asc",
    startCreateFile: vi.fn(),
    subdirFiles: {
      "AGENT.md": [{ filename: "zh.md", size: 5, subdir: "AGENT" }],
    },
    workspacePath: "/tmp/workspace",
  }),
}));

describe("WorkspacePage", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    agentsApiMock.deleteAgentFile.mockResolvedValue({});
    agentsApiMock.deleteAgentSubdirFile.mockResolvedValue({});
    agentsApiMock.downloadAgentWorkspace.mockResolvedValue({
      blob: new Blob(["zip"]),
      filename: "workspace.zip",
    });
    agentsApiMock.listAgents.mockResolvedValue({ agents: [] });
    agentsApiMock.uploadAgentWorkspace.mockResolvedValue({ success: true });
    saveFileMock.mockResolvedValue({ ok: true });
  });

  it("batch-deletes root and subdir workspace files through Rust agent APIs", async () => {
    render(<WorkspacePage />);

    fireEvent.click(screen.getByRole("button", { name: "Batch delete" }));
    fireEvent.click(screen.getByRole("button", { name: "Select root" }));
    fireEvent.click(screen.getByRole("button", { name: "Select subdir" }));
    expect(screen.getByText("Selected 2")).toBeVisible();
    fireEvent.click(screen.getByRole("button", { name: "danger-filled" }));

    await waitFor(() => {
      expect(agentsApiMock.deleteAgentFile).toHaveBeenCalledWith(
        "workspace-agent",
        "ROOT.md",
        false,
      );
      expect(agentsApiMock.deleteAgentSubdirFile).toHaveBeenCalledWith(
        "workspace-agent",
        "AGENT",
        "zh.md",
      );
    });
    expect(fetchFilesMock).toHaveBeenCalled();
    expect(toastMock.success).toHaveBeenCalledWith("Deleted");
  });

  it("downloads the selected agent workspace zip through Rust workspace routes", async () => {
    render(<WorkspacePage />);

    fireEvent.click(screen.getByRole("button", { name: "Export" }));

    await waitFor(() => {
      expect(agentsApiMock.downloadAgentWorkspace).toHaveBeenCalledWith(
        "workspace-agent",
      );
      expect(saveFileMock).toHaveBeenCalledWith(
        expect.any(Blob),
        "workspace.zip",
      );
    });
    expect(toastMock.success).toHaveBeenCalledWith(
      "Downloaded (Downloads/workspace.zip)",
    );
  });

  it("validates workspace upload before calling the Rust zip import API", async () => {
    render(<WorkspacePage />);
    const input = screen.getByTitle("Upload zip");

    fireEvent.change(input, {
      target: { files: [new File(["not zip"], "notes.txt")] },
    });
    expect(toastMock.error).toHaveBeenCalledWith("ZIP only");
    expect(agentsApiMock.uploadAgentWorkspace).not.toHaveBeenCalled();

    const zip = new File(["zip"], "workspace.zip", {
      type: "application/zip",
    });
    fireEvent.change(input, { target: { files: [zip] } });

    await waitFor(() => {
      expect(agentsApiMock.uploadAgentWorkspace).toHaveBeenCalledWith(
        "workspace-agent",
        zip,
      );
    });
    expect(toastMock.success).toHaveBeenCalledWith("Uploaded");
  });

  it("delegates editor file deletion through the selected-agent workspace API", () => {
    render(<WorkspacePage />);

    fireEvent.click(screen.getByRole("button", { name: "Open root" }));
    expect(handleFileClickMock).toHaveBeenCalledWith({ filename: "ROOT.md" });

    fireEvent.click(screen.getByRole("button", { name: "Delete selected" }));
    expect(agentsApiMock.deleteAgentFile).toHaveBeenCalledWith(
      "workspace-agent",
      "ROOT.md",
      false,
    );
  });
});
