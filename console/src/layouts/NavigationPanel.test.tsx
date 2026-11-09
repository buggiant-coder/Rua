import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import NavigationPanel from "./NavigationPanel";

vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));
vi.mock("../components/LanguageSwitcher", () => ({ default: () => null }));
vi.mock("../components/SyncNowButton", () => ({ default: () => null }));
vi.mock("../components/ThemeToggleButton", () => ({ default: () => null }));
vi.mock("./ConversationNavigation", () => ({
  default: () => (
    <div>
      <label>
        Search draft
        <input aria-label="Search draft" defaultValue="" />
      </label>
    </div>
  ),
}));
vi.mock("@/stores/agentStore", () => ({
  useAgentStore: (select: (state: { selectedAgent: string }) => unknown) =>
    select({ selectedAgent: "default" }),
}));

const chatPath = "/projects/project-a/chats/chat-a";
describe("NavigationPanel", () => {
  beforeEach(() => sessionStorage.clear());

  it("replaces the sidebar with full navigation without navigating or remounting chats", () => {
    const onNavigate = vi.fn();
    render(
      <NavigationPanel
        location={{ pathname: chatPath, search: "" }}
        onNavigate={onNavigate}
      />,
    );
    const input = screen.getByRole("textbox", { name: "Search draft" });
    fireEvent.change(input, { target: { value: "keep this query" } });
    expect(screen.getByRole("button", { name: "nav.terminal" })).toBeVisible();
    const terminal = screen.getByRole("button", { name: "nav.terminal" });
    const airdrop = screen.getByRole("button", { name: "nav.airdropTransfer" });
    expect(airdrop).toBeVisible();
    expect(terminal.parentElement).toBe(airdrop.parentElement);
    expect(terminal.nextElementSibling).toBe(airdrop);
    expect(airdrop.nextElementSibling).toBe(
      screen.getByRole("button", { name: "common.more" }),
    );
    const toggle = screen.getByRole("button", { name: "common.more" });
    fireEvent.click(toggle);
    expect(
      screen.getByRole("button", { name: "navigation.backToConversations" }),
    ).toBe(toggle);
    expect(
      screen.getByRole("navigation", { name: "navigation.allFunctions" }),
    ).toBeVisible();
    expect(screen.getByRole("button", { name: "nav.models" })).toBeVisible();
    expect(
      screen.queryByRole("button", { name: "nav.terminal" }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "nav.airdropTransfer" }),
    ).not.toBeInTheDocument();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(input).not.toBeVisible();
    expect(onNavigate).not.toHaveBeenCalled();
    fireEvent.click(
      screen.getByRole("button", { name: "navigation.backToConversations" }),
    );
    expect(screen.getByRole("textbox", { name: "Search draft" })).toBe(input);
    expect(input).toHaveValue("keep this query");
    expect(onNavigate).not.toHaveBeenCalled();
  });

  it("returns from a settings page to the last scoped Chat, including after remount", () => {
    const onNavigate = vi.fn();
    const { rerender, unmount } = render(
      <NavigationPanel
        location={{ pathname: chatPath, search: "" }}
        onNavigate={onNavigate}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "common.more" }));
    fireEvent.click(screen.getByRole("button", { name: "nav.models" }));
    expect(onNavigate).toHaveBeenLastCalledWith("/models");
    rerender(
      <NavigationPanel
        location={{ pathname: "/models", search: "" }}
        onNavigate={onNavigate}
      />,
    );
    unmount();
    render(
      <NavigationPanel
        location={{ pathname: "/models", search: "" }}
        onNavigate={onNavigate}
      />,
    );
    fireEvent.click(
      screen.getByRole("button", { name: "navigation.backToConversations" }),
    );
    expect(onNavigate).toHaveBeenLastCalledWith(chatPath);
  });
});
