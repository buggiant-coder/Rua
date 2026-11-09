import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { FileEditor } from "./FileEditor";

const { copyTextMock, toastMock } = vi.hoisted(() => ({
  copyTextMock: vi.fn(),
  toastMock: {
    error: vi.fn(),
    success: vi.fn(),
    warning: vi.fn(),
  },
}));

vi.mock("../../../../utils/clipboard", () => ({
  copyText: copyTextMock,
}));

vi.mock("@/components/EmptyState", () => ({
  EmptyState: ({ description }: { description: string }) => (
    <div>{description}</div>
  ),
}));

vi.mock("@/components/RuaMarkdown", () => ({
  RuaMarkdown: ({ content }: { content: string }) => (
    <article>{content}</article>
  ),
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
      {children ?? variant ?? "button"}
    </button>
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
  TextArea: ({
    onChange,
    placeholder,
    readOnly,
    value,
  }: {
    onChange: (event: React.ChangeEvent<HTMLTextAreaElement>) => void;
    placeholder?: string;
    readOnly?: boolean;
    value: string;
  }) => (
    <textarea
      placeholder={placeholder}
      readOnly={readOnly}
      value={value}
      onChange={onChange}
    />
  ),
  TextInput: ({
    onBlur,
    onChange,
    onKeyDown,
    value,
  }: {
    onBlur?: () => void;
    onChange: (event: React.ChangeEvent<HTMLInputElement>) => void;
    onKeyDown?: (event: React.KeyboardEvent<HTMLInputElement>) => void;
    value: string;
  }) => (
    <input
      value={value}
      onBlur={onBlur}
      onChange={onChange}
      onKeyDown={onKeyDown}
    />
  ),
  Toggle: ({
    checked,
    onChange,
  }: {
    checked: boolean;
    onChange: (event: React.ChangeEvent<HTMLInputElement>) => void;
  }) => <input checked={checked} type="checkbox" onChange={onChange} />,
  Tooltip: ({ children }: { children: React.ReactNode }) => <>{children}</>,
  toast: toastMock,
}));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string, params?: unknown) => {
      if (key === "workspace.deleteConfirm") {
        const name =
          params && typeof params === "object" && "name" in params
            ? String((params as { name: unknown }).name)
            : "";
        return `Delete ${name}?`;
      }
      return (
        {
          "common.cancel": "Cancel",
          "common.copied": "Copied",
          "common.copyFailed": "Copy failed",
          "common.delete": "Delete",
          "common.preview": "Preview",
          "common.rename": "Rename",
          "common.reset": "Reset",
          "common.save": "Save",
          "workspace.deleteEnabledWarning": "Enabled files cannot be deleted",
          "workspace.deleteSubdirWarning": "Subdir will also be deleted",
          "workspace.fileContent": "File content",
          "workspace.fileContentEmpty": "Empty file",
          "workspace.renameEnabledWarning": "Enabled files cannot be renamed",
          "workspace.readonlyPrompt": "Read-only",
          "workspace.selectFile": "Select a file",
          "workspace.syncToLibrary": "Sync to library",
        }[key] ?? key
      );
    },
  }),
}));

const selectedFile = {
  builtin: false,
  created_time: "",
  filename: "AGENT.md",
  modified_time: "",
  path: "/workspace/AGENT.md",
  size: 12,
  updated_at: 0,
};

function renderEditor(
  overrides: Partial<React.ComponentProps<typeof FileEditor>> = {},
) {
  return render(
    <FileEditor
      fileContent={"---\ntitle: Hidden\n---\n# Visible prompt"}
      hasChanges={true}
      loading={false}
      onContentChange={vi.fn()}
      onDelete={vi.fn()}
      onRename={vi.fn()}
      onReset={vi.fn()}
      onSave={vi.fn()}
      selectedFile={selectedFile}
      {...overrides}
    />,
  );
}

