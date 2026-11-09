import { renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useChannels } from "./useChannels";

const { listChannelsMock, listChannelTypesMock, listChannelSchemasMock } =
  vi.hoisted(() => ({
    listChannelsMock: vi.fn(),
    listChannelTypesMock: vi.fn(),
    listChannelSchemasMock: vi.fn(),
  }));

vi.mock("../../../api", () => ({
  default: {
    listChannels: listChannelsMock,
    listChannelTypes: listChannelTypesMock,
    listChannelSchemas: listChannelSchemasMock,
  },
}));

vi.mock("../../../stores/agentStore", () => ({
  useAgentStore: () => ({ selectedAgent: "default" }),
}));

describe("useChannels", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    listChannelSchemasMock.mockResolvedValue({
      order: ["console", "wechat", "mattermost", "sip", "yuanbao"],
      channels: {},
    });
  });

  it("orders Rust canonical builtin channel ids before custom channels", async () => {
    listChannelsMock.mockResolvedValueOnce({
      console: { enabled: true, isBuiltin: true },
      wechat: { enabled: false, isBuiltin: true },
      yuanbao: { enabled: false, isBuiltin: true },
      mattermost: { enabled: false, isBuiltin: true },
      sip: { enabled: false, isBuiltin: true },
      custom_sms: { enabled: true },
    });
    listChannelTypesMock.mockResolvedValueOnce([
      "custom_sms",
      "yuanbao",
      "wechat",
      "sip",
      "console",
      "mattermost",
    ]);

    const { result } = renderHook(() => useChannels());

    await waitFor(() => expect(result.current.loading).toBe(false));

    expect(result.current.orderedKeys).toEqual([
      "console",
      "wechat",
      "mattermost",
      "sip",
      "yuanbao",
      "custom_sms",
    ]);
    expect(result.current.isBuiltin("wechat")).toBe(true);
    expect(result.current.isBuiltin("custom_sms")).toBe(false);
  });

  it("keeps legacy weixin as an unadvertised custom-style alias when returned", async () => {
    listChannelsMock.mockResolvedValueOnce({
      weixin: { enabled: false, isBuiltin: true },
    });
    listChannelTypesMock.mockResolvedValueOnce(["weixin", "console"]);

    const { result } = renderHook(() => useChannels());

    await waitFor(() => expect(result.current.loading).toBe(false));

    expect(result.current.orderedKeys).toEqual(["console", "weixin"]);
    expect(result.current.orderedKeys).not.toContain("wechat");
    expect(result.current.isBuiltin("weixin")).toBe(true);
  });
});
