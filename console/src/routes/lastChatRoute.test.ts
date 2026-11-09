import { beforeEach, describe, expect, it } from "vitest";
import { lastProjectChatPath, rememberChatRoute } from "./lastChatRoute";

describe("lastChatRoute", () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it("restores the last Project Chat path", () => {
    rememberChatRoute("/projects/project-1/chats/chat-2");

    expect(lastProjectChatPath()).toBe("/projects/project-1/chats/chat-2");
  });

  it("stops restoring Project Chat after a regular Chat is opened", () => {
    rememberChatRoute("/projects/project-1/chats/chat-2");
    rememberChatRoute("/agents/default/chats/chat-3");

    expect(lastProjectChatPath()).toBeNull();
  });

  it("ignores routes outside Chat and Project Chat", () => {
    rememberChatRoute("/projects/project-1/chats/chat-2");
    rememberChatRoute("/settings");

    expect(lastProjectChatPath()).toBe("/projects/project-1/chats/chat-2");
  });

  it("ignores malformed stored routes", () => {
    localStorage.setItem(
      "rua_last_chat_route",
      JSON.stringify({ version: 1, kind: "project-chat", path: "/settings" }),
    );

    expect(lastProjectChatPath()).toBeNull();
  });
});
