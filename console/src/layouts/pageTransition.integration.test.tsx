import { lazy } from "react";
import { fireEvent, render, screen } from "@testing-library/react";
import { Route, Routes, useNavigate } from "react-router-dom";
import { afterEach, describe, expect, it, vi } from "vitest";
import AppRouter, { RouteCommitBoundary } from "./AppRouter";
import { navigateWithPageTransition } from "./pageTransition";

function ChatPage() {
  const navigate = useNavigate();
  return (
    <div>
      <span>Chat page</span>
      <button
        type="button"
        onClick={() => navigateWithPageTransition(navigate, "/terminal")}
      >
        Open terminal
      </button>
    </div>
  );
}

describe("page transition route commit", () => {
  afterEach(() => {
    window.history.replaceState(null, "", "/");
    delete (document as Document & { startViewTransition?: unknown })
      .startViewTransition;
    vi.restoreAllMocks();
  });

  it("keeps the current route visible until a lazy destination is ready", () => {
    window.history.replaceState(null, "", "/agents/default/chats/chat-1");
    const transition = {
      finished: Promise.resolve(),
      skipTransition: vi.fn(),
    } as unknown as ViewTransition;
    const startViewTransition = vi.fn((callback: () => void) => {
      callback();
      return transition;
    });
    Object.defineProperty(document, "startViewTransition", {
      configurable: true,
      value: startViewTransition,
    });
    vi.spyOn(window, "matchMedia").mockReturnValue({
      matches: false,
    } as MediaQueryList);
    const TerminalPage = lazy(() => new Promise<never>(() => undefined));

    render(
      <AppRouter>
        <RouteCommitBoundary fallback={<div>Loading terminal</div>}>
          <Routes>
            <Route
              path="/agents/:agentId/chats/:chatId"
              element={<ChatPage />}
            />
            <Route path="/terminal" element={<TerminalPage />} />
          </Routes>
        </RouteCommitBoundary>
      </AppRouter>,
    );

    fireEvent.click(screen.getByRole("button", { name: "Open terminal" }));

    expect(window.location.pathname).toBe("/terminal");
    expect(screen.getByText("Chat page")).toBeVisible();
    expect(screen.queryByText("Loading terminal")).not.toBeInTheDocument();
    expect(startViewTransition).not.toHaveBeenCalled();
  });
});
