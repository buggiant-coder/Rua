import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("./config", () => ({
  getApiUrl: (path: string) => `/api${path}`,
  clearAuthToken: vi.fn(),
}));
vi.mock("./authHeaders", () => ({
  buildAuthHeaders: vi.fn(() => ({})),
}));

import { buildAuthHeaders } from "./authHeaders";
import { clearAuthToken } from "./config";
import { request } from "./request";

function response(
  status: number,
  body: unknown = "",
  contentType = "application/json",
): Response {
  const text = typeof body === "string" ? body : JSON.stringify(body);
  return new Response(status === 204 || status === 304 ? null : text, {
    status,
    statusText:
      status === 200 ? "OK" : status === 401 ? "Unauthorized" : "Error",
    headers: contentType ? { "Content-Type": contentType } : undefined,
  });
}

describe("request success path", () => {
  beforeEach(() => {
    vi.mocked(buildAuthHeaders).mockReturnValue({});
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    vi.clearAllMocks();
  });

  it("builds Rust API URLs, disables cache, and omits Content-Type for GET", async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(response(200, { ok: true }));
    vi.stubGlobal("fetch", fetchMock);

    await expect(request("/models")).resolves.toEqual({ ok: true });

    expect(fetchMock).toHaveBeenCalledWith(
      "/api/models",
      expect.objectContaining({ cache: "no-store" }),
    );
    const headers = fetchMock.mock.calls[0][1].headers as Headers;
    expect(headers.has("Content-Type")).toBe(false);
  });

  it("adds JSON Content-Type for mutating requests without overriding caller headers", async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(response(200, { ok: true }))
      .mockResolvedValueOnce(response(200, { ok: true }));
    vi.stubGlobal("fetch", fetchMock);

    await request("/models/active", { method: "PUT", body: "{}" });
    await request("/console/upload", {
      method: "POST",
      headers: { "Content-Type": "multipart/form-data" },
    });

    expect(
      (fetchMock.mock.calls[0][1].headers as Headers).get("Content-Type"),
    ).toBe("application/json");
    expect(
      (fetchMock.mock.calls[1][1].headers as Headers).get("Content-Type"),
    ).toBe("multipart/form-data");
  });

  it("injects auth headers without overriding explicit caller headers", async () => {
    vi.mocked(buildAuthHeaders).mockReturnValue({
      Authorization: "Bearer auth-token",
      "X-Agent-Id": "selected",
    });
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(response(200, { ok: true }));
    vi.stubGlobal("fetch", fetchMock);

    await request("/agents/default/chats", {
      headers: { Authorization: "Bearer explicit" },
    });

    const headers = fetchMock.mock.calls[0][1].headers as Headers;
    expect(headers.get("Authorization")).toBe("Bearer explicit");
    expect(headers.get("X-Agent-Id")).toBe("selected");
  });

  it("returns undefined for 204 and text for non-JSON responses", async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(response(204, "", ""))
      .mockResolvedValueOnce(response(200, "ready", "text/plain"));
    vi.stubGlobal("fetch", fetchMock);

    await expect(
      request("/agents/default/chats/chat-1"),
    ).resolves.toBeUndefined();
    await expect(request("/healthz")).resolves.toBe("ready");
  });
});

describe("request error path", () => {
  beforeEach(() => {
    vi.mocked(buildAuthHeaders).mockReturnValue({});
    window.history.replaceState(null, "", "/login");
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    vi.clearAllMocks();
  });

  it("clears the Rust auth token and throws a stable error on 401", async () => {
    vi.stubGlobal("fetch", vi.fn().mockResolvedValueOnce(response(401)));

    await expect(request("/models")).rejects.toThrow("Not authenticated");
    expect(clearAuthToken).toHaveBeenCalledOnce();
  });

  it("surfaces FastAPI detail strings while preserving the raw body", async () => {
    vi.stubGlobal(
      "fetch",
      vi
        .fn()
        .mockResolvedValueOnce(response(400, { detail: "invalid provider" })),
    );

    await expect(request("/models/missing/config")).rejects.toThrow(
      'invalid provider - {"detail":"invalid provider"}',
    );
  });

  it("flattens FastAPI validation detail arrays", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValueOnce(
        response(422, {
          detail: [
            { loc: ["body", "name"], msg: "Field required" },
            { loc: ["body", "config", "enabled"], msg: "Expected boolean" },
          ],
        }),
      ),
    );

    await expect(request("/skills")).rejects.toThrow(
      'name: Field required; config.enabled: Expected boolean - {"detail":[{"loc":["body","name"],"msg":"Field required"},{"loc":["body","config","enabled"],"msg":"Expected boolean"}]}',
    );
  });

  it("falls back to message, error, text, or status messages", async () => {
    vi.stubGlobal(
      "fetch",
      vi
        .fn()
        .mockResolvedValueOnce(response(500, { message: "server message" }))
        .mockResolvedValueOnce(response(500, { error: "server error" }))
        .mockResolvedValueOnce(response(500, "plain failure", "text/plain"))
        .mockResolvedValueOnce(response(500, "")),
    );

    await expect(request("/a")).rejects.toThrow(
      'server message - {"message":"server message"}',
    );
    await expect(request("/b")).rejects.toThrow(
      'server error - {"error":"server error"}',
    );
    await expect(request("/c")).rejects.toThrow(
      "plain failure - plain failure",
    );
    await expect(request("/d")).rejects.toThrow("Request failed: 500 Error");
  });
});
