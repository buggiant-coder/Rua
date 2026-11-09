import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { api } from "../../../../api";
import { parseFrontmatter, SkillDrawer } from "./SkillDrawer";

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    i18n: { language: "zh-CN" },
    t: (key: string) =>
      ({
        "common.cancel": "Cancel",
        "common.name": "Name",
        "common.save": "Save",
        "skills.author": "Author",
        "skills.contentPlaceholder": "Skill content",
        "skills.create": "Create",
        "skills.createSkill": "Create skill",
        "skills.frontmatterDescriptionRequired": "Description required",
        "skills.frontmatterNameRequired": "Name required",
        "skills.frontmatterRequired": "Frontmatter required",
        "skills.installFrom": "Install from",
        "skills.installInfo": "Install info",
        "skills.installChannel": "Install channel",
        "skills.installChannelMarket": "Skill Market",
        "skills.installedAt": "Installed at",
        "skills.installVersion": "Install version",
        "skills.marketName": "Market name",
        "skills.marketProvider": "Market provider",
        "skills.marketSlug": "Market slug",
        "skills.noContentToOptimize": "No content",
        "skills.optimizeSuccess": "Optimized",
        "skills.optimizeWithAI": "Optimize",
        "skills.pleaseInputContent": "Content required",
        "skills.skillNamePlaceholder": "Skill name",
        "skills.sourceUrl": "Source URL",
        "skills.stopOptimize": "Stop optimize",
        "skills.updatedAt": "Updated at",
        "skills.viewSkill": "View skill",
      })[key] ?? key,
  }),
}));

vi.mock("../../../../api", () => ({
  api: {
    getSkill: vi.fn(),
    streamOptimizeSkill: vi.fn(),
  },
}));

