import { beforeEach, describe, expect, it } from "vitest";

import { clearAuthToken, getApiToken, getApiUrl, setAuthToken } from "./config";

function setViteBase(value: string): void {
  (
    globalThis as typeof globalThis & { VITE_API_BASE_URL: string }
  ).VITE_API_BASE_URL = value;
}

function setBuildToken(value: string): void {
  (globalThis as typeof globalThis & { TOKEN: string }).TOKEN = value;
}

describe("getApiUrl", () => {
  beforeEach(() => setViteBase(""));

  it("prepends the Rust /api prefix with or without a leading slash", () => {
    expect(getApiUrl("/models")).toBe("/api/models");
    expect(getApiUrl("models/active")).toBe("/api/models/active");
  });

  it("concatenates the configured API base URL and nested paths", () => {
    setViteBase("http://127.0.0.1:8765");

    expect(getApiUrl("/agents/default/chats")).toBe(
      "http://127.0.0.1:8765/api/agents/default/chats",
    );
  });
});

describe("auth token helpers", () => {
  beforeEach(() => {
    localStorage.clear();
    setBuildToken("");
  });

  it("uses the Rust rua auth token key before the build-time token", () => {
    localStorage.setItem("rua_auth_token", "stored-token");
    setBuildToken("build-token");

    expect(getApiToken()).toBe("stored-token");
  });

  it("falls back to TOKEN and returns empty string when neither token exists", () => {
    setBuildToken("build-token");
    expect(getApiToken()).toBe("build-token");

    setBuildToken("");
    expect(getApiToken()).toBe("");
  });

  it("setAuthToken and clearAuthToken mutate only the rua token key", () => {
    localStorage.setItem("qwenpaw_auth_token", "legacy-token");

    setAuthToken("rua-token");
    expect(localStorage.getItem("rua_auth_token")).toBe("rua-token");
    expect(localStorage.getItem("qwenpaw_auth_token")).toBe("legacy-token");

    clearAuthToken();
    expect(localStorage.getItem("rua_auth_token")).toBeNull();
    expect(localStorage.getItem("qwenpaw_auth_token")).toBe("legacy-token");
  });
});
