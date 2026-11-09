import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import SkillPoolPage, { __resetSkillPoolPageCacheForTests } from "./index";

const {
  apiMock,
  checkScanWarningsMock,
  conflictRenameMock,
  modalConfirmMock,
  toastMock,
} = vi.hoisted(() => ({
  apiMock: {
    batchDeletePoolSkills: vi.fn(),
    createSkillPoolSkill: vi.fn(),
    deleteSkillPoolSkill: vi.fn(),
    downloadSkillPoolSkill: vi.fn(),
    getBlockedHistory: vi.fn(),
    getSkillScanner: vi.fn(),
    importSelectedPoolBuiltins: vi.fn(),
    listPoolBuiltinSources: vi.fn(),
    listSkillPoolSkills: vi.fn(),
    listSkillWorkspaces: vi.fn(),
    refreshSkillPool: vi.fn(),
    saveSkillPoolSkill: vi.fn(),
    uploadSkillPoolZip: vi.fn(),
  },
  checkScanWarningsMock: vi.fn(),
  conflictRenameMock: vi.fn(),
  modalConfirmMock: vi.fn(),
  toastMock: {
    error: vi.fn(),
    info: vi.fn(),
    success: vi.fn(),
    warning: vi.fn(),
  },
}));

vi.mock("../../../api", () => ({ default: apiMock }));

vi.mock("../../../utils/error", () => ({
  parseErrorDetail: (error: { detail?: unknown }) => error.detail,
}));

vi.mock("../../../utils/scanError", () => ({
  checkScanWarnings: checkScanWarningsMock,
  handleScanError: vi.fn(() => false),
}));

vi.mock("@/utils/isMobile", () => ({ isMobile: () => false }));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string, params?: unknown) => {
      if (key === "skillPool.batchDeleteSuccess") {
        return `Deleted ${String(
          (params as { count?: unknown })?.count ?? "",
        )}`;
      }
      if (key === "skillPool.importBuiltinSuccess") {
        return `Imported ${String(
          (params as { names?: unknown })?.names ?? "",
        )}`;
      }
      if (key === "skillPool.imported") {
        return `Imported ${String(
          (params as { names?: unknown })?.names ?? "",
        )}`;
      }
      if (key === "skills.fileSizeExceeded") return "File too large";
      return (
        {
          "common.batchDelete": "Batch delete",
          "common.cancel": "Cancel",
          "common.create": "Create",
          "common.delete": "Delete",
          "common.import": "Import",
          "nav.skillPool": "Skill Pool",
          "skillPool.broadcast": "Broadcast",
          "skillPool.broadcastSuccess": "Broadcasted",
          "skillPool.delete": "Delete skill",
          "skillPool.importBuiltinFailed": "Import builtin failed",
          "skillPool.importBuiltinNoChanges": "No changes",
          "skillPool.noNewImports": "No new imports",
          "skillPool.refreshHint": "Refresh",
          "skillPool.updateBuiltin": "Update builtin",
          "skillPool.zipImportFailed": "Zip import failed",
          "skills.createSkillHint": "Create skill",
          "skills.gridView": "Grid",
          "skills.installChannel": "Install channel",
          "skills.installChannelMarket": "Skill Market",
          "skills.installFrom": "Install from",
          "skills.installInfo": "Install info",
          "skills.installVersion": "Install version",
          "skills.installedAt": "Installed at",
          "skills.listView": "List",
          "skills.marketName": "Market name",
          "skills.marketProvider": "Market provider",
          "skills.marketSlug": "Market slug",
          "skills.searchPlaceholder": "Search skills",
          "skills.selectAll": "Select all",
          "skills.selectedCount": "Selected",
          "skills.sourceUrl": "Source URL",
          "skills.sortByLastUpdated": "Last updated",
          "skills.sortByName": "Name",
          "skills.updatedAt": "Updated at",
          "skills.author": "Author",
          "skills.zipOnly": "ZIP only",
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
      {children ?? title ?? variant ?? "action"}
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
  Drawer: ({
    children,
    open,
  }: {
    children?: React.ReactNode;
    open?: boolean;
  }) => (open ? <div>{children}</div> : null),
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
  FormField: ({ children }: { children: React.ReactNode }) => (
    <label>{children}</label>
  ),
  Modal: {
    confirm: modalConfirmMock,
  },
  StackedList: ({
    children,
    className,
  }: {
    children?: React.ReactNode;
    className?: string;
  }) => <div className={className}>{children}</div>,
  Tag: ({ children }: { children?: React.ReactNode }) => (
    <span>{children}</span>
  ),
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
  Tooltip: ({ children }: { children: React.ReactNode }) => <>{children}</>,
  toast: toastMock,
}));

