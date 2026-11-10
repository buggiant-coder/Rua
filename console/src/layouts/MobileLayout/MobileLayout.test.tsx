import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import {
  MemoryRouter,
  Route,
  RouterProvider,
  Routes,
  createMemoryRouter,
} from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useHistoryDismiss } from "../../hooks/useHistoryDismiss";
import MobileLayout from "./index";

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string) =>
      ({
        "group.main": "Main",
        "nav.chat": "Chat",
        "nav.models": "Models",
        "nav.skills": "Skills",
        "nav.terminal": "Terminal",
        "nav.airdropTransfer": "Airdrop Transfer",
      })[key] ?? key,
  }),
}));

vi.mock("../ConversationNavigation", () => ({
  default: ({ onNavigate }: { onNavigate: (path: string) => void }) => (
    <div>
      <button onClick={() => onNavigate("/agents/default/chats/chat-1")}>
        Chat
      </button>
    </div>
  ),
}));

vi.mock("../../hooks/useHistoryDismiss", () => ({
  useHistoryDismiss: vi.fn(),
}));

vi.mock("../../components/LanguageSwitcher", () => ({
  default: () => <span>LanguageSwitcher</span>,
}));

vi.mock("../../components/ThemeToggleButton", () => ({
  default: () => <span>ThemeToggleButton</span>,
}));

vi.mock("../../components/SyncNowButton", () => ({
  default: () => <span>SyncNowButton</span>,
}));

vi.mock("../../components/Skeleton", () => ({
  default: ({ count, variant }: { count?: number; variant?: string }) => (
    <div>
      Skeleton:{variant}:{count}
    </div>
  ),
}));

vi.mock("@/ui", () => ({
  Drawer: ({
    children,
    onClose,
    open,
  }: {
    children?: React.ReactNode;
    onClose?: () => void;
    open?: boolean;
  }) =>
    open ? (
      <aside aria-label="mobile drawer">
        <button type="button" onClick={onClose}>
          Close drawer
        </button>
        {children}
      </aside>
    ) : null,
}));

vi.mock("../navGroups", () => ({
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
          icon: <span>models-icon</span>,
          key: "models",
          labelKey: "nav.models",
          path: "/models",
        },
        {
          icon: <span>airdrop-icon</span>,
          key: "airdrop-transfer",
          labelKey: "nav.airdropTransfer",
          path: "/settings/airdrop-transfer",
        },
      ],
    },
  ],
}));

vi.mock("../../pages/Chat", () => ({
  default: () => <div>ChatPage</div>,
}));

vi.mock("../../pages/Agent/SkillsHub", () => ({
  default: () => <div>SkillsPage</div>,
}));

vi.mock("../../pages/Settings/Models", () => ({
  default: () => <div>ModelsPage</div>,
}));

vi.mock("../../pages/Terminal", () => ({
  default: () => <div>TerminalPage</div>,
}));

function renderMobileLayout(path = "/skills") {
  return render(
    <MemoryRouter initialEntries={[path]}>
      <Routes>
        <Route path="/" element={<MobileLayout />}>
          <Route
            path="agents/:agentId/chats/:chatId"
            element={<div>ChatPage</div>}
          />
          <Route path="skills" element={<div>SkillsPage</div>} />
          <Route path="models" element={<div>ModelsPage</div>} />
          <Route path="terminal" element={<div>TerminalPage</div>} />
        </Route>
      </Routes>
    </MemoryRouter>,
  );
}

