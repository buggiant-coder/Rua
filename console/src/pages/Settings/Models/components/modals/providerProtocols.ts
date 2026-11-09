import type { TFunction } from "i18next";

export const PROVIDER_PROTOCOLS = [
  {
    value: "OpenAIChatModel",
    labelKey: "models.protocolOpenAI",
  },
  {
    value: "OpenAIResponseModel",
    labelKey: "models.protocolOpenAIResponses",
  },
  {
    value: "AnthropicChatModel",
    labelKey: "models.protocolAnthropic",
  },
  {
    value: "GeminiChatModel",
    labelKey: "models.protocolGemini",
  },
] as const;

export function providerProtocolOptions(t: TFunction) {
  return PROVIDER_PROTOCOLS.map(({ value, labelKey }) => ({
    value,
    label: t(labelKey),
  }));
}
