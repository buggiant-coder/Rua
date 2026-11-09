import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { EnvVar } from "../../../api/types";
import { useEnvVars } from "./useEnvVars";

const { listEnvsMock } = vi.hoisted(() => ({
  listEnvsMock: vi.fn(),
}));

vi.mock("../../../api", () => ({
  default: {
    listEnvs: listEnvsMock,
  },
}));

const envs: EnvVar[] = [
  { key: "RUA_ENV", value: "test" },
  { key: "OPENAI_API_KEY", value: "***" },
];

describe("useEnvVars", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    listEnvsMock.mockResolvedValue(envs);
  });

  it("loads Rust device-local env variables from the env API", async () => {
    const { result } = renderHook(() => useEnvVars());

    await waitFor(() => expect(result.current.loading).toBe(false));

    expect(listEnvsMock).toHaveBeenCalledTimes(1);
    expect(result.current.envVars).toEqual(envs);
    expect(result.current.error).toBeNull();
  });

  it("refreshes the env list and clears a previous error", async () => {
    listEnvsMock.mockRejectedValueOnce(new Error("env store unavailable"));
    const { result } = renderHook(() => useEnvVars());
    await waitFor(() =>
      expect(result.current.error).toBe("env store unavailable"),
    );

    listEnvsMock.mockResolvedValueOnce([{ key: "NEW_ENV", value: "1" }]);

    await act(async () => {
      await result.current.fetchAll();
    });

    expect(result.current.loading).toBe(false);
    expect(result.current.error).toBeNull();
    expect(result.current.envVars).toEqual([{ key: "NEW_ENV", value: "1" }]);
  });

  it("keeps current env vars when a later refresh fails", async () => {
    const { result } = renderHook(() => useEnvVars());
    await waitFor(() => expect(result.current.loading).toBe(false));

    listEnvsMock.mockRejectedValueOnce(new Error("network down"));

    await act(async () => {
      await result.current.fetchAll();
    });

    expect(result.current.envVars).toEqual(envs);
    expect(result.current.error).toBe("network down");
    expect(result.current.loading).toBe(false);
  });
});
