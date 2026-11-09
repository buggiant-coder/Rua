import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import SkillsPage from "./index";

const {
  apiMock,
  agentsApiMock,
  createSkillMock,
  deleteSkillMock,
  hardRefreshMock,
  refreshSkillsMock,
  showConflictRenameModalMock,
  toastMock,
  toggleEnabledMock,
  uploadSkillMock,
} = vi.hoisted(() => ({
  apiMock: {
    downloadSkillPoolSkill: vi.fn(),
    listSkillPoolSkills: vi.fn(),
    uploadWorkspaceSkillToPool: vi.fn(),
  },
  agentsApiMock: {
    batchDeleteAgentSkills: vi.fn(),
    saveAgentSkill: vi.fn(),
    updateAgentSkillChannels: vi.fn(),
  },
  createSkillMock: vi.fn(),
  deleteSkillMock: vi.fn(),
  hardRefreshMock: vi.fn(),
  refreshSkillsMock: vi.fn(),
  showConflictRenameModalMock: vi.fn(),
  toastMock: {
    error: vi.fn(),
    success: vi.fn(),
    warning: vi.fn(),
  },
  toggleEnabledMock: vi.fn(),
  uploadSkillMock: vi.fn(),
}));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string, params?: Record<string, unknown>) => {
      if (key === "skills.batchDeleteSuccess") {
        return `Deleted ${String(params?.count ?? "")}`;
      }
      return (
        {
          "common.batchDelete": "Batch delete",
          "common.cancel": "Cancel",
          "common.confirm": "Confirm",
          "common.delete": "Delete",
          "common.export": "Export",
          "common.import": "Import",
          "common.save": "Save",
          "skills.batchDeleteTitle": "Batch delete skills",
          "skills.create": "Create",
          "skills.createSkillHint": "Create skill",
          "skills.custom": "Custom",
          "skills.downloadedToWorkspace": "Downloaded to workspace",
          "skills.downloadFromPool": "Download from pool",
          "skills.emptyStateTitle": "No skills",
          "skills.gridView": "Grid",
          "skills.listView": "List",
          "skills.refreshHint": "Refresh",
          "skills.searchPlaceholder": "Search skills",
          "skills.selectAll": "Select all",
          "skills.selectedCount": "Selected",
          "skills.sortByEnabled": "Enabled",
          "skills.sortByLastUpdated": "Last updated",
          "skills.sortByName": "Name",
          "skills.title": "Skills",
          "skills.uploadedToPool": "Uploaded to pool",
          "skills.uploadToPool": "Upload to pool",
          "skills.zipOnly": "ZIP only",
          "skillPool.builtinOverwriteTargetsContent": "Builtin overwrite",
          "skillPool.currentVersion": "Current version",
          "skillPool.overwriteConfirm": "Overwrite?",
          "skillPool.sourceVersion": "Source version",
        }[key] ?? key
      );
    },
  }),
}));

vi.mock("../../../stores/agentStore", () => ({
  useAgentStore: () => ({ selectedAgent: "agent-a" }),
}));

vi.mock("../../../api", () => ({
  default: apiMock,
  agentsApi: agentsApiMock,
}));

vi.mock("../../../utils/error", () => ({
  parseErrorDetail: (error: { detail?: unknown }) => error.detail,
}));

vi.mock("@/utils/isMobile", () => ({ isMobile: () => false }));

vi.mock("@/utils/saveFile", () => ({
  getDownloadFolderHint: () => "",
  saveTextFile: vi.fn().mockResolvedValue({ ok: true }),
}));

vi.mock("@/components/Page", () => ({
  Page: ({ children }: { children: React.ReactNode }) => <div>{children}</div>,
  PageBody: ({ children }: { children: React.ReactNode }) => (
    <div>{children}</div>
  ),
}));

vi.mock("@/components/PageHeader", () => ({
  PageHeader: ({
    extra,
    items,
    moreMenuItems,
    showMoreMenu = true,
  }: {
    extra?: React.ReactNode;
    items?: Array<{ title: string }>;
    moreMenuItems?: Array<
      | { type: "divider" }
      | {
          disabled?: boolean;
          key: string;
          label: React.ReactNode;
          onClick?: () => void;
        }
    >;
    showMoreMenu?: boolean;
  }) => (
    <header>
      <h1>{items?.[0]?.title}</h1>
      {extra}
      {showMoreMenu
        ? moreMenuItems?.map((item, index) =>
            item.type === "divider" ? null : (
              <button
                disabled={item.disabled}
                key={item.key ?? index}
                type="button"
                onClick={item.onClick}
              >
                {item.label}
              </button>
            ),
          )
        : null}
    </header>
  ),
}));

