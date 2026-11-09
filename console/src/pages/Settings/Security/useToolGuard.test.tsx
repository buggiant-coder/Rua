import { renderHook, act, waitFor } from "@testing-library/react";
import { vi, describe, it, expect, beforeEach } from "vitest";

const mockApi = vi.hoisted(() => ({
  getAgentToolGuard: vi.fn(),
  getAgentBuiltinRules: vi.fn(),
  resetAgentToolGuardToBuiltin: vi.fn(),
}));

vi.mock("../../../api", () => ({ default: mockApi }));

import { useToolGuard } from "./useToolGuard";

const fakeConfig = {
  enabled: false,
  custom_rules: [],
  disabled_rules: ["r1"],
};

const fakeBuiltinRules = [
  {
    id: "r1",
    tools: ["shell"],
    params: [],
    category: "exec",
    severity: "high",
    patterns: ["rm -rf"],
    exclude_patterns: [],
    description: "block rm",
    remediation: "dont",
  },
];

describe("useToolGuard", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockApi.getAgentToolGuard.mockResolvedValue(fakeConfig);
    mockApi.getAgentBuiltinRules.mockResolvedValue(fakeBuiltinRules);
    mockApi.resetAgentToolGuardToBuiltin.mockResolvedValue(fakeConfig);
  });

  it("agentId=null -> no API calls, loading=false", () => {
    const { result } = renderHook(() => useToolGuard(null));

    expect(mockApi.getAgentToolGuard).not.toHaveBeenCalled();
    expect(mockApi.getAgentBuiltinRules).not.toHaveBeenCalled();
    expect(result.current.loading).toBe(false);
  });

  it('agentId="A" -> calls getAgentToolGuard("A") + getAgentBuiltinRules("A")', async () => {
    const { result } = renderHook(() => useToolGuard("A"));

    await waitFor(() => expect(result.current.loading).toBe(false));

    expect(mockApi.getAgentToolGuard).toHaveBeenCalledWith("A");
    expect(mockApi.getAgentBuiltinRules).toHaveBeenCalledWith("A");
    expect(result.current.builtinRules).toEqual(fakeBuiltinRules);
    expect(result.current.enabled).toBe(false);
    expect(result.current.mergedRules).toEqual([
      expect.objectContaining({
        id: "r1",
        source: "builtin",
        disabled: true,
      }),
    ]);
  });

  it("agentId change A->B triggers refetch", async () => {
    const { result, rerender } = renderHook(({ id }) => useToolGuard(id), {
      initialProps: { id: "A" as string | null },
    });

    await waitFor(() => expect(result.current.loading).toBe(false));
    expect(mockApi.getAgentToolGuard).toHaveBeenCalledWith("A");

    rerender({ id: "B" });

    await waitFor(() => expect(result.current.loading).toBe(false));
    expect(mockApi.getAgentToolGuard).toHaveBeenCalledWith("B");
    expect(mockApi.getAgentToolGuard).toHaveBeenCalledTimes(2);
  });

  it("resetToBuiltin calls endpoint then refetches", async () => {
    const { result } = renderHook(() => useToolGuard("A"));

    await waitFor(() => expect(result.current.loading).toBe(false));

    await act(async () => {
      await result.current.resetToBuiltin();
    });

    expect(mockApi.resetAgentToolGuardToBuiltin).toHaveBeenCalledWith("A");
    expect(mockApi.getAgentToolGuard).toHaveBeenCalledTimes(2);
  });

  it("builds the Rust tool-guard save body from canonical tools, disabled rules, and custom rules", async () => {
    const { result } = renderHook(() => useToolGuard("A"));
    await waitFor(() => expect(result.current.loading).toBe(false));

    const customRule = {
      id: "custom-danger",
      tools: ["Bash"],
      params: ["command"],
      category: "exec",
      severity: "critical",
      patterns: ["rm -rf"],
      exclude_patterns: [],
      description: "dangerous shell",
      remediation: "review",
    };

    act(() => {
      result.current.toggleRule("r1", true);
      result.current.addCustomRule(customRule);
      result.current.toggleRule("custom-danger", false);
    });

    expect(result.current.buildSaveBody()).toEqual({
      enabled: false,
      custom_rules: [customRule],
      disabled_rules: ["custom-danger"],
    });
    expect(result.current.mergedRules).toEqual([
      expect.objectContaining({
        id: "r1",
        source: "builtin",
        disabled: false,
      }),
      expect.objectContaining({
        id: "custom-danger",
        source: "custom",
        disabled: true,
      }),
    ]);
  });

  it("updates and deletes custom rules without leaving stale disabled-rule entries", async () => {
    const { result } = renderHook(() => useToolGuard("A"));
    await waitFor(() => expect(result.current.loading).toBe(false));

    const original = {
      id: "custom-rule",
      tools: ["Bash"],
      params: ["command"],
      category: "exec",
      severity: "high",
      patterns: ["danger"],
      exclude_patterns: [],
      description: "old",
      remediation: "old",
    };
    const updated = { ...original, description: "updated" };

    act(() => {
      result.current.addCustomRule(original);
      result.current.toggleRule("custom-rule", false);
      result.current.updateCustomRule("custom-rule", updated);
    });
    expect(result.current.customRules).toEqual([updated]);
    expect(Array.from(result.current.disabledRules)).toContain("custom-rule");

    act(() => {
      result.current.deleteCustomRule("custom-rule");
    });

    expect(result.current.customRules).toEqual([]);
    expect(Array.from(result.current.disabledRules)).not.toContain(
      "custom-rule",
    );
  });

  it("surfaces load errors while keeping the hook usable for retry", async () => {
    mockApi.getAgentToolGuard.mockRejectedValueOnce(new Error("security down"));
    const errorSpy = vi.spyOn(console, "error").mockImplementation(() => {});

    const { result } = renderHook(() => useToolGuard("A"));

    await waitFor(() => expect(result.current.loading).toBe(false));

    expect(result.current.error).toBe("security down");
    expect(result.current.config).toBeNull();
    expect(errorSpy).toHaveBeenCalled();

    await act(async () => {
      await result.current.fetchAll();
    });

    expect(result.current.error).toBeNull();
    expect(result.current.config).toEqual(fakeConfig);
    errorSpy.mockRestore();
  });
});
