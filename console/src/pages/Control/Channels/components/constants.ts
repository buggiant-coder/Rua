import type { TFunction } from "i18next";

export type ChannelKey = string;

export const CHANNEL_LABELS: Record<string, string> = {
  imessage: "iMessage",
  discord: "Discord",
  dingtalk: "DingTalk",
  feishu: "Feishu",
  qq: "QQ",
  onebot: "OneBot",
  telegram: "Telegram",
  mqtt: "MQTT",
  mattermost: "Mattermost",
  slack: "Slack",
  sip: "SIP",
  matrix: "Matrix",
  console: "Console",
  voice: "Twilio",
  wecom: "WeCom",
  xiaoyi: "XiaoYi",
  wechat: "WeChat",
  yuanbao: "Yuanbao",

  weixin: "WeChat",
};

function formatCustomChannelKey(key: string): string {
  return key
    .split(/[_-]/)
    .map((word) => word.charAt(0).toUpperCase() + word.slice(1))
    .join(" ");
}

export function getChannelLabel(key: string, t?: TFunction): string {
  const english = CHANNEL_LABELS[key] ?? formatCustomChannelKey(key);
  if (t) {
    return t(`channels.channelNames.${key}`, { defaultValue: english });
  }
  return english;
}
