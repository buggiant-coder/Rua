import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { ImportBuiltinModal } from "./ImportBuiltinModal";

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string) =>
      ({
        "agent.selectAll": "Select all",
        "skillPool.currentVersion": "Current version",
        "skillPool.importBuiltin": "Import builtins",
        "skillPool.importBuiltinHint": "Choose builtin skills",
        "skillPool.importStatusConflict": "Conflict",
        "skillPool.importStatusCurrent": "Current",
        "skillPool.importStatusMissing": "Missing",
        "skillPool.sourceVersion": "Source version",
        "skills.clearSelection": "Clear selection",
      })[key] ?? key,
  }),
}));

vi.mock("@/ui", () => ({
  Btn: ({
    children,
    onClick,
  }: {
    children: React.ReactNode;
    onClick?: () => void;
  }) => (
    <button type="button" onClick={onClick}>
      {children}
    </button>
  ),
  Modal: ({
    children,
    okButtonProps,
    onClose,
    onOk,
    open,
    title,
  }: {
    children?: React.ReactNode;
    okButtonProps?: { disabled?: boolean; loading?: boolean };
    onClose?: () => void;
    onOk?: () => void;
    open?: boolean;
    title?: React.ReactNode;
  }) =>
    open ? (
      <div>
        <h2>{title}</h2>
        {children}
        <button
          disabled={okButtonProps?.disabled || okButtonProps?.loading}
          type="button"
          onClick={onOk}
        >
          Confirm
        </button>
        <button type="button" onClick={onClose}>
          Cancel
        </button>
      </div>
    ) : null,
}));

const sources = [
  {
    current_version_text: "",
    name: "review",
    status: "missing",
    version_text: "1.0.0",
  },
  {
    current_version_text: "1.0.0",
    name: "writer",
    status: "conflict",
    version_text: "2.0.0",
  },
] satisfies Parameters<typeof ImportBuiltinModal>[0]["sources"];

describe("ImportBuiltinModal", () => {
  it("imports selected builtin pool sources", async () => {
    const onConfirm = vi.fn().mockResolvedValue(undefined);

    render(
      <ImportBuiltinModal
        loading={false}
        open
        sources={sources}
        onClose={vi.fn()}
        onConfirm={onConfirm}
      />,
    );

    expect(screen.getByRole("button", { name: "Confirm" })).toBeDisabled();
    expect(screen.getByText("review")).toBeVisible();
    expect(screen.getByText("writer")).toBeVisible();
    expect(screen.getByText("Missing")).toBeVisible();
    expect(screen.getByText("Conflict")).toBeVisible();

    fireEvent.click(screen.getByText("review"));
    fireEvent.click(screen.getByRole("button", { name: "Confirm" }));

    await waitFor(() => {
      expect(onConfirm).toHaveBeenCalledWith(["review"]);
    });
  });

  it("supports select-all and clear-selection", async () => {
    const onConfirm = vi.fn().mockResolvedValue(undefined);

    render(
      <ImportBuiltinModal
        loading={false}
        open
        sources={sources}
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
      expect(onConfirm).toHaveBeenCalledWith(["review", "writer"]);
    });
  });

  it("does not close while import is loading", () => {
    const onClose = vi.fn();

    render(
      <ImportBuiltinModal
        loading
        open
        sources={sources}
        onClose={onClose}
        onConfirm={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));

    expect(onClose).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "Confirm" })).toBeDisabled();
  });
});
