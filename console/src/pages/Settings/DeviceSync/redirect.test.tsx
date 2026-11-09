import { render } from "@testing-library/react";
import { describe, it, expect, vi } from "vitest";
import {
  MemoryRouter,
  Routes,
  Route,
  Navigate,
  useLocation,
} from "react-router-dom";

vi.mock("../../../hooks/useAppMessage", () => ({
  useAppMessage: () => ({ message: { success: vi.fn(), error: vi.fn() } }),
}));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string, fallback?: string | Record<string, unknown>) => {
      if (typeof fallback === "string") return fallback;
      return key;
    },
  }),
}));

vi.mock("../../../api/modules/sync", () => ({
  syncApi: {
    getStatus: vi
      .fn()
      .mockResolvedValue({ enabled: false, paired: false, peer_count: 0 }),
  },
  devicesApi: { listDevices: vi.fn().mockResolvedValue([]) },
  conflictsApi: {
    getRecent: vi.fn().mockResolvedValue({ conflicts: [], count: 0 }),
  },
  pairingApi: { initiate: vi.fn(), cancel: vi.fn(), status: vi.fn() },
}));

vi.mock("../../../api", () => ({
  default: {
    exportNetworkSecretBackup: vi.fn(),
    renderNetworkSecretBackupQr: vi.fn(),
  },
}));

function LocationDisplay() {
  const loc = useLocation();
  return <div data-testid="location">{loc.pathname}</div>;
}

function renderWithRedirect(initialPath: string) {
  return render(
    <MemoryRouter initialEntries={[initialPath]}>
      <Routes>
        <Route path="/settings/device-sync" element={<LocationDisplay />} />
        <Route
          path="/settings/devices"
          element={<Navigate to="/settings/device-sync" replace />}
        />
        <Route
          path="/settings/media"
          element={<Navigate to="/settings/device-sync" replace />}
        />
        <Route
          path="/settings/sync"
          element={<Navigate to="/settings/device-sync" replace />}
        />
        <Route
          path="/settings/sync/conflicts"
          element={<Navigate to="/settings/device-sync" replace />}
        />
        <Route
          path="/security/master-export"
          element={<Navigate to="/settings/device-sync" replace />}
        />
        <Route
          path="/devices/add"
          element={<Navigate to="/settings/device-sync" replace />}
        />
      </Routes>
    </MemoryRouter>,
  );
}

describe("Old path redirects", () => {
  const oldPaths = [
    "/settings/devices",
    "/settings/media",
    "/settings/sync",
    "/settings/sync/conflicts",
    "/security/master-export",
    "/devices/add",
  ];

  oldPaths.forEach((path) => {
    it(`${path} redirects to /settings/device-sync`, () => {
      const { getByTestId } = renderWithRedirect(path);
      expect(getByTestId("location").textContent).toBe("/settings/device-sync");
    });
  });
});