vi.mock("../../../../components/InputWithExample", () => ({
  InputWithExample: ({
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
}));

vi.mock("../../../../components/MarkdownCopy/MarkdownCopy", () => ({
  MarkdownCopy: ({
    content,
    onContentChange,
    showCopyButton,
    textareaProps,
  }: {
    content: string;
    onContentChange?: (content: string) => void;
    showCopyButton?: boolean;
    textareaProps?: { placeholder?: string };
  }) => (
    <>
      <span>copy-button:{String(showCopyButton)}</span>
      <textarea
        aria-label="skill-content"
        placeholder={textareaProps?.placeholder}
        value={content}
        onChange={(event) => onContentChange?.(event.target.value)}
      />
    </>
  ),
}));

vi.mock("./SkillUpdateButton", () => ({
  SkillUpdateButton: () => <button type="button">Update skill</button>,
}));

const toastMock = vi.hoisted(() => ({
  error: vi.fn(),
  success: vi.fn(),
  warning: vi.fn(),
}));

vi.mock("@/ui", () => ({
  Btn: ({
    children,
    disabled,
    onClick,
  }: {
    children?: React.ReactNode;
    disabled?: boolean;
    onClick?: () => void;
  }) => (
    <button disabled={disabled} type="button" onClick={onClick}>
      {children}
    </button>
  ),
  Drawer: ({
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
  FormField: ({
    children,
    error,
    label,
  }: {
    children?: React.ReactNode;
    error?: string;
    label?: React.ReactNode;
  }) => (
    <label>
      {label}
      {children}
      {error && <span role="alert">{error}</span>}
    </label>
  ),
  Loading: ({ children }: { children?: React.ReactNode }) => <>{children}</>,
  TextInput: ({
    onChange,
    value,
  }: {
    onChange?: (event: React.ChangeEvent<HTMLInputElement>) => void;
    value?: string;
  }) => <input value={value ?? ""} onChange={onChange} />,
  toast: toastMock,
}));

const mockedApi = vi.mocked(api);

describe("SkillDrawer", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockedApi.getSkill.mockResolvedValue({
      name: "docs",
      content: "---\nname: docs\ndescription: Fresh docs\n---\nFresh",
      install_meta: {
        installed_from: "skills.sh",
        installed_version: "1.2.0",
        installed_at: "2026-06-18T10:00:00Z",
        updated_at: "2026-06-19T10:00:00Z",
        source_url: "https://skills.sh/org/docs",
        market_meta: {
          author: "Alice",
          name: "Docs Skill",
          slug: "org/docs",
          source: "clawhub",
        },
      },
    });
  });

  it("parses simple YAML frontmatter used by Rust skill specs", () => {
    expect(
      parseFrontmatter("---\nname: docs\ndescription: A: B\n---\nBody"),
    ).toEqual({ name: "docs", description: "A: B" });
    expect(parseFrontmatter("# no frontmatter")).toBeNull();
  });

  it("creates a skill from validated frontmatter content", async () => {
    const onSubmit = vi.fn();

    render(
      <SkillDrawer
        open
        editingSkill={null}
        onClose={vi.fn()}
        onSubmit={onSubmit}
      />,
    );

    fireEvent.change(screen.getByPlaceholderText("Skill name"), {
      target: { value: "docs" },
    });
    fireEvent.change(screen.getByLabelText("skill-content"), {
      target: {
        value: "---\nname: docs\ndescription: Docs\n---\nRead docs",
      },
    });
    fireEvent.click(screen.getByRole("button", { name: "Create" }));

    await waitFor(() => {
      expect(onSubmit).toHaveBeenCalledWith(
        expect.objectContaining({
          name: "docs",
          content: "---\nname: docs\ndescription: Docs\n---\nRead docs",
          source: "",
        }),
      );
    });
  });

  it("loads current Rust skill detail and renders install metadata", async () => {
    const onSubmit = vi.fn();

    render(
      <SkillDrawer
        open
        agentId="agent-a"
        editingSkill={{
          name: "docs",
          content: "---\nname: docs\ndescription: Old\n---\nOld",
          source: "workspace",
        }}
        onClose={vi.fn()}
        onSubmit={onSubmit}
      />,
    );

    await waitFor(() => {
      expect(mockedApi.getSkill).toHaveBeenCalledWith("docs", "agent-a");
      expect(screen.getByText("skills.sh")).toBeVisible();
      expect(screen.getByText("Skill Market")).toBeVisible();
      expect(screen.getByText("clawhub")).toBeVisible();
      expect(screen.getByText("Docs Skill")).toBeVisible();
      expect(screen.getByText("org/docs")).toBeVisible();
      expect(screen.getByText("1.2.0")).toBeVisible();
      expect(screen.getByText("Alice")).toBeVisible();
      expect(screen.getByText("https://skills.sh/org/docs")).toBeVisible();
      expect(screen.getByText("copy-button:false")).toBeVisible();
    });

    expect(screen.getByLabelText("skill-content")).toHaveValue(
      "---\nname: docs\ndescription: Fresh docs\n---\nFresh",
    );
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => {
      expect(onSubmit).toHaveBeenCalledWith(
        expect.objectContaining({
          name: "docs",
          content: "---\nname: docs\ndescription: Fresh docs\n---\nFresh",
          source: "workspace",
        }),
      );
    });
  });

  it("streams AI optimization through the Rust skill optimizer API", async () => {
    mockedApi.streamOptimizeSkill.mockImplementation(
      async (_content, onChunk: (chunk: string) => void) => {
        onChunk("---\nname: docs\n");
        onChunk("description: Optimized\n---\nBody");
      },
    );

    render(
      <SkillDrawer
        open
        editingSkill={null}
        onClose={vi.fn()}
        onSubmit={vi.fn()}
      />,
    );

    fireEvent.change(screen.getByLabelText("skill-content"), {
      target: {
        value: "---\nname: docs\ndescription: Draft\n---\nDraft",
      },
    });
    fireEvent.click(screen.getByRole("button", { name: "Optimize" }));

    await waitFor(() => {
      expect(mockedApi.streamOptimizeSkill).toHaveBeenCalledWith(
        "---\nname: docs\ndescription: Draft\n---\nDraft",
        expect.any(Function),
        expect.any(AbortSignal),
        "zh-CN",
      );
      expect(screen.getByLabelText("skill-content")).toHaveValue(
        "---\nname: docs\ndescription: Optimized\n---\nBody",
      );
      expect(toastMock.success).toHaveBeenCalledWith("Optimized");
    });
  });
});
