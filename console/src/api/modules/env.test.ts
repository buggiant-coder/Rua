import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("../request", () => ({ request: vi.fn() }));

import { request } from "../request";
import { envApi } from "./env";

describe("envApi", () => {
  afterEach(() => vi.clearAllMocks());

  it("lists device-local environment variables from the Rust env route", async () => {
    vi.mocked(request).mockResolvedValueOnce([
      { key: "RUA_ENV", value: "test", masked: null },
    ]);

    await expect(envApi.listEnvs()).resolves.toEqual([
      { key: "RUA_ENV", value: "test", masked: null },
    ]);
    expect(request).toHaveBeenCalledWith("/envs");
  });

  it("saves the full env map with PUT instead of a per-row patch", async () => {
    vi.mocked(request).mockResolvedValueOnce([
      { key: "A", value: "1", masked: null },
      { key: "B", value: "2", masked: null },
    ]);

    await envApi.saveEnvs({ A: "1", B: "2" });

    expect(request).toHaveBeenCalledWith("/envs", {
      method: "PUT",
      body: JSON.stringify({ A: "1", B: "2" }),
    });
  });

  it("deletes an env key through the encoded Rust route", async () => {
    vi.mocked(request).mockResolvedValueOnce([]);

    await envApi.deleteEnv("A/B key");

    expect(request).toHaveBeenCalledWith("/envs/A%2FB%20key", {
      method: "DELETE",
    });
  });
});
