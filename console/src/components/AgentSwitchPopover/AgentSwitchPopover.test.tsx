import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import AgentSwitchPopover from "./index";

const {
  ensureCatalogLoadedMock,
  setSelectedAgentMock,
  storeState,
  toastErrorMock,
  toastSuccessMock,
} = vi.hoisted(() => ({
  ensureCatalogLoadedMock: vi.fn(),
  setSelectedAgentMock: vi.fn(),
  storeState: {
    agents: [] as Array<{
      description?: string;
      enabled: boolean;
      id: string;
      name?: string;
    }>,
    selectedAgent: "default",
  },
  toastErrorMock: vi.fn(),
  toastSuccessMock: vi.fn(),
}));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string) =>
      ({
        "agent.loadFailed": "Failed to load agents",
        "agent.switchAgent": "Switch agent",
        "agent.currentWorkspace": "Current Agent",
        "agent.switchSuccess": "Switched agent",
        "common.disabled": "Disabled",
      })[key] ?? key,
  }),
}));

vi.mock("lucide-react", () => ({
  Check: () => <span data-testid="active-agent" />,
}));

vi.mock("../../stores/agentStore", () => ({
  useAgentStore: () => ({
    ...storeState,
    ensureCatalogLoaded: ensureCatalogLoadedMock,
    setSelectedAgent: setSelectedAgentMock,
  }),
}));

vi.mock("@/ui/Popover", () => ({
  default: ({
    children,
    content,
    placement,
    onOpenChange,
  }: {
    children?: ReactNode;
    content?: ReactNode;
    placement?: string;
    onOpenChange?: (open: boolean) => void;
  }) => (
    <div data-placement={placement}>
      <div onClick={() => onOpenChange?.(true)}>{children}</div>
      <div>{content}</div>
    </div>
  ),
}));

vi.mock("@/ui/Toast", () => ({
  default: {
    error: toastErrorMock,
    success: toastSuccessMock,
    warning: vi.fn(),
  },
}));

const unorderedAgents = [
  { id: "disabled", name: "Disabled", enabled: false },
  { id: "default", name: "Default Agent", enabled: true },
  { id: "coder", name: "Coder", enabled: true },
];

describe("AgentSwitchPopover", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    storeState.selectedAgent = "default";
    storeState.agents = unorderedAgents;
    ensureCatalogLoadedMock.mockResolvedValue(unorderedAgents);
  });

  it("ensures the shared catalog is loaded and displays enabled agents first", async () => {
    render(
      <AgentSwitchPopover>
        <button type="button">trigger</button>
      </AgentSwitchPopover>,
    );
    fireEvent.click(screen.getByText("trigger"));

    await waitFor(() =>
      expect(ensureCatalogLoadedMock).toHaveBeenCalledTimes(1),
    );
    const options = screen.getAllByRole("button").slice(1);
    expect(options.map((node) => node.textContent)).toEqual([
      "DDefault AgentCurrent Agent",
      "CCoderSwitch agent",
      "DDisabledDisabled",
    ]);
    expect(screen.getAllByText("Switch agent")).toHaveLength(2);
    expect(screen.getByText("Current Agent")).toBeVisible();
    expect(screen.getAllByText("Disabled")).toHaveLength(2);
  });

  it("marks the selected agent and switches only to enabled agents", () => {
    render(
      <AgentSwitchPopover placement="top-end">
        <button type="button">trigger</button>
      </AgentSwitchPopover>,
    );

    expect(screen.getByTestId("active-agent")).toBeVisible();
    const disabledOption = screen.getByRole("button", {
      name: "DisabledDisabled",
    });
    fireEvent.click(disabledOption);
    expect(setSelectedAgentMock).not.toHaveBeenCalledWith("disabled");
    expect(disabledOption).toBeDisabled();

    fireEvent.click(screen.getByText("Coder"));
    expect(setSelectedAgentMock).toHaveBeenCalledWith("coder");
    expect(toastSuccessMock).toHaveBeenCalledWith("Switched agent");
  });

  it("reports agent loading failures without switching state", async () => {
    const errorSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    ensureCatalogLoadedMock.mockRejectedValueOnce(new Error("network down"));

    render(
      <AgentSwitchPopover>
        <button type="button">trigger</button>
      </AgentSwitchPopover>,
    );
    fireEvent.click(screen.getByText("trigger"));

    await waitFor(() =>
      expect(toastErrorMock).toHaveBeenCalledWith("Failed to load agents"),
    );
    expect(setSelectedAgentMock).not.toHaveBeenCalled();
    errorSpy.mockRestore();
  });
});
