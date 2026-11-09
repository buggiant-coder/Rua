import { describe, it, expect } from "vitest";
import type { ChannelSchema } from "../../../../api/types/channel";
import { channelFormValues } from "./channelForm";

describe("channel form config values", () => {
  it("applies defaults without resetting credentials, extension objects, or explicit display flags", () => {
    const schema = {
      id: "telegram",
      defaults: {
        bot_token: "",
        show_thinking: true,
        show_tool_calls: true,
        show_tool_results: true,
        base_url: "",
      },
    } as ChannelSchema;
    const values = channelFormValues(schema, {
      bot_token: "***",
      show_tool_results: false,
      show_thinking: false,
      show_tool_calls: true,
      vendor: { enabled: true },
    });
    expect(values).toMatchObject({
      bot_token: "***",
      show_thinking: false,
      show_tool_calls: true,
      show_tool_results: false,
      base_url: "",
      vendor: { enabled: true },
    });
  });

  it("preserves canonical credentials and never silently rewrites unknown fields", () => {
    expect(
      channelFormValues(undefined, {
        access_key: "app",
        secret_key: "***",
        host: "new",
        broker_url: "old",
      }),
    ).toEqual({
      access_key: "app",
      secret_key: "***",
      host: "new",
      broker_url: "old",
    });
  });
});
