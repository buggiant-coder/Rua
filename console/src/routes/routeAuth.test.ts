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
import { __resetRouteAccessForTests, verifyRouteAccess } from "./routeAuth";

describe("route auth availability", () => {
  afterEach(() => {
    __resetRouteAccessForTests();
    vi.clearAllMocks();
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