vi.mock("@/components/EmptyState", () => ({
  EmptyState: ({ description }: { description?: string }) => (
    <div>{description ?? "empty"}</div>
  ),
}));

vi.mock("@/ui", () => ({
  Btn: ({
    children,
    disabled,
    onClick,
    title,
    variant,
  }: {
    children?: React.ReactNode;
    disabled?: boolean;
    onClick?: (event: React.MouseEvent<HTMLButtonElement>) => void;
    title?: string;
    variant?: string;
  }) => (
    <button disabled={disabled} title={title} type="button" onClick={onClick}>
      {children ?? title ?? variant ?? "button"}
    </button>
  ),
  Check: ({
    checked,
    onClick,
  }: {
    checked?: boolean;
    onClick?: (event: React.MouseEvent<HTMLButtonElement>) => void;
  }) => (
    <button aria-pressed={checked} type="button" onClick={onClick}>
      check
    </button>
  ),
  Dropdown: ({
    children,
    menu,
  }: {
    children: React.ReactNode;
    menu?: {
      items?: Array<{
        disabled?: boolean;
        key: string;
        label: string;
        onClick?: () => void;
      }>;
      onClick?: (event: { key: string }) => void;
    };
  }) => (
    <div>
      {children}
      {menu?.items?.map((item) => (
        <button
          disabled={item.disabled}
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
  Loading: () => <div>Loading</div>,
  Modal: {
    confirm: vi.fn(({ onOk }) => onOk?.()),
  },
  StackedList: ({ children }: { children: React.ReactNode }) => (
    <div>{children}</div>
  ),
  Tag: ({ children }: { children: React.ReactNode }) => <span>{children}</span>,
  TextInput: ({
    onChange,
    placeholder,
    value,
  }: {
    onChange?: (event: React.ChangeEvent<HTMLInputElement>) => void;
    placeholder?: string;
    value?: string;
  }) => (
    <input placeholder={placeholder} value={value ?? ""} onChange={onChange} />
  ),
  Toggle: ({
    checked,
    onChange,
  }: {
    checked?: boolean;
    onChange?: (event: { stopPropagation: () => void }) => void;
  }) => (
    <button
      aria-pressed={checked}
      type="button"
      onClick={() => onChange?.({ stopPropagation: vi.fn() })}
    >
      toggle
    </button>
  ),
  Tooltip: ({ children }: { children: React.ReactNode }) => <>{children}</>,
  toast: toastMock,
}));

vi.mock("./useSkills", () => ({
  useSkills: () => ({
    createSkill: createSkillMock,
    deleteSkill: deleteSkillMock,
    hardRefresh: hardRefreshMock,
    loadError: null,
    loading: false,
    refreshSkills: refreshSkillsMock,
    skills: [
      {
        channels: ["all"],
        config: {},
        content: "---\nname: docs\ndescription: Docs\n---",
        description: "Docs",
        enabled: true,
        last_updated: "2026-06-18T00:00:00Z",
        name: "docs",
        source: "workspace",
      },
      {
        channels: ["all"],
        config: {},
        content: "---\nname: review\ndescription: Review\n---",
        description: "Review",
        enabled: false,
        last_updated: "2026-06-19T00:00:00Z",
        name: "review",
        source: "builtin",
      },
    ],
    toggleEnabled: toggleEnabledMock,
    uploadSkill: uploadSkillMock,
    uploading: false,
  }),
}));

vi.mock("./components", () => ({
  SkillCard: ({
    onClick,
    onSelect,
    selected,
    skill,
  }: {
    onClick: () => void;
    onSelect: () => void;
    selected?: boolean;
    skill: { name: string; description?: string };
  }) => (
    <div>
      <span>
        skill:{skill.name}:{skill.description}:{String(selected)}
      </span>
      <button type="button" onClick={onClick}>
        Edit {skill.name}
      </button>
      <button type="button" onClick={onSelect}>
        Select {skill.name}
      </button>
    </div>
  ),
  SkillDrawer: ({
    editingSkill,
    onSubmit,
    open,
  }: {
    editingSkill: { name: string } | null;
    onSubmit: (values: {
      channels?: string[];
      config?: Record<string, unknown>;
      content: string;
      name: string;
    }) => void;
    open: boolean;
  }) =>
    open ? (
      <button
        type="button"
        onClick={() =>
          onSubmit({
            channels: ["all"],
            config: { level: "strict" },
            content: "---\nname: new-skill\ndescription: New\n---",
            name: editingSkill?.name ?? "new-skill",
          })
        }
      >
        Submit drawer
      </button>
    ) : null,
  PoolTransferModal: ({
    mode,
    onDownload,
    onUpload,
  }: {
    mode: "upload" | "download" | null;
    onDownload: (names: string[]) => void;
    onUpload: (names: string[]) => void;
  }) =>
    mode === "upload" ? (
      <button type="button" onClick={() => onUpload(["docs"])}>
        Confirm upload to pool
      </button>
    ) : mode === "download" ? (
      <button type="button" onClick={() => onDownload(["pool-docs"])}>
        Confirm download from pool
      </button>
    ) : null,
  getSkillDisplaySource: (source: string) => source,
  getSkillVisual: (name: string) => name.slice(0, 1).toUpperCase(),
  skillIconUrl: () => "",
  useConflictRenameModal: () => ({
    conflictRenameModal: null,
    showConflictRenameModal: showConflictRenameModalMock,
  }),
}));

function openMoreMenu(label: string) {
  fireEvent.click(screen.getByRole("button", { name: label }));
}

describe("SkillsPage", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    localStorage.clear();
    apiMock.listSkillPoolSkills.mockResolvedValue([
      { name: "pool-docs", source: "pool" },
    ]);
    apiMock.uploadWorkspaceSkillToPool.mockResolvedValue({ success: true });
    apiMock.downloadSkillPoolSkill.mockResolvedValue({ success: true });
    agentsApiMock.batchDeleteAgentSkills.mockResolvedValue({
      results: { docs: { success: true }, review: { success: true } },
    });
    agentsApiMock.saveAgentSkill.mockResolvedValue({
      mode: "save",
      name: "docs",
    });
    agentsApiMock.updateAgentSkillChannels.mockResolvedValue({ success: true });
    createSkillMock.mockResolvedValue({ success: true, name: "new-skill" });
    refreshSkillsMock.mockResolvedValue(undefined);
    uploadSkillMock.mockResolvedValue({ success: true });
    showConflictRenameModalMock.mockResolvedValue({ docs: "docs_1" });
  });

  it("creates a workspace skill and saves Rust channel settings", async () => {
    render(<SkillsPage />);

    fireEvent.click(screen.getAllByRole("button", { name: "filled" })[0]);
    fireEvent.click(screen.getByRole("button", { name: "Submit drawer" }));

    await waitFor(() => {
      expect(createSkillMock).toHaveBeenCalledWith(
        "new-skill",
        "---\nname: new-skill\ndescription: New\n---",
        { level: "strict" },
        true,
      );
      expect(agentsApiMock.updateAgentSkillChannels).toHaveBeenCalledWith(
        "agent-a",
        "new-skill",
        ["all"],
      );
      expect(refreshSkillsMock).toHaveBeenCalled();
    });
  });

  it("uploads workspace skills to the pool after Rust preview conflicts are confirmed", async () => {
    apiMock.uploadWorkspaceSkillToPool
      .mockRejectedValueOnce({ detail: { reason: "conflict" } })
      .mockResolvedValueOnce({ success: true });

    render(<SkillsPage />);
    openMoreMenu("Upload to pool");
    fireEvent.click(
      screen.getByRole("button", { name: "Confirm upload to pool" }),
    );

    await waitFor(() => {
      expect(apiMock.uploadWorkspaceSkillToPool).toHaveBeenNthCalledWith(1, {
        workspace_id: "agent-a",
        skill_name: "docs",
        preview_only: true,
      });
      expect(apiMock.uploadWorkspaceSkillToPool).toHaveBeenNthCalledWith(2, {
        workspace_id: "agent-a",
        skill_name: "docs",
        overwrite: true,
      });
      expect(toastMock.success).toHaveBeenCalledWith("Uploaded to pool");
    });
  });

  it("downloads pool skills to the selected workspace with builtin-upgrade overwrite", async () => {
    apiMock.downloadSkillPoolSkill
      .mockRejectedValueOnce({
        detail: {
          conflicts: [
            {
              current_version_text: "1.0.0",
              reason: "builtin_upgrade",
              skill_name: "pool-docs",
              source_version_text: "1.1.0",
            },
          ],
        },
      })
      .mockResolvedValueOnce({ success: true });

    render(<SkillsPage />);
    openMoreMenu("Download from pool");
    fireEvent.click(
      screen.getByRole("button", { name: "Confirm download from pool" }),
    );

    await waitFor(() => {
      expect(apiMock.downloadSkillPoolSkill).toHaveBeenNthCalledWith(1, {
        skill_name: "pool-docs",
        targets: [{ workspace_id: "agent-a" }],
        preview_only: true,
      });
      expect(apiMock.downloadSkillPoolSkill).toHaveBeenNthCalledWith(2, {
        skill_name: "pool-docs",
        targets: [{ workspace_id: "agent-a" }],
        overwrite: true,
      });
      expect(toastMock.success).toHaveBeenCalledWith("Downloaded to workspace");
    });
  });

  it("batch-deletes selected workspace skills through agent-scoped Rust APIs", async () => {
    render(<SkillsPage />);

    openMoreMenu("Batch delete");
    fireEvent.click(screen.getByRole("button", { name: "Select all" }));
    fireEvent.click(screen.getByRole("button", { name: /Delete \(2\)/ }));

    await waitFor(() => {
      expect(agentsApiMock.batchDeleteAgentSkills).toHaveBeenCalledWith(
        "agent-a",
        ["docs", "review"],
      );
      expect(toastMock.success).toHaveBeenCalledWith("Deleted 2");
      expect(refreshSkillsMock).toHaveBeenCalled();
    });
  });

  it("imports zip skills and retries Rust rename_map conflicts", async () => {
    uploadSkillMock
      .mockResolvedValueOnce({
        conflict: {
          conflicts: [{ skill_name: "docs", suggested_name: "docs_1" }],
        },
        success: false,
      })
      .mockResolvedValueOnce({ success: true });
    const { container } = render(<SkillsPage />);
    const input = container.querySelector('input[type="file"]')!;

    openMoreMenu("Import");
    fireEvent.change(input, {
      target: { files: [new File(["zip"], "skills.zip")] },
    });

    await waitFor(() => {
      expect(uploadSkillMock).toHaveBeenNthCalledWith(
        1,
        expect.any(File),
        undefined,
        undefined,
      );
      expect(uploadSkillMock).toHaveBeenNthCalledWith(
        2,
        expect.any(File),
        undefined,
        { docs: "docs_1" },
      );
    });
  });

  it("persists the selected workspace skill view mode", async () => {
    render(<SkillsPage />);

    fireEvent.click(screen.getByRole("button", { name: "List" }));

    expect(localStorage.getItem("agent.skills.viewMode")).toBe("list");
  });

  it("restores the workspace skill list view from localStorage", async () => {
    localStorage.setItem("agent.skills.viewMode", "list");

    render(<SkillsPage />);

    expect(screen.queryByText(/skill:docs:Docs/)).not.toBeInTheDocument();
    expect(screen.getByText("docs")).toBeVisible();
    expect(screen.queryByText("all")).not.toBeInTheDocument();
  });

  it("does not open the skill drawer when toggling a list-mode skill", async () => {
    localStorage.setItem("agent.skills.viewMode", "list");

    render(<SkillsPage />);
    fireEvent.click(screen.getAllByRole("button", { name: "toggle" })[0]);

    await waitFor(() => {
      expect(toggleEnabledMock).toHaveBeenCalledWith(
        expect.objectContaining({ name: "docs" }),
      );
      expect(refreshSkillsMock).toHaveBeenCalled();
    });
    expect(screen.queryByText("Submit drawer")).not.toBeInTheDocument();
  });
});
