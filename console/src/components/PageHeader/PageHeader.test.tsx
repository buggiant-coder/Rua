import { fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

vi.mock("../AgentBadge", () => ({
  default: () => <span data-testid="agent-badge">agent</span>,
}));

import { PageHeader } from "./index";

describe("PageHeader", () => {
  it("builds breadcrumbs from parent/current and renders extra slots", () => {
    render(
      <PageHeader
        parent="Settings"
        current="Security"
        center={<span>center slot</span>}
        extra={<button>extra action</button>}
        afterBreadcrumb={<span>after</span>}
        subRow={<span>sub row</span>}
      />,
    );

    expect(screen.getByText("Settings")).toBeInTheDocument();
    expect(screen.getByText("Security")).toBeInTheDocument();
    expect(screen.getByText("/")).toBeInTheDocument();
    expect(screen.getByText("after")).toBeInTheDocument();
    expect(screen.getByText("center slot")).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "extra action" }),
    ).toBeInTheDocument();
    expect(screen.getByText("sub row")).toBeInTheDocument();
    expect(screen.queryByTestId("agent-badge")).not.toBeInTheDocument();
  });

  it("renders AgentBadge only for agent-scoped pages", () => {
    render(<PageHeader current="Workspace" agentScoped />);

    expect(screen.getByText("Workspace")).toBeInTheDocument();
    expect(screen.getByTestId("agent-badge")).toBeInTheDocument();
  });

  it("honors explicit breadcrumb items over parent/current props", () => {
    render(
      <PageHeader
        parent="Ignored"
        current="Also ignored"
        items={[{ title: "Root" }, { title: "Leaf" }]}
      />,
    );

    expect(screen.getByText("Root")).toBeInTheDocument();
    expect(screen.getByText("Leaf")).toBeInTheDocument();
    expect(screen.queryByText("Ignored")).not.toBeInTheDocument();
  });

  it("orders more menu items around refresh", () => {
    render(
      <PageHeader
        current="Channels"
        moreMenuItems={[{ key: "disable", label: "Disable all" }]}
        moreOptionsMenuItems={[{ key: "options", label: "Page options" }]}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "More" }));

    const menu = screen.getByRole("menu");
    const labels = within(menu)
      .getAllByRole("menuitem")
      .map((item) => item.textContent);

    expect(labels).toEqual(["Disable all", "Refresh page", "Page options"]);
  });
});