vi.mock("../../Agent/Skills/components", () => ({
  getSkillDisplaySource: (source: string) => source,
  getSkillVisual: (name: string) => name.slice(0, 1).toUpperCase(),
  parseFrontmatter: (content: string) =>
    content.includes("name:") && content.includes("description:")
      ? { description: "Skill", name: "skill" }
      : null,
  SkillInstallInfo: ({
    meta,
  }: {
    meta: {
      installed_from: string;
      source_url?: string;
      market_meta?: {
        author?: string;
        name?: string;
        slug?: string;
        source?: string;
      };
    };
  }) => (
    <div>
      {meta.market_meta && <span>Skill Market</span>}
      <span>{meta.installed_from}</span>
      <span>{meta.market_meta?.source}</span>
      <span>{meta.market_meta?.name}</span>
      <span>{meta.market_meta?.slug}</span>
      <span>{meta.market_meta?.author}</span>
      <span>{meta.source_url}</span>
    </div>
  ),
  useConflictRenameModal: () => ({
    conflictRenameModal: null,
    showConflictRenameModal: conflictRenameMock,
  }),
}));

vi.mock("../../Agent/Skills/components/SkillUpdateButton", () => ({
  SkillUpdateButton: () => <button type="button">Update skill</button>,
}));

vi.mock("../../../components/MarkdownCopy/MarkdownCopy", () => ({
  MarkdownCopy: ({
    content,
    showCopyButton,
  }: {
    content: string;
    showCopyButton?: boolean;
  }) => (
    <div>
      <span>{content}</span>
      <span>copy-button:{String(showCopyButton)}</span>
    </div>
  ),
}));

vi.mock("./components", () => ({
  SkillPoolCard: ({
    batchMode,
    isSelected,
    onBroadcast,
    onDelete,
    onEdit,
    onToggleSelect,
    skill,
  }: {
    batchMode: boolean;
    isSelected: boolean;
    onBroadcast: (skill: { name: string }) => void;
    onDelete: (skill: { name: string }) => void;
    onEdit: (skill: { name: string }) => void;
    onToggleSelect: (name: string) => void;
    skill: { name: string; description?: string };
  }) => (
    <div>
      <span>
        skill:{skill.name}:{skill.description}:{String(isSelected)}:
        {String(batchMode)}
      </span>
      <button type="button" onClick={() => onEdit(skill)}>
        Edit {skill.name}
      </button>
      <button type="button" onClick={() => onBroadcast(skill)}>
        Broadcast {skill.name}
      </button>
      <button type="button" onClick={() => onDelete(skill)}>
        Delete {skill.name}
      </button>
      <button type="button" onClick={() => onToggleSelect(skill.name)}>
        Select {skill.name}
      </button>
    </div>
  ),
  poolSkillIconUrl: () => "",
}));

vi.mock("./components/BroadcastModal", () => ({
  BroadcastModal: ({
    onConfirm,
    open,
  }: {
    onConfirm: (skills: string[], workspaces: string[]) => void;
    open: boolean;
  }) =>
    open ? (
      <button type="button" onClick={() => onConfirm(["docs"], ["default"])}>
        Confirm broadcast
      </button>
    ) : null,
}));

vi.mock("./components/ImportBuiltinModal", () => ({
  ImportBuiltinModal: ({
    onConfirm,
    open,
  }: {
    onConfirm: (names: string[]) => void;
    open: boolean;
  }) =>
    open ? (
      <button type="button" onClick={() => onConfirm(["review"])}>
        Confirm builtin import
      </button>
    ) : null,
}));

function skill(
  name: string,
  description = "Skill",
  lastUpdated = "2026-06-01",
  installMeta?: Record<string, unknown>,
) {
  return {
    config: {},
    content: `---\nname: ${name}\ndescription: ${description}\n---`,
    description,
    last_updated: lastUpdated,
    name,
    protected: false,
    source: "pool",
    tags: [],
    install_meta: installMeta,
  };
}

