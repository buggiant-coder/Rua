import { addressFromChatPath } from "@/pages/Chat/conversationAddress";
const LAST_CHAT_ROUTE_KEY = "rua_last_chat_route";

export function rememberChatRoute(pathname: string): void {
  if (!addressFromChatPath(pathname)) return;
  try {
    localStorage.setItem(
      LAST_CHAT_ROUTE_KEY,
      JSON.stringify({ path: pathname }),
    );
  } catch {}
}

export function lastChatPath(): string | null {
  try {
    const value: unknown = JSON.parse(
      localStorage.getItem(LAST_CHAT_ROUTE_KEY) ?? "null",
    );
    if (
      value &&
      typeof value === "object" &&
      "path" in value &&
      typeof value.path === "string" &&
      addressFromChatPath(value.path)
    )
      return value.path;
  } catch {}
  return null;
}
