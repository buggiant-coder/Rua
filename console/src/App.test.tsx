import { fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import App from "./App";
import { toast } from "./ui";

vi.mock("react-router-dom", () => ({
  createBrowserRouter: vi.fn(() => ({
    state: { initialized: true },
    subscribe: () => () => {},
  })),
  RouterProvider: () => (
    <button onClick={() => toast.error("Project Chat not found")}>
      Delete missing Chat
    </button>
  ),
  Navigate: () => null,
  Outlet: () => null,
}));
vi.mock("./layouts/MainLayout", () => ({ default: () => null }));
vi.mock("./pages/Login", () => ({ default: () => null }));
vi.mock("./routes/appRoutes", () => ({ appRoutes: [] }));
vi.mock("./routes/routeAuth", () => ({ verifyRouteAccess: vi.fn() }));
vi.mock("./hooks/useAppLifecycle", () => ({ useAppLifecycle: vi.fn() }));
vi.mock("./hooks/useSyncConflictToast", () => ({
  useSyncConflictToast: vi.fn(),
}));
vi.mock("./hooks/useUserNotifications", () => ({
  useUserNotifications: vi.fn(),
}));
vi.mock("./desktop/runtime", () => ({
  syncDesktopLocale: vi.fn().mockResolvedValue(undefined),
}));
vi.mock("./api/modules/language", () => ({
  languageApi: { getLanguage: vi.fn().mockResolvedValue({ language: "en" }) },
}));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    i18n: {
      language: "en",
      on: vi.fn(),
      off: vi.fn(),
      changeLanguage: vi.fn(),
    },
  }),
}));

describe("App feedback providers", () => {
  afterEach(() => vi.restoreAllMocks());

  it("shows route errors using the providers mounted by the real App", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(
      new Response(JSON.stringify({ initialized: true }), {
        headers: { "Content-Type": "application/json" },
      }),
    );
    render(<App />);
    fireEvent.click(
      await screen.findByRole("button", { name: "Delete missing Chat" }),
    );
    expect(await screen.findByText("Project Chat not found")).toBeVisible();
  });
});
