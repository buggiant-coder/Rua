import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { EnabledFileItem } from "./EnabledFileItem";
import { FileItem } from "./FileItem";
import type { MarkdownFile, SubdirFile } from "../../../../api/types";

vi.mock("@dnd-kit/sortable", () => ({
  useSortable: () => ({
    attributes: {},
    isDragging: false,
    listeners: {},
    setNodeRef: vi.fn(),
    transform: null,
    transition: undefined,
  }),
}));

vi.mock("@dnd-kit/utilities", () => ({
  CSS: {
    Transform: {
      toString: () => "",
    },
  },
}));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string) =>
      ({
        "workspace.createSubFile": "Create sub-file",
        "workspace.systemPromptToggleTooltip": "Toggle system prompt",
      })[key] ?? key,
  }),
}));

vi.mock("@/ui", () => ({
  Toggle: ({
    checked,
    onChange,
  }: {
    checked: boolean;
    onChange: (event: React.ChangeEvent<HTMLInputElement>) => void;
  }) => <input checked={checked} type="checkbox" onChange={onChange} />,
  Tooltip: ({ children }: { children: React.ReactNode }) => <>{children}</>,
}));

const rootFile = {
  builtin: false,
  created_time: "",
  filename: "ROOT.md",
  modified_time: "",
  path: "/workspace/ROOT.md",
  size: 1536,
  updated_at: 0,
} satisfies MarkdownFile;

const agentFile = {
  ...rootFile,
  filename: "AGENT.md",
  size: 10,
} satisfies MarkdownFile;

const subdirFile = {
  builtin: false,
  created_time: "",
  filename: "zh.md",
  modified_time: "",
  path: "/workspace/AGENT/zh.md",
  size: 5,
  subdir: "AGENT",
  updated_at: 0,
} satisfies SubdirFile;

describe("workspace prompt file rows", () => {
  it("opens and toggles enabled prompt entries without conflating the actions", () => {
    const onClick = vi.fn();
    const onToggle = vi.fn();

    render(
      <EnabledFileItem
        enabled
        entry="AGENT/zh.md"
        fileSize={1536}
        onClick={onClick}
        onToggle={onToggle}
      />,
    );

    expect(screen.getByText("AGENT/zh")).toBeVisible();
    expect(screen.getByText("1.5 KB")).toBeVisible();

    fireEvent.click(screen.getByText("AGENT/zh"));
    fireEvent.click(screen.getByRole("checkbox"));

    expect(onClick).toHaveBeenCalledWith("AGENT/zh.md");
    expect(onToggle).toHaveBeenCalledWith("AGENT/zh.md");
    expect(onClick).toHaveBeenCalledTimes(1);
  });

  it("shows root prompt file state and delegates row, toggle, batch, and create-subdir actions", () => {
    const onCreateSubdirFile = vi.fn();
    const onFileClick = vi.fn();
    const onToggleBatchSelect = vi.fn();
    const onToggleEnabled = vi.fn();

    render(
      <FileItem
        batchDeleteMode
        batchSelected
        enabled={false}
        enabledFiles={[]}
        file={rootFile}
        hasSubdir={false}
        isExpanded={false}
        onCreateSubdirFile={onCreateSubdirFile}
        onFileClick={onFileClick}
        onSubdirFileClick={vi.fn()}
        onToggleBatchSelect={onToggleBatchSelect}
        onToggleEnabled={onToggleEnabled}
        selectedFile={rootFile}
        subdirFiles={[]}
      />,
    );

    expect(screen.getByText("ROOT")).toBeVisible();
    expect(screen.getByText("1.5 KB")).toBeVisible();

    fireEvent.click(screen.getByText("ROOT"));
    fireEvent.click(screen.getByRole("checkbox"));
    fireEvent.click(document.querySelector("[class*='batchCheckbox']")!);
    fireEvent.click(document.querySelector("[class*='createSubdirBtn']")!);

    expect(onFileClick).toHaveBeenCalledWith(rootFile);
    expect(onToggleEnabled).toHaveBeenCalledWith("ROOT.md");
    expect(onToggleBatchSelect).toHaveBeenCalledWith("ROOT.md");
    expect(onCreateSubdirFile).toHaveBeenCalledWith("ROOT.md");
  });

  it("uses Rust subdir enabled keys for expanded prompt files", () => {
    const onSubdirFileClick = vi.fn();
    const onToggleEnabled = vi.fn();
    const onToggleBatchSelect = vi.fn();

    render(
      <FileItem
        batchDeleteMode
        batchSelectedSubs={new Set(["AGENT/zh.md"])}
        enabled={false}
        enabledFiles={["AGENT/zh.md"]}
        file={agentFile}
        hasSubdir
        isExpanded
        onCreateSubdirFile={vi.fn()}
        onFileClick={vi.fn()}
        onSubdirFileClick={onSubdirFileClick}
        onToggleBatchSelect={onToggleBatchSelect}
        onToggleEnabled={onToggleEnabled}
        selectedFile={subdirFile}
        subdirFiles={[subdirFile]}
      />,
    );

    expect(screen.getByText("AGENT")).toBeVisible();
    expect(screen.getByText("zh")).toBeVisible();
    fireEvent.click(screen.getByText("zh"));
    fireEvent.click(screen.getAllByRole("checkbox")[1]);

    expect(onSubdirFileClick).toHaveBeenCalledWith("AGENT.md", subdirFile);
    expect(onToggleEnabled).toHaveBeenCalledWith("AGENT/zh.md");
    expect(onToggleBatchSelect).not.toHaveBeenCalled();
  });
});
