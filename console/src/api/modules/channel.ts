import { request } from "../request";
import type {
  ChannelConfig,
  ChannelSchemasResponse,
  SingleChannelConfig,
} from "../types";

function qrcodeQuery(params?: Record<string, string>): string {
  const search = new URLSearchParams();
  for (const [key, value] of Object.entries(params ?? {})) {
    if (value) search.set(key, value);
  }
  const query = search.toString();
  return query ? `?${query}` : "";
}

export const channelApi = {
  listChannelTypes: () => request<string[]>("/config/channels/types"),

  listChannelSchemas: () =>
    request<ChannelSchemasResponse>("/config/channels/schemas"),

  listChannels: () => request<ChannelConfig>("/config/channels"),

  updateChannels: (body: ChannelConfig) =>
    request<ChannelConfig>("/config/channels", {
      method: "PUT",
      body: JSON.stringify(body),
    }),

  getChannelConfig: (channelName: string) =>
    request<SingleChannelConfig>(
      `/config/channels/${encodeURIComponent(channelName)}`,
    ),

  updateChannelConfig: (channelName: string, body: Record<string, unknown>) =>
    request<SingleChannelConfig>(
      `/config/channels/${encodeURIComponent(channelName)}`,
      {
        method: "PUT",
        body: JSON.stringify(body),
      },
    ),

  reapplyChannel: (channelName: string) =>
    request<{ channel: string; status: string; detail: string }>(
      `/config/channels/${encodeURIComponent(channelName)}/restart`,
      { method: "POST" },
    ),

  getChannelQrcode: (channel: string, params?: Record<string, string>) =>
    request<{ qrcode_img: string; poll_token: string }>(
      `/config/channels/${encodeURIComponent(channel)}/qrcode${qrcodeQuery(
        params,
      )}`,
    ),

  getChannelQrcodeStatus: (
    channel: string,
    token: string,
    params?: Record<string, string>,
  ) => {
    const search = new URLSearchParams(qrcodeQuery(params).slice(1));
    search.set("token", token);
    return request<{
      status: string;
      credentials: Record<string, string>;
    }>(
      `/config/channels/${encodeURIComponent(
        channel,
      )}/qrcode/status?${search.toString()}`,
    );
  },
};
