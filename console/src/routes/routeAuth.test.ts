import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("@/api/modules/auth", () => ({
  authApi: {
    getStatus: vi.fn(),
  },
}));

vi.mock("@/api/config", () => ({
  clearAuthToken: vi.fn(),
  getApiToken: vi.fn(() => null),
  getApiUrl: (path: string) => `/api${path}`,
}));

import { authApi } from "@/api/modules/auth";
import {
  __resetRouteAccessForTests,
  requireRouteAccess,
  verifyRouteAccess,
} from "./routeAuth";

describe("route auth availability", () => {
  afterEach(() => {
    __resetRouteAccessForTests();
    vi.clearAllMocks();
  });

  it("keeps the return Chat, turn and fragment across login under the Console basename", async () => {
    vi.mocked(authApi.getStatus).mockResolvedValue({
      enabled: true,
      has_users: true,
    });
    const target = "/console/chats/chat-a?turn=turn-1&source=notice";
    history.replaceState({}, "", target + "#result");
    const error = await requireRouteAccess(
      new Request(location.origin + target),
    ).catch((reason) => reason);
    const login = new URL(error.headers.get("Location"), location.origin);
    expect(login.searchParams.get("redirect")).toBe(
      "/chats/chat-a?turn=turn-1&source=notice#result",
    );
    history.replaceState({}, "", "/");
  });

  it("does not cache a transient auth status failure as auth-required", async () => {
    vi.mocked(authApi.getStatus)
      .mockRejectedValueOnce(new Error("backend unavailable"))
      .mockResolvedValueOnce({ enabled: false, has_users: false });

    await expect(verifyRouteAccess()).rejects.toThrow("backend unavailable");
    await expect(verifyRouteAccess()).resolves.toBe("ok");
    expect(authApi.getStatus).toHaveBeenCalledTimes(2);
  });
});
