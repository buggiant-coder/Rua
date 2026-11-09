import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { PoolTransferModal } from "./PoolTransferModal";

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string) =>
      ({
        "agent.selectBuiltin": "Select builtin",
        "common.cancel": "Cancel",
        "common.confirm": "Confirm",
        "skills.clearSelection": "Clear selection",
        "skills.downloadFromPool": "Download from pool",
        "skills.selectAll": "Select all",
        "skills.selectPoolItem": "Select pool skills",
        "skills.selectWorkspaceSkill": "Select workspace skills",
        "skills.uploadToPool": "Upload to pool",
      })[key] ?? key,
  }),
}));

vi.mock("@/ui", () => ({
  Btn: ({
    children,
    disabled,
    onClick,
  }: {
    children: React.ReactNode;
    disabled?: boolean;
    onClick?: () => void;
  }) => (
    <button disabled={disabled} type="button" onClick={onClick}>
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
  Tooltip: ({ children }: { children: React.ReactNode }) => <>{children}</>,
}));

const workspaceSkills = [
  { name: "docs", source: "workspace" },
  { name: "writer", source: "builtin" },
] satisfies Parameters<typeof PoolTransferModal>[0]["skills"];

const poolSkills = [
  { name: "review", source: "pool" },
  { name: "builtin-helper", source: "builtin" },
] satisfies Parameters<typeof PoolTransferModal>[0]["poolSkills"];

describe("PoolTransferModal", () => {
  it("uploads selected workspace skills into the Rust skill pool", async () => {
    const onUpload = vi.fn().mockResolvedValue(undefined);

    render(
      <PoolTransferModal
        mode="upload"
        skills={workspaceSkills}
        poolSkills={poolSkills}
        onClose={vi.fn()}
        onUpload={onUpload}
        onDownload={vi.fn()}
      />,
    );

    expect(screen.getByRole("button", { name: "Confirm" })).toBeDisabled();
    fireEvent.click(screen.getByText("docs"));
    fireEvent.click(screen.getByRole("button", { name: "Confirm" }));

    await waitFor(() => {
      expect(onUpload).toHaveBeenCalledWith(["docs"]);
    });
  });

  it("downloads builtin-only pool skills to the selected Rust workspace", async () => {
    const onDownload = vi.fn().mockResolvedValue(undefined);

    render(
      <PoolTransferModal
        mode="download"
        skills={workspaceSkills}
        poolSkills={poolSkills}
        onClose={vi.fn()}
        onUpload={vi.fn()}
        onDownload={onDownload}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "Select builtin" }));
    fireEvent.click(screen.getByRole("button", { name: "Confirm" }));

    await waitFor(() => {
      expect(onDownload).toHaveBeenCalledWith(["builtin-helper"]);
    });
  });

  it("resets selections when switching transfer modes", () => {
    const onUpload = vi.fn();
    const { rerender } = render(
      <PoolTransferModal
        mode="upload"
        skills={workspaceSkills}
        poolSkills={poolSkills}
        onClose={vi.fn()}
        onUpload={onUpload}
        onDownload={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByText("docs"));
    expect(screen.getByRole("button", { name: "Confirm" })).toBeEnabled();

    rerender(
      <PoolTransferModal
        mode="download"
        skills={workspaceSkills}
        poolSkills={poolSkills}
        onClose={vi.fn()}
        onUpload={onUpload}
        onDownload={vi.fn()}
      />,
    );

    expect(screen.getByRole("button", { name: "Confirm" })).toBeDisabled();
  });
});
