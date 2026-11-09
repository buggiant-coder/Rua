import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import PromptLibraryPage from "./index";

const {
  clearSelectionMock,
  fetchFilesMock,
  promptLibraryApiMock,
  restoreBuiltinMock,
  saveFileMock,
  setSortModeMock,
  toastMock,
} = vi.hoisted(() => ({
  clearSelectionMock: vi.fn(),
  fetchFilesMock: vi.fn(),
  promptLibraryApiMock: {
    broadcast: vi.fn(),
    deleteFile: vi.fn(),
    deleteSubdirFile: vi.fn(),
    renameFile: vi.fn(),
    renameSubdirFile: vi.fn(),
  },
  restoreBuiltinMock: vi.fn(),
  saveFileMock: vi.fn(),
  setSortModeMock: vi.fn(),
  toastMock: {
    error: vi.fn(),
    success: vi.fn(),
    warning: vi.fn(),
  },
}));

vi.mock("../../../api/modules/promptLibrary", () => ({
  promptLibraryApi: promptLibraryApiMock,
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
      if (key === "workspace.fileSizeExceeded") return "File too large";
      if (key === "promptLibrary.conflictMessage") {
        const name =
          params && typeof params === "object" && "name" in params
            ? String((params as { name: unknown }).name)
            : "";
        return `Conflict ${name}`;
      }
      return (
        {
          "common.cancel": "Cancel",
          "common.confirm": "Confirm",
          "common.create": "Create",
          "common.export": "Export",
          "common.import": "Import",
          "nav.promptLibrary": "Prompt Library",
          "promptLibrary.broadcast": "Broadcast",
          "promptLibrary.broadcastFailed": "Broadcast failed",
          "promptLibrary.broadcastPartial": "Broadcast partial",
          "promptLibrary.broadcastSuccess": "Broadcasted",
          "promptLibrary.conflictTitle": "Conflict",
          "promptLibrary.loadFromDisk": "Refresh",
          "promptLibrary.mdOrZipOnly": "MD or ZIP only",
          "promptLibrary.overwrite": "Overwrite",
          "promptLibrary.renameCreate": "Rename",
          "promptLibrary.restoreBuiltin": "Restore builtin",
          "promptLibrary.restoreBuiltinConfirmContent": "Restore builtin?",
          "promptLibrary.restoreBuiltinConfirmTitle": "Restore builtin",
          "promptLibrary.searchPlaceholder": "Search prompts",
          "promptLibrary.sortByLastUpdated": "Last updated",
          "promptLibrary.sortByName": "Name",
          "workspace.batchDelete": "Batch delete",
          "workspace.deleteFailed": "Delete failed",
          "workspace.deleteSuccess": "Deleted",
          "workspace.downloadFailed": "Download failed",
          "workspace.downloadSuccess": "Downloaded",
          "workspace.renameFailed": "Rename failed",
          "workspace.renameSuccess": "Renamed",
          "workspace.uploadFailed": "Upload failed",
          "workspace.uploadSuccess": "Uploaded",
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
      onClick?: (event: { key: string }) => void;
    };
  }) => (
    <div>
      {children}
      {menu?.items?.map((item) => (
        <button
          key={item.key}
          type="button"
          onClick={() =>
            item.onClick ? item.onClick() : menu.onClick?.({ key: item.key })
          }
        >
          {item.label}
        </button>
      ))}
    </div>
  ),
  Modal: Object.assign(
    ({
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
    {
      confirm: vi.fn(({ onOk }) => onOk?.()),
    },
  ),
  TextInput: ({
    onChange,
    placeholder,
    value,
  }: {
    onChange: (event: React.ChangeEvent<HTMLInputElement>) => void;
    placeholder?: string;
    value: string;
  }) => <input placeholder={placeholder} value={value} onChange={onChange} />,
  Tooltip: ({ children }: { children: React.ReactNode }) => <>{children}</>,
  toast: toastMock,
}));

vi.mock("../../Agent/Workspace/components/FileEditor", () => ({
  FileEditor: ({
    onDelete,
    onRename,
    selectedFile,
  }: {
    onDelete: () => void;
    onRename: (name: string) => void;
    selectedFile: { filename: string } | null;
  }) => (
    <div>
      <span>editor:{selectedFile?.filename ?? "none"}</span>
      <button type="button" onClick={onDelete}>
        Delete selected
      </button>
      <button type="button" onClick={() => onRename("RENAMED.md")}>
        Rename selected
      </button>
    </div>
  ),
}));

vi.mock("./components/usePromptLibraryData", () => ({
  usePromptLibraryData: () => ({
    autoRename: false,
    cancelCreateFile: vi.fn(),
    clearSelection: clearSelectionMock,
    commitNewFileName: vi.fn(),
    expandedDirs: new Set(["AGENT.md"]),
    fetchFiles: fetchFilesMock,
    fetchSubdirFiles: vi.fn(),
    fileContent: "content",
    files: [
      { filename: "ZETA.md", size: 10, updated_at: 2 },
      { filename: "AGENT.md", size: 20, updated_at: 1 },
    ],
    filesWithSubdirs: new Set(["AGENT.md"]),
    handleCreateSubdirFile: vi.fn(),
    handleFileClick: vi.fn(),
    handleReset: vi.fn(),
    handleSave: vi.fn(),
    handleSubdirFileClick: vi.fn(),
    handleToggleExpandAll: vi.fn(),
    hasChanges: false,
    isCreatingNew: false,
    loading: false,
    restoreBuiltin: restoreBuiltinMock,
    selectedFile: { filename: "AGENT.md", size: 20 },
    setAutoRename: vi.fn(),
    setFileContent: vi.fn(),
    setSortMode: setSortModeMock,
    sortMode: "name-asc",
    startCreateFile: vi.fn(),
    subdirFiles: {
      "AGENT.md": [{ filename: "zh.md", size: 5, subdir: "AGENT" }],
    },
    toggleExpand: vi.fn(),
  }),
}));

vi.mock("./components/LibraryFileListPanel", () => ({
  LibraryFileListPanel: ({
    batchDeleteMode,
    batchSelected,
    files,
    onToggleBatchSelect,
  }: {
    batchDeleteMode: boolean;
    batchSelected: Set<string>;
    files: Array<{ filename: string }>;
    onToggleBatchSelect: (filename: string) => void;
  }) => (
    <div>
      <span>batch:{String(batchDeleteMode)}</span>
      <span>selected:{Array.from(batchSelected).join(",")}</span>
      <span>files:{files.map((file) => file.filename).join(",")}</span>
      <button type="button" onClick={() => onToggleBatchSelect("AGENT.md")}>
        Select root
      </button>
      <button type="button" onClick={() => onToggleBatchSelect("AGENT/zh.md")}>
        Select subdir
      </button>
    </div>
  ),
}));

vi.mock("./components/BroadcastModal", () => ({
  BroadcastModal: ({
    onConfirm,
    open,
  }: {
    onConfirm: (files: string[], agentIds: string[]) => void;
    open: boolean;
  }) =>
    open ? (
      <button
        type="button"
        onClick={() => onConfirm(["AGENT.md"], ["default"])}
      >
        Confirm broadcast
      </button>
    ) : null,
}));

describe("PromptLibraryPage", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    restoreBuiltinMock.mockResolvedValue(["ROOT.md"]);
    promptLibraryApiMock.broadcast.mockResolvedValue({
      results: [
        { agent_id: "default", file: "AGENT.md", final_name: "AGENT.md" },
      ],
    });
    promptLibraryApiMock.deleteFile.mockResolvedValue({});
    promptLibraryApiMock.deleteSubdirFile.mockResolvedValue({});
    promptLibraryApiMock.renameFile.mockResolvedValue({});
    saveFileMock.mockResolvedValue({ ok: true });
    vi.stubGlobal(
      "fetch",
      vi.fn((url: string, init?: RequestInit) => {
        if (String(url).endsWith("/download")) {
          return Promise.resolve({
            blob: () => Promise.resolve(new Blob(["zip"])),
            headers: {
              get: () => 'attachment; filename="prompt_library.zip"',
            },
            ok: true,
          });
        }
        if (init?.method === "POST") {
          return Promise.resolve({
            json: () => Promise.resolve({ success: true }),
            ok: true,
          });
        }
        return Promise.reject(new Error("unexpected fetch"));
      }),
    );
  });

  it("filters and sorts the shared prompt-library list without changing hook ownership", () => {
    render(<PromptLibraryPage />);

    expect(screen.getByText("files:AGENT.md,ZETA.md")).toBeVisible();
    fireEvent.change(screen.getByPlaceholderText("Search prompts"), {
      target: { value: "zeta" },
    });
    expect(screen.getByText("files:ZETA.md")).toBeVisible();

    fireEvent.click(screen.getByRole("button", { name: "Last updated ↓" }));
    expect(setSortModeMock).toHaveBeenCalledWith("time-desc");
  });

  it("batch-deletes root and subdir prompt-library files through Rust routes", async () => {
    render(<PromptLibraryPage />);

    fireEvent.click(screen.getByRole("button", { name: "Batch delete" }));
    fireEvent.click(screen.getByRole("button", { name: "Select root" }));
    fireEvent.click(screen.getByRole("button", { name: "Select subdir" }));
    fireEvent.click(screen.getByRole("button", { name: "danger" }));

    await waitFor(() => {
      expect(promptLibraryApiMock.deleteFile).toHaveBeenCalledWith(
        "AGENT.md",
        true,
      );
      expect(promptLibraryApiMock.deleteSubdirFile).toHaveBeenCalledWith(
        "AGENT",
        "zh.md",
      );
    });
    expect(clearSelectionMock).toHaveBeenCalled();
    expect(fetchFilesMock).toHaveBeenCalled();
    expect(toastMock.success).toHaveBeenCalledWith("Deleted");
  });

  it("exports and imports the shared prompt library through Rust zip/md routes", async () => {
    const { container } = render(<PromptLibraryPage />);
    const input = container.querySelector('input[type="file"]')!;

    fireEvent.click(screen.getByRole("button", { name: "Export" }));
    await waitFor(() => {
      expect(saveFileMock).toHaveBeenCalledWith(
        expect.any(Blob),
        "prompt_library.zip",
      );
    });
    expect(toastMock.success).toHaveBeenCalledWith(
      "Downloaded (Downloads/prompt_library.zip)",
    );

    fireEvent.change(input, {
      target: { files: [new File(["bad"], "notes.txt")] },
    });
    expect(toastMock.error).toHaveBeenCalledWith("MD or ZIP only");

    const file = new File(["# Prompt"], "prompt.md", { type: "text/markdown" });
    fireEvent.change(input, { target: { files: [file] } });
    await waitFor(() => {
      expect(fetch).toHaveBeenCalledWith(
        "/api/prompt-library/upload",
        expect.objectContaining({ method: "POST" }),
      );
    });
    expect(fetchFilesMock).toHaveBeenCalled();
    expect(toastMock.success).toHaveBeenCalledWith("Uploaded");
  });

  it("restores builtin prompts and broadcasts selected prompts", async () => {
    render(<PromptLibraryPage />);

    fireEvent.click(screen.getByRole("button", { name: "Restore builtin" }));
    await waitFor(() => {
      expect(restoreBuiltinMock).toHaveBeenCalledTimes(1);
    });

    fireEvent.click(screen.getByRole("button", { name: "Broadcast" }));
    fireEvent.click(screen.getByRole("button", { name: "Confirm broadcast" }));

    await waitFor(() => {
      expect(promptLibraryApiMock.broadcast).toHaveBeenCalledWith(
        ["AGENT.md"],
        ["default"],
        "ask",
      );
    });
    expect(toastMock.success).toHaveBeenCalledWith("Broadcasted");
  });

  it("retries prompt-library broadcast conflicts with the selected resolution", async () => {
    promptLibraryApiMock.broadcast
      .mockResolvedValueOnce({
        results: [
          {
            agent_id: "default",
            conflict: true,
            file: "AGENT.md",
            suggested_name: "AGENT_1.md",
          },
        ],
      })
      .mockResolvedValueOnce({
        results: [
          {
            agent_id: "default",
            file: "AGENT.md",
            final_name: "AGENT_1.md",
          },
        ],
      });

    render(<PromptLibraryPage />);

    fireEvent.click(screen.getByRole("button", { name: "Broadcast" }));
    fireEvent.click(screen.getByRole("button", { name: "Confirm broadcast" }));
    expect(await screen.findByText("Conflict AGENT.md")).toBeVisible();
    fireEvent.click(
      screen.getByRole("button", { name: "Rename (AGENT_1.md)" }),
    );

    await waitFor(() => {
      expect(promptLibraryApiMock.broadcast).toHaveBeenNthCalledWith(
        1,
        ["AGENT.md"],
        ["default"],
        "ask",
      );
      expect(promptLibraryApiMock.broadcast).toHaveBeenNthCalledWith(
        2,
        ["AGENT.md"],
        ["default"],
        "rename",
      );
    });
    expect(toastMock.success).toHaveBeenCalledWith("Broadcasted");
  });

  it("reports prompt-library download and upload failures without refreshing", async () => {
    vi.mocked(fetch)
      .mockResolvedValueOnce({
        ok: false,
      } as Response)
      .mockResolvedValueOnce({
        json: () =>
          Promise.resolve({ message: "invalid archive", success: false }),
        ok: true,
      } as Response);

    const { container } = render(<PromptLibraryPage />);
    const input = container.querySelector('input[type="file"]')!;

    fireEvent.click(screen.getByRole("button", { name: "Export" }));
    await waitFor(() => {
      expect(toastMock.error).toHaveBeenCalledWith(
        "Download failed: Download failed",
      );
    });

    const file = new File(["zip"], "prompt.zip", { type: "application/zip" });
    fireEvent.change(input, { target: { files: [file] } });

    await waitFor(() => {
      expect(toastMock.error).toHaveBeenCalledWith(
        "Upload failed: invalid archive",
      );
    });
  });

  it("reports batch-delete failures for shared prompt-library files", async () => {
    promptLibraryApiMock.deleteFile.mockRejectedValueOnce(new Error("locked"));

    render(<PromptLibraryPage />);

    fireEvent.click(screen.getByRole("button", { name: "Batch delete" }));
    fireEvent.click(screen.getByRole("button", { name: "Select root" }));
    fireEvent.click(screen.getByRole("button", { name: "danger" }));

    await waitFor(() => {
      expect(toastMock.error).toHaveBeenCalledWith("Delete failed");
    });
    expect(toastMock.success).not.toHaveBeenCalledWith("Deleted");
  });
});
