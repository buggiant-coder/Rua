import { render, screen } from "@testing-library/react";
import type { ReactNode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import AgentBadge from "./index";

const { storeState } = vi.hoisted(() => ({
  storeState: {
    agents: [] as Array<{
      description?: string;
      enabled?: boolean;
      id: string;
      name?: string;
    }>,
    selectedAgent: "default",
  },
}));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string) =>
      ({
        "agent.switchAgent": "Switch agent",
      })[key] ?? key,
  }),
}));

vi.mock("lucide-react", () => ({
  Bot: () => <span data-testid="bot-icon" />,
  ChevronDown: () => <span data-testid="chevron-icon" />,
}));

vi.mock("../../stores/agentStore", () => ({
  useAgentStore: () => storeState,
}));

vi.mock("../AgentSwitchPopover", () => ({
  default: ({
    children,
    placement,
  }: {
    children?: ReactNode;
    placement?: string;
  }) => <div data-placement={placement}>{children}</div>,
}));

describe("AgentBadge", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    storeState.selectedAgent = "default";
    storeState.agents = [
      {
        id: "default",
        name: "Default Agent",
        description: "primary",
        enabled: true,
      },
      { id: "coder", name: "Coder", enabled: true },
    ];
  });

  it("renders the selected Rust agent display name", () => {
    render(<AgentBadge />);

    expect(screen.getByRole("button", { name: "Switch agent" })).toBeVisible();
    expect(screen.getByText("Default Agent")).toBeVisible();
    expect(screen.getByTestId("bot-icon")).toBeVisible();
    expect(screen.getByTestId("chevron-icon")).toBeVisible();
  });

  it("falls back to selected agent id when the agent list has not loaded yet", () => {
    storeState.selectedAgent = "research";
    storeState.agents = [];

    render(<AgentBadge />);

    expect(screen.getByText("research")).toBeVisible();
  });

  it("uses the compact bottom-end switch popover placement", () => {
    const { container } = render(<AgentBadge />);

    expect(container.querySelector("[data-placement]")).toHaveAttribute(
      "data-placement",
      "bottom-end",
    );
  });
});
