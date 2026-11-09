export const CHANNELS = {
  imessage: "imessage",
  discord: "discord",
  dingtalk: "dingtalk",
  feishu: "feishu",
  qq: "qq",
  telegram: "telegram",
  mattermost: "mattermost",
  mqtt: "mqtt",
  console: "console",
  matrix: "matrix",
  voice: "voice",
  wechat: "wechat",
  wecom: "wecom",
  xiaoyi: "xiaoyi",
  yuanbao: "yuanbao",
};

export function mapChannelTagColor(
  antdColor?: string,
): "primary" | "danger" | "success" | "warning" | "default" {
  switch (antdColor) {
    case "green":
      return "success";
    case "blue":
    case "geekblue":
    case "cyan":
    case "purple":
      return "primary";
    case "red":
      return "danger";
    case "gold":
    case "orange":
    case "volcano":
    case "olive":
      return "warning";
    default:
      return "default";
  }
}

export const CHANNEL_COLORS: Record<string, string> = {
  imessage: "geekblue",
  discord: "blue",
  dingtalk: "green",
  feishu: "volcano",
  qq: "gold",
  telegram: "geekblue",
  mattermost: "purple",
  mqtt: "orange",
  console: "green",
  matrix: "red",
  voice: "geekblue",
  wechat: "green",
  wecom: "olive",

  weixin: "green",
  xiaoyi: "cyan",
  yuanbao: "geekblue",
};
