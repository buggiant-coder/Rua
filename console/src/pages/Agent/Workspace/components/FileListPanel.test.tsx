import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { FileListPanel } from "./FileListPanel";

vi.mock("@dnd-kit/core", () => ({
  DndContext: ({ children }: { children: React.ReactNode }) => <>{children}</>,
  PointerSensor: vi.fn(),
  closestCenter: vi.fn(),
  useSensor: vi.fn(() => ({})),
  useSensors: vi.fn(() => []),
}));

vi.mock("@dnd-kit/sortable", () => ({
  SortableContext: ({ children }: { children: React.ReactNode }) => (
    <>{children}</>
  ),
  arrayMove: (items: string[], from: number, to: number) => {
    const next = [...items];
    const [item] = next.splice(from, 1);
    next.splice(to, 0, item);
    return next;
  },
  verticalListSortingStrategy: {},
}));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string) =>
      ({
        "workspace.collapseAll": "Collapse all",
        "workspace.expandAll": "Expand all",
        "workspace.loadedPrompts": "Loaded prompts",
        "workspace.noEnabledFiles": "No enabled prompts",
        "workspace.noFiles": "No files",
        "workspace.promptDesc": "Prompt files",
        "workspace.sort": "Sort",
        "workspace.sortNameAsc": "Name asc",
        "workspace.sortNameDesc": "Name desc",
        "workspace.sortTimeAsc": "Time asc",
        "workspace.sortTimeDesc": "Time desc",
      })[key] ?? key,
  }),
}));

vi.mock("@/ui", () => ({
  Btn: ({
    children,
    id,
    onClick,
  }: {
    children?: React.ReactNode;
    id?: string;
    onClick?: () => void;
  }) => (
    <button id={id} type="button" onClick={onClick}>
      {children ?? id ?? "action"}
    </button>
  ),
  Dropdown: ({
    children,
    menu,
  }: {
    children: React.ReactNode;
    menu?: {
      items?: Array<{ key: string; label: string }>;
      onClick?: (event: { key: string }) => void;
    };
  }) => (
    <div>
      {children}
      {menu?.items?.map((item) => (
        <button
          key={item.key}
          type="button"
          onClick={() => menu.onClick?.({ key: item.key })}
        >
          {item.label}
        </button>
      ))}
    </div>
  ),
  Tooltip: ({ children }: { children: React.ReactNode }) => <>{children}</>,
}));

vi.mock("./EnabledFileItem", () => ({
  EnabledFileItem: ({
    enabled,
    entry,
    fileSize,
    label,
    onClick,
    onToggle,
    readOnly,
  }: {
    enabled: boolean;
    entry: string;
    fileSize: number | null;
    label?: string;
    onClick: (entry: string) => void;
    onToggle: (entry: string) => void;
    readOnly?: boolean;
  }) => (
    <div>
      <span>
        loaded:{label ?? entry}:{fileSize ?? "missing"}:{String(enabled)}:
        {String(Boolean(readOnly))}
      </span>
      <button type="button" onClick={() => onClick(entry)}>
        Open loaded {entry}
      </button>
      {!readOnly && (
        <button type="button" onClick={() => onToggle(entry)}>
          Toggle loaded {entry}
        </button>
      )}
    </div>
  ),
}));

vi.mock("./FileItem", () => ({
  FileItem: ({
    batchSelected,
    file,
    isExpanded,
    onFileClick,
    onSubdirFileClick,
    onToggleBatchSelect,
    subdirFiles,
  }: {
    batchSelected: boolean;
    file: { filename: string };
    isExpanded: boolean;
    onFileClick: (file: { filename: string }) => void;
    onSubdirFileClick: (
      parentFilename: string,
      file: { filename: string },
    ) => void;
    onToggleBatchSelect: (filename: string) => void;
    subdirFiles: Array<{ filename: string }>;
  }) => (
    <div>
      <span>
        file:{file.filename}:{String(isExpanded)}:{String(batchSelected)}
      </span>
      <button type="button" onClick={() => onFileClick(file)}>
        Open file {file.filename}
      </button>
      <button type="button" onClick={() => onToggleBatchSelect(file.filename)}>
        Select file {file.filename}
      </button>
      {subdirFiles.map((subdir) => (
        <button
          key={subdir.filename}
          type="button"
          onClick={() => onSubdirFileClick(file.filename, subdir)}
        >
          Open subdir {file.filename}/{subdir.filename}
        </button>
      ))}
    </div>
  ),
}));

const files = [
  { filename: "ROOT.md", size: 10 },
  { filename: "AGENT.md", size: 20 },
] as Parameters<typeof FileListPanel>[0]["files"];

