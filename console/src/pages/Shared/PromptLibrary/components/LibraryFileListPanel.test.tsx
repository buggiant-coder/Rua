import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { LibraryFileItem } from "./LibraryFileItem";
import { LibraryFileListPanel } from "./LibraryFileListPanel";
import type { MarkdownFile, SubdirFile } from "../../../../api/types";

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string) =>
      ({
        "promptLibrary.fileListDesc": "Prompt library files",
        "promptLibrary.noFiles": "No library files",
        "workspace.collapseAll": "Collapse all",
        "workspace.createSubFile": "Create sub-file",
        "workspace.expandAll": "Expand all",
      })[key] ?? key,
  }),
}));

vi.mock("@/ui", () => ({
  Btn: ({ onClick }: { onClick?: () => void }) => (
    <button type="button" onClick={onClick}>
      action
    </button>
  ),
  Tooltip: ({ children }: { children: React.ReactNode }) => <>{children}</>,
}));

const rootFile = {
  builtin: true,
  created_time: "",
  filename: "ROOT.md",
  modified_time: "",
  path: "/library/ROOT.md",
  size: 1536,
  updated_at: 0,
} satisfies MarkdownFile;

const agentFile = {
  ...rootFile,
  builtin: false,
  filename: "AGENT.md",
  size: 10,
} satisfies MarkdownFile;

const subdirFile = {
  builtin: false,
  created_time: "",
  filename: "zh.md",
  modified_time: "",
  path: "/library/AGENT/zh.md",
  size: 5,
  subdir: "AGENT",
  updated_at: 0,
} satisfies SubdirFile;

describe("LibraryFileItem", () => {
  it("delegates root, expand, create-subdir, and batch-selection actions", () => {
    const onCreateSubdirFile = vi.fn();
    const onFileClick = vi.fn();
    const onToggleBatchSelect = vi.fn();
    const onToggleExpand = vi.fn();

    render(
      <LibraryFileItem
        batchDeleteMode
        batchSelected
        batchSelectedSubs={new Set()}
        file={rootFile}
        hasSubdir
        isExpanded={false}
        onCreateSubdirFile={onCreateSubdirFile}
        onFileClick={onFileClick}
        onSubdirFileClick={vi.fn()}
        onToggleBatchSelect={onToggleBatchSelect}
        onToggleExpand={onToggleExpand}
        selectedFile={rootFile}
        subdirFiles={[]}
      />,
    );

    expect(screen.getByText("ROOT")).toBeVisible();
    expect(screen.getByText("内置")).toBeVisible();
    expect(screen.getByText("1.5 KB")).toBeVisible();

    fireEvent.click(screen.getByText("ROOT"));
    fireEvent.click(screen.getByRole("button", { name: "expand" }));
    fireEvent.click(document.querySelector("[class*='batchCheckbox']")!);
    fireEvent.click(document.querySelector("[class*='createSubdirBtn']")!);

    expect(onFileClick).toHaveBeenCalledWith(rootFile);
    expect(onToggleExpand).toHaveBeenCalledWith("ROOT.md");
    expect(onToggleBatchSelect).toHaveBeenCalledWith("ROOT.md");
    expect(onCreateSubdirFile).toHaveBeenCalledWith("ROOT.md");
  });

  it("uses Rust prompt-library subdir keys for expanded rows", () => {
    const onSubdirFileClick = vi.fn();
    const onToggleBatchSelect = vi.fn();

    render(
      <LibraryFileItem
        batchDeleteMode
        batchSelected={false}
        batchSelectedSubs={new Set(["AGENT/zh.md"])}
        file={agentFile}
        hasSubdir
        isExpanded
        onCreateSubdirFile={vi.fn()}
        onFileClick={vi.fn()}
        onSubdirFileClick={onSubdirFileClick}
        onToggleBatchSelect={onToggleBatchSelect}
        onToggleExpand={vi.fn()}
        selectedFile={subdirFile}
        subdirFiles={[subdirFile]}
      />,
    );

    fireEvent.click(screen.getByText("zh"));
    fireEvent.click(document.querySelectorAll("[class*='batchCheckbox']")[1]);

    expect(onSubdirFileClick).toHaveBeenCalledWith("AGENT.md", subdirFile);
    expect(onToggleBatchSelect).toHaveBeenCalledWith("AGENT/zh.md");
  });
});

describe("LibraryFileListPanel", () => {
  beforeEach(() => {
    vi.stubGlobal(
      "ResizeObserver",
      class {
        disconnect() {}
        observe() {}
      },
    );
    vi.stubGlobal(
      "MutationObserver",
      class {
        disconnect() {}
        observe() {}
      },
    );
  });

  it("renders prompt-library rows and delegates expand-all", () => {
    const onToggleExpandAll = vi.fn();

    render(
      <LibraryFileListPanel
        batchDeleteMode={false}
        batchSelected={new Set()}
        expandedDirs={new Set(["AGENT.md"])}
        files={[rootFile, agentFile]}
        filesWithSubdirs={new Set(["AGENT.md"])}
        onCreateSubdirFile={vi.fn()}
        onFileClick={vi.fn()}
        onSubdirFileClick={vi.fn()}
        onToggleBatchSelect={vi.fn()}
        onToggleExpand={vi.fn()}
        onToggleExpandAll={onToggleExpandAll}
        selectedFile={null}
        subdirFiles={{ "AGENT.md": [subdirFile] }}
      />,
    );

    expect(screen.getByText("Prompt library files")).toBeVisible();
    expect(screen.getByText("ROOT")).toBeVisible();
    expect(screen.getByText("AGENT")).toBeVisible();
    expect(screen.getByText("zh")).toBeVisible();

    fireEvent.click(screen.getByRole("button", { name: "action" }));
    expect(onToggleExpandAll).toHaveBeenCalledTimes(1);
  });

  it("renders an empty library state", () => {
    render(
      <LibraryFileListPanel
        batchDeleteMode={false}
        batchSelected={new Set()}
        expandedDirs={new Set()}
        files={[]}
        filesWithSubdirs={new Set()}
        onCreateSubdirFile={vi.fn()}
        onFileClick={vi.fn()}
        onSubdirFileClick={vi.fn()}
        onToggleBatchSelect={vi.fn()}
        onToggleExpand={vi.fn()}
        onToggleExpandAll={vi.fn()}
        selectedFile={null}
        subdirFiles={{}}
      />,
    );

    expect(screen.getByText("No library files")).toBeVisible();
  });
});
