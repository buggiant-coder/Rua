import { render, screen } from "@testing-library/react";
import type { ReactNode } from "react";
import { describe, expect, it, vi } from "vitest";

import ProjectAgentSelector from "./ProjectAgentSelector";

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string) =>
      ({
        "projects.chat.agentFallback": "Agent",
        "projects.chat.unavailableAgent": "Unavailable Agent",
      })[key] ?? key,
  }),
}));

vi.mock("@/ui", () => ({
  Modal: {
    confirm: vi.fn(),
  },
  Popover: ({ children }: { children: ReactNode }) => <>{children}</>,
}));

vi.mock("lucide-react", () => ({
  Bot: () => null,
  Check: () => null,
  ChevronDown: () => null,
  EyeOff: () => null,
  LoaderCircle: () => null,
}));

const baseProps = {
  selectedAgentId: "agent-private-id",
  switching: false,
  switchable: true,
  confirmSelection: false,
  onSelect: vi.fn().mockResolvedValue(undefined),
};

describe("ProjectAgentSelector", () => {
  it("does not expose the selected Agent id while the catalog is loading", () => {
    render(<ProjectAgentSelector {...baseProps} agents={[]} loading={true} />);

    expect(screen.queryByText("agent-private-id")).not.toBeInTheDocument();
    expect(screen.getByText("Agent")).toBeInTheDocument();
  });

  it("shows the Agent name as soon as the catalog is available", () => {
    render(
      <ProjectAgentSelector
        {...baseProps}
        agents={[
          {
            id: "agent-private-id",
            name: "Research Agent",
            enabled: true,
          },
        ]}
        loading={false}
      />,
    );

    expect(screen.getByText("Research Agent")).toBeInTheDocument();
    expect(screen.queryByText("agent-private-id")).not.toBeInTheDocument();
  });

  it("shows an unavailable state after loading instead of using the id as a name", () => {
    render(<ProjectAgentSelector {...baseProps} agents={[]} loading={false} />);

    expect(screen.getAllByText("Unavailable Agent").length).toBeGreaterThan(0);
    expect(screen.queryByText("agent-private-id")).not.toBeInTheDocument();
  });
});
