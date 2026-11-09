import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import MainLayout from "./index";
vi.mock("../useNavigationActivity", () => ({ useNavigationActivity: vi.fn() }));

const { getDeviceStatusMock, getStatusMock, isMobileMock } = vi.hoisted(() => ({
  getDeviceStatusMock: vi.fn(),
  getStatusMock: vi.fn(),
  isMobileMock: vi.fn(),
}));

vi.mock("../../utils/isMobile", () => ({
  isMobile: isMobileMock,
}));

vi.mock("../../api/modules/sync", () => ({
  devicesApi: {
    getDeviceStatus: getDeviceStatusMock,
  },
  syncApi: {
    getStatus: getStatusMock,
  },
}));

vi.mock("../Sidebar", () => ({
  default: ({
    onNavigate,
    open,
  }: {
    onNavigate: (path: string) => void;
    open: boolean;
  }) => (
    <aside aria-label="navigation" hidden={!open}>
      <button type="button" onClick={() => onNavigate("/models")}>
        Go to models
      </button>
    </aside>
  ),
}));

vi.mock("../MobileLayout", () => ({
  default: () => <main>MobileLayout</main>,
}));

vi.mock("lucide-react", () => ({
  Menu: () => <span data-testid="menu-icon" />,
}));

vi.mock("../pageTransition", () => ({
  navigateWithPageTransition: (
    navigate: (path: string) => void,
    path: string,
  ) => navigate(path),
}));

vi.mock("../../pages/Chat/preparedChatTransition", () => ({
  cancelPreparedChatTransition: vi.fn(),
}));

vi.mock("../../components/Skeleton", () => ({
  default: ({ count, variant }: { count?: number; variant?: string }) => (
    <div>
      Skeleton:{variant}:{count}
    </div>
  ),
}));

vi.mock("../../pages/Chat", () => ({
  default: () => <div>ChatPage</div>,
}));

vi.mock("../../pages/Settings/Models", () => ({
  default: () => <div>ModelsPage</div>,
}));

vi.mock("../../pages/Settings/Agents", () => ({
  default: () => <div>AgentsPage</div>,
}));

vi.mock("../../pages/Terminal", () => ({
  default: () => <div>TerminalPage</div>,
}));

vi.mock("../../pages/DeviceRevoked", () => ({
  default: () => <div>DeviceRevokedPage</div>,
}));

vi.mock("../../pages/Onboarding/JoinDevices", () => ({
  default: () => <div>JoinDevicesPage</div>,
}));

function renderLayout(path: string) {
  return render(
    <MemoryRouter initialEntries={[path]}>
      <Routes>
        <Route path="/" element={<MainLayout />}>
          <Route path="models" element={<div>ModelsPage</div>} />
          <Route path="terminal" element={<div>TerminalPage</div>} />
          <Route path="device-revoked" element={<div>DeviceRevokedPage</div>} />
          <Route path="agent-templates" element={<div>AgentsPage</div>} />
        </Route>
      </Routes>
    </MemoryRouter>,
  );
}

describe("MainLayout", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    isMobileMock.mockReturnValue(false);
    getStatusMock.mockResolvedValue({ enabled: true });
    getDeviceStatusMock.mockResolvedValue({ is_revoked: false });
    localStorage.clear();
    document.documentElement.style.removeProperty("--header-height");
  });

  it("uses the mobile-aligned desktop shell with a persistent header menu", () => {
    localStorage.setItem("rua_sider_collapsed", "false");

    renderLayout("/models");

    expect(
      screen.getByRole("complementary", { name: "navigation" }),
    ).toBeVisible();
    expect(screen.getByRole("button", { name: "menu" })).toHaveAttribute(
      "aria-expanded",
      "true",
    );
    expect(document.querySelector(".desktop-mobile-shell")).not.toBeNull();
    expect(screen.queryByText("ConsoleCronBubble")).not.toBeInTheDocument();
    expect(
      document.documentElement.style.getPropertyValue("--header-height"),
    ).toBe("0px");
  });

  it("toggles the desktop sidebar without moving the menu into the panel", () => {
    localStorage.setItem("rua_sider_collapsed", "true");
    renderLayout("/models");

    const menuButton = screen.getByRole("button", { name: "menu" });
    expect(
      screen.queryByRole("complementary", { name: "navigation" }),
    ).not.toBeInTheDocument();

    fireEvent.click(menuButton);

    expect(
      screen.getByRole("complementary", { name: "navigation" }),
    ).toBeVisible();
    expect(menuButton).toHaveAttribute("aria-expanded", "true");
    expect(localStorage.getItem("rua_sider_collapsed")).toBe("false");

    fireEvent.click(menuButton);

    expect(
      screen.queryByRole("complementary", { name: "navigation" }),
    ).not.toBeInTheDocument();
    expect(menuButton).toHaveAttribute("aria-expanded", "false");
    expect(localStorage.getItem("rua_sider_collapsed")).toBe("true");
  });

  it("keeps the desktop sidebar open after navigation", async () => {
    localStorage.setItem("rua_sider_collapsed", "false");
    renderLayout("/terminal");

    fireEvent.click(screen.getByRole("button", { name: "Go to models" }));

    await waitFor(() => {
      expect(screen.getByText("ModelsPage")).toBeVisible();
    });
    expect(
      screen.getByRole("complementary", { name: "navigation" }),
    ).toBeVisible();
    expect(screen.getByRole("button", { name: "menu" })).toHaveAttribute(
      "aria-expanded",
      "true",
    );
  });

  it("shows conversation navigation by default and retains it while collapsed", () => {
    renderLayout("/models");
    const sidebar = screen.getByRole("complementary", { name: "navigation" });
    fireEvent.click(screen.getByRole("button", { name: "menu" }));
    expect(sidebar).toBeInTheDocument();
    expect(sidebar).not.toBeVisible();
    fireEvent.click(screen.getByRole("button", { name: "menu" }));
    expect(screen.getByRole("complementary", { name: "navigation" })).toBe(
      sidebar,
    );
  });

  it("uses the mobile layout without mounting desktop chrome", () => {
    isMobileMock.mockReturnValue(true);

    renderLayout("/models");

    expect(screen.getByText("MobileLayout")).toBeVisible();
    expect(
      screen.queryByRole("button", { name: "menu" }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("complementary", { name: "navigation" }),
    ).not.toBeInTheDocument();
  });

  it("redirects revoked devices into the Rust device revoked route", async () => {
    getDeviceStatusMock.mockResolvedValueOnce({ is_revoked: true });

    renderLayout("/models");

    await waitFor(() => {
      expect(screen.getByText("DeviceRevokedPage")).toBeVisible();
    });
  });

  it("does not poll device revocation when sync is disabled", async () => {
    getStatusMock.mockResolvedValueOnce({ enabled: false });

    renderLayout("/models");

    await waitFor(() => {
      expect(getStatusMock).toHaveBeenCalledTimes(1);
    });
    expect(getDeviceStatusMock).not.toHaveBeenCalled();
  });

  it("redirects legacy agent-template route to the unified agents page", async () => {
    renderLayout("/agent-templates");

    await waitFor(() => {
      expect(screen.getByText("AgentsPage")).toBeVisible();
    });
  });

  it("renders the terminal route with a terminal nav key", async () => {
    renderLayout("/terminal");

    await waitFor(() => {
      expect(screen.getAllByText("TerminalPage")).not.toHaveLength(0);
    });
    expect(screen.getByRole("button", { name: "menu" })).toBeVisible();
  });
});
