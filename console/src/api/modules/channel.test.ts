import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("../request", () => ({ request: vi.fn() }));

import { request } from "../request";
import { channelApi } from "./channel";

describe("channelApi Rust route canaries", () => {
  afterEach(() => vi.clearAllMocks());

  it("uses Rust channel config routes for type discovery and full replacement", async () => {
    const body = { console: { enabled: true } } as Parameters<
      typeof channelApi.updateChannels
    >[0];

    await channelApi.listChannelTypes();
    await channelApi.listChannelSchemas();
    await channelApi.listChannels();
    await channelApi.updateChannels(body);

    expect(request).toHaveBeenNthCalledWith(1, "/config/channels/types");
    expect(request).toHaveBeenNthCalledWith(2, "/config/channels/schemas");
    expect(request).toHaveBeenNthCalledWith(3, "/config/channels");
    expect(request).toHaveBeenNthCalledWith(4, "/config/channels", {
      method: "PUT",
      body: JSON.stringify(body),
    });
  });

  it("encodes channel ids for single-channel config and QR routes", async () => {
    await channelApi.getChannelConfig("wecom/aibot");
    await channelApi.updateChannelConfig("wecom/aibot", { enabled: false });
    await channelApi.getChannelQrcode("wechat");
    await channelApi.getChannelQrcodeStatus("wechat", "token/with space");

    expect(request).toHaveBeenNthCalledWith(
      1,
      "/config/channels/wecom%2Faibot",
    );
    expect(request).toHaveBeenNthCalledWith(
      2,
      "/config/channels/wecom%2Faibot",
      {
        method: "PUT",
        body: JSON.stringify({ enabled: false }),
      },
    );
    expect(request).toHaveBeenNthCalledWith(
      3,
      "/config/channels/wechat/qrcode",
    );
    expect(request).toHaveBeenNthCalledWith(
      4,
      "/config/channels/wechat/qrcode/status?token=token%2Fwith+space",
    );
  });

  it("passes QR code query parameters for domain-specific auth flows", async () => {
    await channelApi.getChannelQrcode("feishu", { domain: "lark" });
    await channelApi.getChannelQrcodeStatus("feishu", "dev-code", {
      domain: "lark",
    });

    expect(request).toHaveBeenNthCalledWith(
      1,
      "/config/channels/feishu/qrcode?domain=lark",
    );
    expect(request).toHaveBeenNthCalledWith(
      2,
      "/config/channels/feishu/qrcode/status?domain=lark&token=dev-code",
    );
  });
});
