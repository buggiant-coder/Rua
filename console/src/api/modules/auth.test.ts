import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../config", () => ({
  getApiUrl: (path: string) => `/api${path}`,
}));

import { authApi } from "./auth";

function jsonResponse(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status,
    statusText: status >= 200 && status < 300 ? "OK" : "Bad Request",
    headers: { "Content-Type": "application/json" },
  });
}

describe("authApi login/register/status", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    vi.clearAllMocks();
  });

  it("logs in through the Rust auth route and returns the response body", async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(
        jsonResponse(200, { token: "tok", username: "u" }),
      );
    vi.stubGlobal("fetch", fetchMock);

    await expect(authApi.login("u", "p")).resolves.toEqual({
      token: "tok",
      username: "u",
    });
    expect(fetchMock).toHaveBeenCalledWith("/api/auth/login", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ username: "u", password: "p" }),
    });
  });

  it("registers through the Rust auth route", async () => {
    const fetchMock = vi.fn().mockResolvedValueOnce(
      jsonResponse(200, {
        token: "new-token",
        username: "new-user",
        message: "registered",
      }),
    );
    vi.stubGlobal("fetch", fetchMock);

    await expect(authApi.register("new-user", "secret")).resolves.toEqual({
      token: "new-token",
      username: "new-user",
      message: "registered",
    });
    expect(fetchMock).toHaveBeenCalledWith("/api/auth/register", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ username: "new-user", password: "secret" }),
    });
  });

  it("checks auth status through the public Rust status route", async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(
        jsonResponse(200, { enabled: true, has_users: false }),
      );
    vi.stubGlobal("fetch", fetchMock);

    await expect(authApi.getStatus()).resolves.toEqual({
      enabled: true,
      has_users: false,
    });
    expect(fetchMock).toHaveBeenCalledWith("/api/auth/status");
  });

  it("surfaces FastAPI-style detail errors with client fallbacks", async () => {
    vi.stubGlobal(
      "fetch",
      vi
        .fn()
        .mockResolvedValueOnce(jsonResponse(401, { detail: "bad login" }))
        .mockResolvedValueOnce(jsonResponse(409, {}))
        .mockResolvedValueOnce(jsonResponse(503, {})),
    );

    await expect(authApi.login("u", "bad")).rejects.toThrow("bad login");
    await expect(authApi.register("u", "p")).rejects.toThrow(
      "Registration failed",
    );
    await expect(authApi.getStatus()).rejects.toThrow(
      "Failed to check auth status",
    );
  });
});

describe("authApi updateProfile", () => {
  beforeEach(() => localStorage.clear());

  afterEach(() => {
    localStorage.clear();
    vi.unstubAllGlobals();
    vi.clearAllMocks();
  });

  it("uses the rua auth token key and posts profile changes", async () => {
    localStorage.setItem("rua_auth_token", "rua-token");
    const fetchMock = vi.fn().mockResolvedValueOnce(
      jsonResponse(200, {
        token: "next-token",
        username: "next",
      }),
    );
    vi.stubGlobal("fetch", fetchMock);

    await expect(
      authApi.updateProfile("old-password", "next", "new-password"),
    ).resolves.toEqual({ token: "next-token", username: "next" });
    expect(fetchMock).toHaveBeenCalledWith("/api/auth/update-profile", {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        Authorization: "Bearer rua-token",
      },
      body: JSON.stringify({
        current_password: "old-password",
        new_username: "next",
        new_password: "new-password",
      }),
    });
  });

  it("sends null optional fields and surfaces detail errors", async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(jsonResponse(400, { detail: "wrong password" }));
    vi.stubGlobal("fetch", fetchMock);

    await expect(authApi.updateProfile("wrong")).rejects.toThrow(
      "wrong password",
    );
    expect(fetchMock).toHaveBeenCalledWith(
      "/api/auth/update-profile",
      expect.objectContaining({
        body: JSON.stringify({
          current_password: "wrong",
          new_username: null,
          new_password: null,
        }),
      }),
    );
  });
});
