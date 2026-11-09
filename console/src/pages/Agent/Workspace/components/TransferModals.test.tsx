import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { LoadFromLibraryModal } from "./LoadFromLibraryModal";
import { SyncToLibraryModal } from "./SyncToLibraryModal";

const { listFilesMock } = vi.hoisted(() => ({
  listFilesMock: vi.fn(),
}));

vi.mock("../../../../api/modules/promptLibrary", () => ({
  promptLibraryApi: {
    listFiles: listFilesMock,
  },
}));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string) =>
      ({
        "common.cancel": "Cancel",
        "common.confirm": "Confirm",
        "common.loading": "Loading",
        "promptLibrary.noFiles": "No library files",
        "promptLibrary.selectFiles": "Select library files",
        "skills.clearSelection": "Clear selection",
        "skills.selectAll": "Select all",
        "workspace.batchSyncToLibrary": "Sync to library",
        "workspace.loadFromLibrary": "Load from library",
        "workspace.selectSyncFiles": "Select workspace files",
      })[key] ?? key,
  }),
}));

vi.mock("@/ui", () => ({
  Btn: ({
    children,
    disabled,
    loading,
    onClick,
  }: {
    children: React.ReactNode;
    disabled?: boolean;
    loading?: boolean;
    onClick?: () => void;
  }) => (
    <button disabled={disabled || loading} type="button" onClick={onClick}>
      {children}
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
}));

const workspaceFiles = [
  { filename: "ROOT.md", size: 10 },
  { filename: "AGENT.md", size: 20 },
] satisfies Parameters<typeof SyncToLibraryModal>[0]["files"];

describe("workspace transfer modals", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    listFilesMock.mockResolvedValue([
      { filename: "LIBRARY.md", size: 30 },
      { filename: "WRITING.md", size: 40 },
    ]);
  });

  it("syncs selected workspace prompt files to the Rust prompt library", async () => {
    const onConfirm = vi.fn().mockResolvedValue(undefined);

    render(
      <SyncToLibraryModal
        files={workspaceFiles}
        open
        onClose={vi.fn()}
        onConfirm={onConfirm}
      />,
    );

    expect(screen.getByRole("button", { name: "Confirm" })).toBeDisabled();

    fireEvent.click(screen.getByText("ROOT.md"));
    fireEvent.click(screen.getByRole("button", { name: "Confirm" }));

    await waitFor(() => {
      expect(onConfirm).toHaveBeenCalledWith(["ROOT.md"]);
    });
  });

  it("supports select-all and clear-selection for workspace sync", async () => {
    const onConfirm = vi.fn().mockResolvedValue(undefined);

    render(
      <SyncToLibraryModal
        files={workspaceFiles}
        open
        onClose={vi.fn()}
        onConfirm={onConfirm}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "Select all" }));
    fireEvent.click(screen.getByRole("button", { name: "Clear selection" }));
    expect(screen.getByRole("button", { name: "Confirm" })).toBeDisabled();

    fireEvent.click(screen.getByRole("button", { name: "Select all" }));
    fireEvent.click(screen.getByRole("button", { name: "Confirm" }));

    await waitFor(() => {
      expect(onConfirm).toHaveBeenCalledWith(["ROOT.md", "AGENT.md"]);
    });
  });

  it("loads prompt-library files before broadcasting them into the agent workspace", async () => {
    const onConfirm = vi.fn().mockResolvedValue(undefined);

    render(
      <LoadFromLibraryModal open onClose={vi.fn()} onConfirm={onConfirm} />,
    );

    expect(screen.getByText("Loading")).toBeVisible();
    expect(await screen.findByText("LIBRARY.md")).toBeVisible();
    fireEvent.click(screen.getByText("WRITING.md"));
    fireEvent.click(screen.getByRole("button", { name: "Confirm" }));

    await waitFor(() => {
      expect(listFilesMock).toHaveBeenCalledTimes(1);
      expect(onConfirm).toHaveBeenCalledWith(["WRITING.md"]);
    });
  });

  it("renders an empty library state when prompt-library listing fails", async () => {
    listFilesMock.mockRejectedValueOnce(new Error("unavailable"));

    render(<LoadFromLibraryModal open onClose={vi.fn()} onConfirm={vi.fn()} />);

    expect(await screen.findByText("No library files")).toBeVisible();
    expect(screen.getByRole("button", { name: "Confirm" })).toBeDisabled();
  });
});
