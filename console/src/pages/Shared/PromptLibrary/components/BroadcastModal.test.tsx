import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { BroadcastModal } from "./BroadcastModal";

const { listAgentsMock } = vi.hoisted(() => ({
  listAgentsMock: vi.fn(),
}));

vi.mock("../../../../api/modules/agents", () => ({
  agentsApi: {
    listAgents: listAgentsMock,
  },
}));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string) =>
      ({
        "agent.selectAll": "Select all",
        "promptLibrary.broadcast": "Broadcast",
        "promptLibrary.selectAgents": "Select agents",
        "promptLibrary.selectFiles": "Select files",
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
    onOk,
    open,
    title,
  }: {
    children?: React.ReactNode;
    okButtonProps?: { disabled?: boolean };
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
      </div>
    ) : null,
}));

const files = [
  { filename: "ROOT.md", size: 10 },
  { filename: "AGENT.md", size: 20 },
] satisfies Parameters<typeof BroadcastModal>[0]["files"];

describe("BroadcastModal", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    listAgentsMock.mockResolvedValue({
      agents: [
        { id: "default", name: "Default Agent" },
        { id: "writer", name: "Writer" },
      ],
    });
  });

  it("loads Rust agents and broadcasts selected prompt files to selected agents", async () => {
    const onConfirm = vi.fn().mockResolvedValue(undefined);

    render(
      <BroadcastModal
        files={files}
        initialFiles={["ROOT.md"]}
        open
        onClose={vi.fn()}
        onConfirm={onConfirm}
      />,
    );

    expect(await screen.findByText("Default Agent")).toBeVisible();
    expect(screen.getByRole("button", { name: "Confirm" })).toBeDisabled();

    fireEvent.click(screen.getByText("Writer"));
    fireEvent.click(screen.getByRole("button", { name: "Confirm" }));

    await waitFor(() => {
      expect(listAgentsMock).toHaveBeenCalledTimes(1);
      expect(onConfirm).toHaveBeenCalledWith(["ROOT.md"], ["writer"]);
    });
  });

  it("supports select-all and clear-selection across file and agent pickers", async () => {
    const onConfirm = vi.fn().mockResolvedValue(undefined);

    render(
      <BroadcastModal
        files={files}
        initialFiles={[]}
        open
        onClose={vi.fn()}
        onConfirm={onConfirm}
      />,
    );

    await screen.findByText("Writer");
    const [selectAllFiles, selectAllAgents] = screen.getAllByRole("button", {
      name: "Select all",
    });
    const [clearFiles] = screen.getAllByRole("button", {
      name: "Clear selection",
    });

    fireEvent.click(selectAllFiles);
    fireEvent.click(clearFiles);
    expect(screen.getByRole("button", { name: "Confirm" })).toBeDisabled();

    fireEvent.click(selectAllFiles);
    fireEvent.click(selectAllAgents);
    fireEvent.click(screen.getByRole("button", { name: "Confirm" }));

    await waitFor(() => {
      expect(onConfirm).toHaveBeenCalledWith(
        ["ROOT.md", "AGENT.md"],
        ["default", "writer"],
      );
    });
  });
});