describe("SkillPoolPage", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    __resetSkillPoolPageCacheForTests();
    localStorage.clear();
    apiMock.listSkillPoolSkills.mockResolvedValue([
      skill("docs", "Documentation", "2026-06-01"),
      skill("review", "Code review", "2026-06-02"),
    ]);
    apiMock.listSkillWorkspaces.mockResolvedValue([
      { agent_id: "default", agent_name: "Default" },
    ]);
    apiMock.refreshSkillPool.mockResolvedValue([skill("fresh", "Fresh")]);
    apiMock.batchDeletePoolSkills.mockResolvedValue({
      results: { docs: { success: true }, review: { success: true } },
    });
    apiMock.deleteSkillPoolSkill.mockResolvedValue({ success: true });
    apiMock.downloadSkillPoolSkill.mockResolvedValue({ success: true });
    apiMock.importSelectedPoolBuiltins.mockResolvedValue({
      imported: ["review"],
      unchanged: [],
      updated: [],
    });
    apiMock.listPoolBuiltinSources.mockResolvedValue([
      { name: "review", status: "missing", version_text: "1.0.0" },
    ]);
    apiMock.uploadSkillPoolZip.mockResolvedValue({
      count: 1,
      imported: ["docs"],
    });
    conflictRenameMock.mockResolvedValue({ docs: "docs_1" });
    modalConfirmMock.mockImplementation(({ onOk }) => onOk?.());
  });

  it("loads, filters, and sorts Rust skill-pool cards", async () => {
    render(<SkillPoolPage />);

    expect(await screen.findByText(/skill:docs:Documentation/)).toBeVisible();
    expect(screen.getByText(/skill:review:Code review/)).toBeVisible();

    fireEvent.change(screen.getByPlaceholderText("Search skills"), {
      target: { value: "review" },
    });
    expect(
      screen.queryByText(/skill:docs:Documentation/),
    ).not.toBeInTheDocument();
    expect(screen.getByText(/skill:review:Code review/)).toBeVisible();

    fireEvent.click(screen.getByRole("button", { name: "Last updated ↓" }));
    expect(localStorage.getItem("agent.skillPool.sortField")).toBe(
      "lastUpdated",
    );
    expect(localStorage.getItem("agent.skillPool.sortOrder")).toBe("desc");
  });

  it("persists the selected skill-pool view mode", async () => {
    render(<SkillPoolPage />);
    await screen.findByText(/skill:docs/);

    fireEvent.click(screen.getByRole("button", { name: "List" }));

    expect(localStorage.getItem("agent.skillPool.viewMode")).toBe("list");
  });

  it("reuses loaded skill-pool data across remounts without showing a loading screen", async () => {
    const { unmount } = render(<SkillPoolPage />);
    await screen.findByText(/skill:docs:Documentation/);
    expect(apiMock.listSkillPoolSkills).toHaveBeenCalledTimes(1);

    unmount();
    render(<SkillPoolPage />);

    expect(screen.getByText(/skill:docs:Documentation/)).toBeVisible();
    expect(screen.queryByText("common.loading")).not.toBeInTheDocument();
    expect(apiMock.listSkillPoolSkills).toHaveBeenCalledTimes(1);
  });

  it("restores the skill-pool list view from localStorage", async () => {
    localStorage.setItem("agent.skillPool.viewMode", "list");

    render(<SkillPoolPage />);

    await waitFor(() => {
      expect(
        screen.queryByText(/skill:docs:Documentation/),
      ).not.toBeInTheDocument();
      expect(screen.getByText("docs")).toBeVisible();
      expect(screen.queryByText("all")).not.toBeInTheDocument();
    });
  });

  it("shows market install metadata in the skill-pool drawer without a copy button", async () => {
    apiMock.listSkillPoolSkills.mockResolvedValue([
      skill("docs", "Documentation", "2026-06-01", {
        installed_from: "clawhub",
        installed_version: "1.2.0",
        installed_at: "2026-06-18T10:00:00Z",
        updated_at: "2026-06-19T10:00:00Z",
        source_url: "https://clawhub.ai/docs",
        market_meta: {
          author: "Alice",
          name: "Docs Skill",
          slug: "org/docs",
          source: "clawhub",
        },
      }),
    ]);

    render(<SkillPoolPage />);
    await screen.findByText(/skill:docs/);
    fireEvent.click(screen.getByRole("button", { name: "Edit docs" }));

    expect(screen.getByText("Skill Market")).toBeVisible();
    expect(screen.getAllByText("clawhub").length).toBeGreaterThan(0);
    expect(screen.getByText("Docs Skill")).toBeVisible();
    expect(screen.getByText("org/docs")).toBeVisible();
    expect(screen.getByText("Alice")).toBeVisible();
    expect(screen.getByText("https://clawhub.ai/docs")).toBeVisible();
    expect(screen.getByText("copy-button:false")).toBeVisible();
  });

  it("batch-deletes selected pool skills through Rust pool APIs", async () => {
    render(<SkillPoolPage />);
    await screen.findByText(/skill:docs/);

    fireEvent.click(screen.getByRole("button", { name: "Batch delete" }));
    fireEvent.click(screen.getByRole("button", { name: "Select all" }));
    fireEvent.click(screen.getByRole("button", { name: /Delete \(2\)/ }));

    await waitFor(() => {
      expect(apiMock.batchDeletePoolSkills).toHaveBeenCalledWith([
        "docs",
        "review",
      ]);
    });
    expect(toastMock.success).toHaveBeenCalledWith("Deleted 2");
  });

  it("imports selected builtin skills into the Rust skill pool", async () => {
    render(<SkillPoolPage />);
    await screen.findByText(/skill:docs/);

    fireEvent.click(screen.getByRole("button", { name: "Update builtin" }));
    await waitFor(() => {
      expect(apiMock.listPoolBuiltinSources).toHaveBeenCalledTimes(1);
    });
    fireEvent.click(
      screen.getByRole("button", { name: "Confirm builtin import" }),
    );

    await waitFor(() => {
      expect(apiMock.importSelectedPoolBuiltins).toHaveBeenCalledWith({
        overwrite_conflicts: false,
        skill_names: ["review"],
      });
    });
    expect(toastMock.success).toHaveBeenCalledWith("Imported review");
  });

  it("imports zip bundles and retries conflicts with Rust rename_map", async () => {
    apiMock.uploadSkillPoolZip
      .mockRejectedValueOnce({
        detail: {
          conflicts: [{ skill_name: "docs", suggested_name: "docs_1" }],
        },
      })
      .mockResolvedValueOnce({ count: 1, imported: ["docs_1"] });
    const { container } = render(<SkillPoolPage />);
    await screen.findByText(/skill:docs/);
    const input = container.querySelector('input[type="file"]')!;

    fireEvent.click(screen.getByRole("button", { name: "Import" }));
    fireEvent.change(input, {
      target: { files: [new File(["zip"], "skills.zip")] },
    });

    await waitFor(() => {
      expect(apiMock.uploadSkillPoolZip).toHaveBeenNthCalledWith(
        1,
        expect.any(File),
        { overwrite: false, rename_map: undefined },
      );
      expect(apiMock.uploadSkillPoolZip).toHaveBeenNthCalledWith(
        2,
        expect.any(File),
        { overwrite: false, rename_map: { docs: "docs_1" } },
      );
    });
    expect(toastMock.success).toHaveBeenCalledWith("Imported docs_1");
  });

  it("broadcasts a pool skill and checks scanner warnings", async () => {
    render(<SkillPoolPage />);
    await screen.findByText(/skill:docs/);

    fireEvent.click(screen.getByRole("button", { name: "Broadcast docs" }));
    fireEvent.click(screen.getByRole("button", { name: "Confirm broadcast" }));

    await waitFor(() => {
      expect(apiMock.downloadSkillPoolSkill).toHaveBeenCalledWith({
        skill_name: "docs",
        targets: [{ target_name: undefined, workspace_id: "default" }],
      });
    });
    expect(checkScanWarningsMock).toHaveBeenCalledWith(
      "docs",
      apiMock.getBlockedHistory,
      apiMock.getSkillScanner,
      expect.any(Function),
    );
    expect(toastMock.success).toHaveBeenCalledWith("Broadcasted");
  });
});
