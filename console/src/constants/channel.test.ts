import { describe, expect, it } from "vitest";

import { CHANNELS, CHANNEL_COLORS, mapChannelTagColor } from "./channel";

describe("CHANNELS", () => {
  it("keeps canonical Rust channel ids as key-matching strings", () => {
    expect(CHANNELS.wechat).toBe("wechat");
    expect(CHANNELS.yuanbao).toBe("yuanbao");
    expect(CHANNELS.dingtalk).toBe("dingtalk");
    expect(CHANNELS.console).toBe("console");

    for (const [key, value] of Object.entries(CHANNELS)) {
      expect(value).toBe(key);
    }
  });

  it("has tag colors for every canonical channel plus the legacy weixin alias", () => {
    for (const key of Object.keys(CHANNELS)) {
      expect(CHANNEL_COLORS[key]).toBeTruthy();
    }

    expect(CHANNEL_COLORS.wechat).toBe("green");
    expect(CHANNEL_COLORS.yuanbao).toBe("geekblue");
    expect(CHANNEL_COLORS.weixin).toBe("green");
    expect(CHANNELS).not.toHaveProperty("weixin");
  });
});

describe("mapChannelTagColor", () => {
  it.each([
    ["green", "success"],
    ["blue", "primary"],
    ["geekblue", "primary"],
    ["cyan", "primary"],
    ["purple", "primary"],
    ["red", "danger"],
    ["gold", "warning"],
    ["orange", "warning"],
    ["volcano", "warning"],
    ["olive", "warning"],
    [undefined, "default"],
    ["unknown", "default"],
  ] as const)(
    "maps Ant Design color %s to DS tag color %s",
    (input, output) => {
      expect(mapChannelTagColor(input)).toBe(output);
    },
  );
});
