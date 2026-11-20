import { beforeEach, describe, expect, it } from "vitest";
import { lastChatPath, rememberChatRoute } from "./lastChatRoute";

describe("lastChatRoute", () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it("restores the last Project Chat path", () => {
    rememberChatRoute("/chats/chat-2");

    expect(lastChatPath()).toBe("/chats/chat-2");
  });

  it("restores the most recently opened Chat", () => {
    rememberChatRoute("/chats/chat-2");
    rememberChatRoute("/chats/chat-3");

    expect(lastChatPath()).toBe("/chats/chat-3");
  });

  it("ignores routes outside Chat and Project Chat", () => {
    rememberChatRoute("/chats/chat-2");
    rememberChatRoute("/settings");

    expect(lastChatPath()).toBe("/chats/chat-2");
  });

  it("ignores malformed stored routes", () => {
    localStorage.setItem(
      "rua_last_chat_route",
      JSON.stringify({ version: 1, kind: "project-chat", path: "/settings" }),
    );

    expect(lastChatPath()).toBeNull();
  });
});
