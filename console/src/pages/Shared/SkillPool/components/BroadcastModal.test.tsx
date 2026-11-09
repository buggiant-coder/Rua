import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { BroadcastModal } from "./BroadcastModal";

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string) =>
      ({
        "agent.selectAll": "Select all",
        "agent.selectBuiltin": "Select builtin",
        "skillPool.allWorkspaces": "All workspaces",
        "skillPool.broadcast": "Broadcast",
        "skillPool.selectWorkspaces": "Select workspaces",
        "skills.clearSelection": "Clear selection",
        "skills.selectPoolItem": "Select pool skills",
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
    okButtonProps?: { disabled?: boolean };
    onClose?: () => void;
    onOk?: () => void;
    open?: boolean;
    title?: React.ReactNode;
  }) =>
    open ? (
      <div>
        <h2>{title}</h2>
        {children}
        <button disabled={okButtonProps?.disabled} type="button" onClick={onOk}>
          Confirm
        </button>
        <button type="button" onClick={onClose}>
          Cancel
        </button>
      </div>
    ) : null,
}));

const skills = [
  {
    content: "---\nname: docs\n---",
    description: "Docs",
    name: "docs",
    source: "pool",
  },
  {
    content: "---\nname: review\n---",
    description: "Review",
    name: "review",
    source: "builtin",
  },
] satisfies Parameters<typeof BroadcastModal>[0]["skills"];

const workspaces = [
  { agent_id: "default", agent_name: "Default Agent" },
  { agent_id: "writer", agent_name: "Writer" },
] satisfies Parameters<typeof BroadcastModal>[0]["workspaces"];

describe("SkillPool BroadcastModal", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("broadcasts initial pool skill selection to selected Rust workspaces", async () => {
    const onConfirm = vi.fn().mockResolvedValue(undefined);

    render(
      <BroadcastModal
        initialSkillNames={["docs"]}
        open
        skills={skills}
        workspaces={workspaces}
        onClose={vi.fn()}
        onConfirm={onConfirm}
      />,
    );

    expect(screen.getByRole("button", { name: "Confirm" })).toBeDisabled();
    fireEvent.click(screen.getByText("Writer"));
    fireEvent.click(screen.getByRole("button", { name: "Confirm" }));

    await waitFor(() => {
      expect(onConfirm).toHaveBeenCalledWith(["docs"], ["writer"]);
    });
  });

  it("supports builtin-only skill selection and all-workspace selection", async () => {
    const onConfirm = vi.fn().mockResolvedValue(undefined);

    render(
      <BroadcastModal
        initialSkillNames={[]}
        open
        skills={skills}
        workspaces={workspaces}
        onClose={vi.fn()}
        onConfirm={onConfirm}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "Select builtin" }));
    fireEvent.click(screen.getByRole("button", { name: "All workspaces" }));
    fireEvent.click(screen.getByRole("button", { name: "Confirm" }));

    await waitFor(() => {
      expect(onConfirm).toHaveBeenCalledWith(["review"], ["default", "writer"]);
    });
  });

  it("clears selection on cancel before closing", () => {
    const onClose = vi.fn();

    render(
      <BroadcastModal
        initialSkillNames={["docs"]}
        open
        skills={skills}
        workspaces={workspaces}
        onClose={onClose}
        onConfirm={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByText("Writer"));
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));

    expect(onClose).toHaveBeenCalledTimes(1);
  });
});