describe("FileEditor", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    copyTextMock.mockResolvedValue(true);
  });

  it("renders markdown preview without frontmatter and copies the raw file content", async () => {
    renderEditor();

    expect(screen.getByText("# Visible prompt")).toBeVisible();
    expect(screen.queryByText(/title: Hidden/)).not.toBeInTheDocument();

    fireEvent.click(screen.getAllByRole("button", { name: "outline" })[0]);

    await waitFor(() => {
      expect(copyTextMock).toHaveBeenCalledWith(
        "---\ntitle: Hidden\n---\n# Visible prompt",
      );
    });
    expect(toastMock.success).toHaveBeenCalledWith("Copied");
  });

  it("switches markdown files into edit mode and reports content changes", () => {
    const onContentChange = vi.fn();
    renderEditor({ onContentChange });

    fireEvent.click(screen.getByRole("checkbox"));
    fireEvent.change(screen.getByPlaceholderText("File content"), {
      target: { value: "# Changed" },
    });

    expect(onContentChange).toHaveBeenCalledWith("# Changed");
  });

  it("renames files by appending the markdown extension", async () => {
    const onRename = vi.fn();
    renderEditor({ onRename });

    fireEvent.click(screen.getByText("AGENT.md"));
    const input = screen.getByDisplayValue("AGENT");
    fireEvent.change(input, { target: { value: "CODING" } });
    fireEvent.blur(input);

    await waitFor(() => {
      expect(onRename).toHaveBeenCalledWith("CODING.md");
    });
  });

  it("protects enabled files from rename and delete actions", () => {
    const onDelete = vi.fn();
    const onRename = vi.fn();
    renderEditor({ isFileEnabled: true, onDelete, onRename });

    fireEvent.click(screen.getByText("AGENT.md"));
    fireEvent.click(screen.getByRole("button", { name: "danger" }));

    expect(onRename).not.toHaveBeenCalled();
    expect(onDelete).not.toHaveBeenCalled();
    expect(toastMock.warning).toHaveBeenCalledWith(
      "Enabled files cannot be renamed",
    );
    expect(toastMock.warning).toHaveBeenCalledWith(
      "Enabled files cannot be deleted",
    );
  });

  it("renders read-only prompt files without mutation actions", () => {
    const onContentChange = vi.fn();
    const onDelete = vi.fn();
    const onRename = vi.fn();
    const onSave = vi.fn();
    renderEditor({
      readOnly: true,
      onContentChange,
      onDelete,
      onRename,
      onSave,
    });

    expect(screen.getByText("Read-only")).toBeVisible();
    expect(screen.queryByRole("button", { name: "danger" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Save" })).toBeNull();

    fireEvent.click(screen.getByRole("checkbox"));
    const textarea = screen.getByPlaceholderText("File content");
    expect(textarea).toHaveAttribute("readonly");
    fireEvent.change(textarea, { target: { value: "# Changed" } });

    expect(onContentChange).not.toHaveBeenCalled();
    expect(onDelete).not.toHaveBeenCalled();
    expect(onRename).not.toHaveBeenCalled();
    expect(onSave).not.toHaveBeenCalled();
  });

  it("confirms deletion for non-empty files and deletes empty files directly", () => {
    const onDelete = vi.fn();
    const { rerender } = renderEditor({ onDelete });

    fireEvent.click(screen.getByRole("button", { name: "danger" }));
    expect(screen.getByText("Delete AGENT.md?")).toBeVisible();
    fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    expect(onDelete).toHaveBeenCalledTimes(1);

    rerender(
      <FileEditor
        fileContent=""
        hasChanges={false}
        loading={false}
        onContentChange={vi.fn()}
        onDelete={onDelete}
        onReset={vi.fn()}
        onSave={vi.fn()}
        selectedFile={selectedFile}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "danger" }));
    expect(onDelete).toHaveBeenCalledTimes(2);
  });
});
