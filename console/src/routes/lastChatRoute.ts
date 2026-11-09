const LAST_CHAT_ROUTE_KEY = "rua_last_chat_route";

type LastChatRoute =
  | { version: 1; kind: "chat" }
  | { version: 1; kind: "project-chat"; path: string };

const CHAT_PATH = /^\/agents\/[^/]+\/chats\/[^/]+$/;
const PROJECT_CHAT_PATH = /^\/projects\/[^/]+\/chats\/[^/]+$/;

export function rememberChatRoute(pathname: string): void {
  let route: LastChatRoute;

  if (CHAT_PATH.test(pathname)) {
    route = { version: 1, kind: "chat" };
  } else if (PROJECT_CHAT_PATH.test(pathname)) {
    route = { version: 1, kind: "project-chat", path: pathname };
  } else {
    return;
  }

  try {
    localStorage.setItem(LAST_CHAT_ROUTE_KEY, JSON.stringify(route));
  } catch {}
}

export function lastProjectChatPath(): string | null {
  try {
    const value: unknown = JSON.parse(
      localStorage.getItem(LAST_CHAT_ROUTE_KEY) ?? "null",
    );
    if (
      typeof value === "object" &&
      value !== null &&
      "version" in value &&
      value.version === 1 &&
      "kind" in value &&
      value.kind === "project-chat" &&
      "path" in value &&
      typeof value.path === "string" &&
      PROJECT_CHAT_PATH.test(value.path)
    ) {
      return value.path;
    }
  } catch {}

  return null;
}
