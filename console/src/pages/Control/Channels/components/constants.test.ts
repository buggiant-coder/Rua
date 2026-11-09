import { describe, expect, it, vi } from "vitest";
import { CHANNEL_LABELS, getChannelLabel } from "./constants";

describe("channel control constants", () => {
  it("keeps Rust canonical channel labels while preserving legacy aliases", () => {
    expect(CHANNEL_LABELS.wechat).toBe("WeChat");
    expect(CHANNEL_LABELS.weixin).toBe("WeChat");
    expect(CHANNEL_LABELS.sip).toBe("SIP");
    expect(CHANNEL_LABELS.yuanbao).toBe("Yuanbao");
  });

  it("uses localized labels with English defaults and formats custom ids", () => {
    const t = vi.fn((key: string, options?: { defaultValue?: string }) => {
      if (key === "channels.channelNames.wechat") return "微信";
      return options?.defaultValue ?? key;
    });

    expect(getChannelLabel("wechat", t as never)).toBe("微信");
    expect(getChannelLabel("yuanbao", t as never)).toBe("Yuanbao");
    expect(getChannelLabel("custom_sms", t as never)).toBe("Custom Sms");
    expect(t).toHaveBeenCalledWith("channels.channelNames.wechat", {
      defaultValue: "WeChat",
    });
  });
});