describe("MobileLayout", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    sessionStorage.clear();
    document.documentElement.classList.remove("mobile-mode");
    document.documentElement.style.removeProperty("--rua-mobile-visual-height");
    Object.defineProperty(window, "visualViewport", {
      configurable: true,
      value: undefined,
    });
  });

  it("marks the document as mobile mode and renders the current route", async () => {
    renderMobileLayout("/skills");

    expect(document.documentElement).toHaveClass("mobile-mode");
    expect(screen.queryByText("ConsoleCronBubble")).not.toBeInTheDocument();
    expect(await screen.findByText("SkillsPage")).toBeVisible();
  });

  it("renders the terminal route", async () => {
    renderMobileLayout("/terminal");

    expect(await screen.findByText("TerminalPage")).toBeVisible();
  });

  it("tracks the visual viewport while the Android keyboard resizes content", () => {
    const listeners = new Map<string, Set<EventListener>>();
    const viewport = {
      height: 720,
      addEventListener: vi.fn((type: string, listener: EventListener) => {
        const current = listeners.get(type) ?? new Set<EventListener>();
        current.add(listener);
        listeners.set(type, current);
      }),
      removeEventListener: vi.fn((type: string, listener: EventListener) => {
        listeners.get(type)?.delete(listener);
      }),
    };
    Object.defineProperty(window, "visualViewport", {
      configurable: true,
      value: viewport,
    });

    const { unmount } = renderMobileLayout("/terminal");
    expect(
      document.documentElement.style.getPropertyValue(
        "--rua-mobile-visual-height",
      ),
    ).toBe("720px");

    viewport.height = 420;
    listeners
      .get("resize")
      ?.forEach((listener) => listener(new Event("resize")));
    expect(
      document.documentElement.style.getPropertyValue(
        "--rua-mobile-visual-height",
      ),
    ).toBe("420px");

    unmount();
    expect(
      document.documentElement.style.getPropertyValue(
        "--rua-mobile-visual-height",
      ),
    ).toBe("");
  });

  it("opens the drawer with Rust navigation actions and closes it explicitly", () => {
    renderMobileLayout("/agents/default/chats/chat-1");

    fireEvent.click(screen.getByRole("button", { name: "menu" }));

    expect(
      screen.getByRole("complementary", { name: "mobile drawer" }),
    ).toBeVisible();
    expect(screen.getByText("Rua")).toBeVisible();
    expect(screen.getByText("LanguageSwitcher")).toBeVisible();
    expect(screen.getByText("ThemeToggleButton")).toBeVisible();
    expect(screen.getByText("SyncNowButton")).toBeVisible();

    fireEvent.click(screen.getByRole("button", { name: "Close drawer" }));
    expect(
      screen.queryByRole("complementary", { name: "mobile drawer" }),
    ).not.toBeInTheDocument();
  });

  it("closes the drawer when the menu is tapped again", () => {
    renderMobileLayout();
    const menu = screen.getByRole("button", { name: "menu" });
    fireEvent.click(menu);
    expect(menu).toHaveAttribute("aria-expanded", "true");
    fireEvent.click(menu);
    expect(menu).toHaveAttribute("aria-expanded", "false");
    expect(
      screen.queryByRole("complementary", { name: "mobile drawer" }),
    ).not.toBeInTheDocument();
  });

  it("does not register a second history dismiss handler for the drawer", () => {
    renderMobileLayout("/agents/default/chats/chat-1");

    fireEvent.click(screen.getByRole("button", { name: "menu" }));

    expect(useHistoryDismiss).not.toHaveBeenCalled();
  });

  it("shows conversations and frequent actions before switching to the full menu", () => {
    renderMobileLayout("/skills");

    fireEvent.click(screen.getByRole("button", { name: "menu" }));

    const chatButton = screen.getByRole("button", { name: /Chat/ });
    const terminalButton = screen.getByRole("button", { name: /Terminal/ });
    expect(screen.queryByText("Main")).not.toBeVisible();

    expect(screen.getAllByRole("button", { name: /Chat/ })).toHaveLength(1);
    expect(screen.getAllByRole("button", { name: /Terminal/ })).toHaveLength(1);
    expect(
      chatButton.compareDocumentPosition(terminalButton) &
        Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "common.more" }));
    expect(screen.getByText("Main")).toBeVisible();
    expect(
      screen.getByRole("button", { name: "navigation.backToConversations" }),
    ).toBeVisible();
  });

  it("navigates from the drawer and closes it after selection", async () => {
    renderMobileLayout("/agents/default/chats/chat-1");

    fireEvent.click(screen.getByRole("button", { name: "menu" }));
    fireEvent.click(screen.getByRole("button", { name: "common.more" }));
    fireEvent.click(screen.getByRole("button", { name: /Models/ }));

    await waitFor(() => {
      expect(screen.getByText("ModelsPage")).toBeVisible();
    });
    expect(
      screen.queryByRole("complementary", { name: "mobile drawer" }),
    ).not.toBeInTheDocument();
  });

  it("keeps the drawer open until a delayed route commits", async () => {
    let finishLoader: (() => void) | undefined;
    const loader = new Promise<void>((resolve) => {
      finishLoader = resolve;
    });
    const router = createMemoryRouter(
      [
        {
          path: "/",
          element: <MobileLayout />,
          children: [
            {
              path: "agents/:agentId/chats/:chatId",
              element: <div>ChatPage</div>,
            },
            {
              path: "models",
              loader: () => loader,
              element: <div>ModelsPage</div>,
            },
          ],
        },
      ],
      { initialEntries: ["/agents/default/chats/chat-1"] },
    );

    render(<RouterProvider router={router} useTransitions={false} />);

    fireEvent.click(screen.getByRole("button", { name: "menu" }));
    fireEvent.click(screen.getByRole("button", { name: "common.more" }));
    fireEvent.click(screen.getByRole("button", { name: /Models/ }));

    expect(screen.getByText("ChatPage")).toBeVisible();
    expect(
      screen.getByRole("complementary", { name: "mobile drawer" }),
    ).toBeVisible();

    await act(async () => finishLoader?.());

    expect(await screen.findByText("ModelsPage")).toBeVisible();
    expect(
      screen.queryByRole("complementary", { name: "mobile drawer" }),
    ).not.toBeInTheDocument();
  });

  it("keeps Terminal and Airdrop Transfer at the bottom of conversation navigation", () => {
    renderMobileLayout("/agents/default/chats/chat-1");

    fireEvent.click(screen.getByRole("button", { name: "menu" }));

    expect(
      screen.getByRole("button", { name: /Airdrop Transfer/ }),
    ).toBeVisible();
    expect(screen.getByRole("button", { name: /Terminal/ })).toBeVisible();
    fireEvent.click(screen.getByRole("button", { name: "common.more" }));
    expect(
      screen.queryByRole("button", { name: /Terminal/ }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: /Airdrop Transfer/ }),
    ).not.toBeInTheDocument();
  });
});
