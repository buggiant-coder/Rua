import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import Sidebar from "./Sidebar";

const onNavigateMock = vi.fn();

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string) =>
      ({
        "group.main": "Main",
        "nav.agents": "Agents",
        "nav.chat": "Chat",
        "nav.skills": "Skills",
        "nav.terminal": "Terminal",
      })[key] ?? key,
  }),
}));

vi.mock("../components/LanguageSwitcher", () => ({
  default: () => <span>LanguageSwitcher</span>,
}));

vi.mock("../components/ThemeToggleButton", () => ({
  default: () => <span>ThemeToggleButton</span>,
}));

vi.mock("../components/SyncNowButton", () => ({
  default: () => <span>SyncNowButton</span>,
}));

vi.mock("./navGroups", () => ({
  NAV_GROUPS: [
    {
      key: "primary",
      labelKey: null,
      items: [
        {
          icon: <span>chat-icon</span>,
          key: "chat",
          labelKey: "nav.chat",
          path: null,
        },
        {
          icon: <span>terminal-icon</span>,
          key: "terminal",
          labelKey: "nav.terminal",
          path: "/terminal",
        },
      ],
    },
    {
      key: "main",
      labelKey: "group.main",
      items: [
        {
          icon: <span>skills-icon</span>,
          key: "skills",
          labelKey: "nav.skills",
          path: "/skills",
        },
        {
          icon: <span>agents-icon</span>,
          key: "agents",
          labelKey: "nav.agents",
          path: "/agents",
        },
      ],
    },
  ],
}));

describe("Sidebar", () => {
  beforeEach(() => {
    onNavigateMock.mockReset();
  });

  it("renders the same navigation panel content as mobile mode", () => {
    render(
      <Sidebar
        location={{ pathname: "/skills", search: "" }}
        onNavigate={onNavigateMock}
      />,
    );

    expect(
      screen.getByRole("complementary", { name: "navigation" }),
    ).toBeVisible();
    expect(screen.getByText("Rua")).toBeVisible();
    expect(screen.getByText("SyncNowButton")).toBeVisible();
    expect(screen.getByText("LanguageSwitcher")).toBeVisible();
    expect(screen.getByText("ThemeToggleButton")).toBeVisible();
    expect(
      screen.queryByRole("button", { name: "menu" }),
    ).not.toBeInTheDocument();
    expect(screen.queryByText(/AgentSelector/)).not.toBeInTheDocument();
  });

  it("keeps navigation state outside the panel and reports selections", () => {
    localStorage.setItem("rua_last_chat_id_default", "chat-1");
    render(
      <Sidebar
        location={{ pathname: "/skills", search: "" }}
        onNavigate={onNavigateMock}
      />,
    );

    const skillsButton = screen.getByRole("button", { name: /Skills/ });
    expect(skillsButton.className).toContain("navItemActive");

    fireEvent.click(screen.getByRole("button", { name: /Chat/ }));

    expect(onNavigateMock).toHaveBeenCalledWith("/agents/default/chats/chat-1");
    expect(
      screen.getByRole("complementary", { name: "navigation" }),
    ).toBeVisible();
  });

  it("does not highlight agent management on an agent chat route", () => {
    render(
      <Sidebar
        location={{
          pathname: "/agents/default/chats/chat-1",
          search: "",
        }}
        onNavigate={onNavigateMock}
      />,
    );

    expect(screen.getByRole("button", { name: /Chat/ }).className).toContain(
      "navItemActive",
    );
    expect(
      screen.getByRole("button", { name: /Agents/ }).className,
    ).not.toContain("navItemActive");
  });
});