const subdirFiles = {
  "AGENT.md": [{ filename: "zh.md", size: 5, subdir: "AGENT" }],
} as Parameters<typeof FileListPanel>[0]["subdirFiles"];

function renderPanel(
  overrides: Partial<Parameters<typeof FileListPanel>[0]> = {},
) {
  return render(
    <FileListPanel
      batchDeleteMode={false}
      batchSelected={new Set()}
      displayedLoadedFiles={["ROOT.md", "AGENT/zh.md"]}
      enabledFiles={["ROOT.md", "AGENT/zh.md"]}
      expandedDirs={new Set(["AGENT.md"])}
      files={files}
      filesWithSubdirs={new Set(["AGENT.md"])}
      onCreateSubdirFile={vi.fn()}
      onFileClick={vi.fn()}
      onRefresh={vi.fn()}
      onReorder={vi.fn()}
      onSortModeChange={vi.fn()}
      onSubdirFileClick={vi.fn()}
      onToggleBatchSelect={vi.fn()}
      onToggleEnabled={vi.fn()}
      onToggleExpandAll={vi.fn()}
      selectedFile={null}
      sortMode="name-asc"
      subdirFiles={subdirFiles}
      workspacePath="/tmp/workspace"
      {...overrides}
    />,
  );
}

describe("FileListPanel", () => {
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

  it("routes enabled root and subdir prompt clicks to the Rust workspace handlers", () => {
    const onFileClick = vi.fn();
    const onSubdirFileClick = vi.fn();
    const onToggleEnabled = vi.fn();
    renderPanel({ onFileClick, onSubdirFileClick, onToggleEnabled });

    expect(screen.getByText("loaded:ROOT.md:10:true:false")).toBeVisible();
    expect(screen.getByText("loaded:AGENT/zh.md:5:true:false")).toBeVisible();

    fireEvent.click(
      screen.getByRole("button", { name: "Open loaded ROOT.md" }),
    );
    fireEvent.click(
      screen.getByRole("button", { name: "Open loaded AGENT/zh.md" }),
    );
    fireEvent.click(
      screen.getByRole("button", { name: "Toggle loaded AGENT/zh.md" }),
    );

    expect(onFileClick).toHaveBeenCalledWith(files[0]);
    expect(onSubdirFileClick).toHaveBeenCalledWith(
      "AGENT.md",
      subdirFiles["AGENT.md"][0],
    );
    expect(onToggleEnabled).toHaveBeenCalledWith("AGENT/zh.md");
  });

  it("delegates sort, expand-all, and refresh controls", () => {
    const onRefresh = vi.fn();
    const onSortModeChange = vi.fn();
    const onToggleExpandAll = vi.fn();
    renderPanel({ onRefresh, onSortModeChange, onToggleExpandAll });

    fireEvent.click(screen.getByRole("button", { name: "Name desc" }));
    fireEvent.click(screen.getAllByRole("button", { name: "action" })[2]);
    fireEvent.click(screen.getByRole("button", { name: "prompt_list_reload" }));

    expect(onSortModeChange).toHaveBeenCalledWith("name-desc");
    expect(onToggleExpandAll).toHaveBeenCalledTimes(1);
    expect(onRefresh).toHaveBeenCalled();
  });

  it("renders empty states and batch selection flags", () => {
    renderPanel({
      batchDeleteMode: true,
      batchSelected: new Set(["ROOT.md"]),
      displayedLoadedFiles: [],
      files: [],
    });

    expect(screen.getByText("No enabled prompts")).toBeVisible();
    expect(screen.getByText("No files")).toBeVisible();
  });

  it("passes full-file-list clicks and batch selection to FileItem rows", () => {
    const onFileClick = vi.fn();
    const onSubdirFileClick = vi.fn();
    const onToggleBatchSelect = vi.fn();
    renderPanel({
      batchDeleteMode: true,
      batchSelected: new Set(["ROOT.md"]),
      onFileClick,
      onSubdirFileClick,
      onToggleBatchSelect,
    });

    expect(screen.getByText("file:ROOT.md:false:true")).toBeVisible();
    expect(screen.getByText("file:AGENT.md:true:false")).toBeVisible();

    fireEvent.click(screen.getByRole("button", { name: "Open file ROOT.md" }));
    fireEvent.click(
      screen.getByRole("button", { name: "Open subdir AGENT.md/zh.md" }),
    );
    fireEvent.click(
      screen.getByRole("button", { name: "Select file ROOT.md" }),
    );

    expect(onFileClick).toHaveBeenCalledWith(files[0]);
    expect(onSubdirFileClick).toHaveBeenCalledWith(
      "AGENT.md",
      subdirFiles["AGENT.md"][0],
    );
    expect(onToggleBatchSelect).toHaveBeenCalledWith("ROOT.md");
  });
});
