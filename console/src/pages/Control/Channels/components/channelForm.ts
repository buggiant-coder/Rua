import type { ChannelSchema } from "../../../../api/types/channel";

export function channelFormValues(
  schema: ChannelSchema | undefined,
  saved: Record<string, unknown> = {},
): Record<string, unknown> {
  const values = { ...schema?.defaults, ...saved };
  if (schema?.id === "console") values.enabled = true;
  return values;
}
