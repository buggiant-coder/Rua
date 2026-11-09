import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { CHANNEL_DEFAULT_ICON_URL, getChannelIconUrl } from "./channelIcons";
import { ChannelCard } from "./ChannelCard";

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string, options?: { defaultValue?: string }) => {
      const labels: Record<string, string> = {
        "common.enabled": "Enabled",
        "common.disabled": "Disabled",
        "channels.botPrefix": "Bot prefix",
        "channels.notSet": "Not set",
      };
      if (key === "channels.channelNames.wechat") return "WeChat";
      if (key === "channels.channelNames.yuanbao") return "Yuanbao";
      return labels[key] ?? options?.defaultValue ?? key;
    },
  }),
}));

describe("ChannelCard", () => {
  it("renders canonical Rust channel labels, status, prefix, and click handlers", () => {
    const onClick = vi.fn();
    const onMouseEnter = vi.fn();
    const onMouseLeave = vi.fn();

    render(
      <ChannelCard
        channelKey="wechat"
        config={{ enabled: true, bot_prefix: "/rua" }}
        isHover={false}
        onClick={onClick}
        onMouseEnter={onMouseEnter}
        onMouseLeave={onMouseLeave}
      />,
    );

    expect(screen.getByText("WeChat")).toBeVisible();
    expect(screen.getByText("Enabled")).toBeVisible();
    expect(screen.getByText("Bot prefix: /rua")).toBeVisible();
    expect(screen.getByRole("img", { name: "wechat" })).toHaveAttribute(
      "src",
      getChannelIconUrl("wechat"),
    );

    fireEvent.mouseEnter(screen.getByText("WeChat"));
    fireEvent.mouseLeave(screen.getByText("WeChat"));
    fireEvent.click(screen.getByText("WeChat"));
    expect(onMouseEnter).toHaveBeenCalledTimes(1);
    expect(onMouseLeave).toHaveBeenCalledTimes(1);
    expect(onClick).toHaveBeenCalledTimes(1);
  });

  it("renders unknown custom channels with fallback labels and icons", () => {
    render(
      <ChannelCard
        channelKey="custom_sms"
        config={{ enabled: false }}
        isHover={false}
        onClick={vi.fn()}
        onMouseEnter={vi.fn()}
        onMouseLeave={vi.fn()}
      />,
    );

    expect(screen.getByText("Custom Sms")).toBeVisible();
    expect(screen.getByText("Disabled")).toBeVisible();
    expect(screen.getByText("Bot prefix: Not set")).toBeVisible();
    expect(screen.getByRole("img", { name: "custom_sms" })).toHaveAttribute(
      "src",
      CHANNEL_DEFAULT_ICON_URL,
    );
  });
});
