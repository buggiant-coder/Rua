import { describe, expect, it } from "vitest";

import {
  chatAddressFromRuaUri,
  chatAddressFromConsolePath,
  shouldPresentChatNotification,
} from "./chatViewer";

describe("chat notification viewer suppression", () => {
  const request = {
    target_device_id: "device-a",
    sender: { id: "agent-a", name: "Agent A" },
    title: "Reply complete",
    rua_uri: "rua://agents/agent-a/chats/chat-42",
  };

  it("suppresses a completed reply while this visible window views the chat", () => {
    expect(
      shouldPresentChatNotification(request, {
        currentPathname: "/console/agents/agent-a/chats/chat-42",
        windowActive: true,
        remotelyViewedChats: new Set(),
      }),
    ).toBe(false);
  });

  it("presents a completed reply when this window views another page", () => {
    expect(
      shouldPresentChatNotification(request, {
        currentPathname: "/console/agents/agent-a/chats/another-chat",
        windowActive: true,
        remotelyViewedChats: new Set(),
      }),
    ).toBe(true);
  });

  it("presents a completed reply when the matching chat window is not active", () => {
    expect(
      shouldPresentChatNotification(request, {
        currentPathname: "/console/agents/agent-a/chats/chat-42",
        windowActive: false,
        remotelyViewedChats: new Set(),
      }),
    ).toBe(true);
  });

  it("suppresses a completed reply while another local window views the chat", () => {
    expect(
      shouldPresentChatNotification(request, {
        currentPathname: "/console/settings",
        windowActive: true,
        remotelyViewedChats: new Set(["agents/agent-a/chats/chat-42"]),
      }),
    ).toBe(false);
  });

  it("normalizes supported chat addresses from URI and console routes", () => {
    expect(chatAddressFromRuaUri(request.rua_uri)).toBe(
      "agents/agent-a/chats/chat-42",
    );
    expect(
      chatAddressFromConsolePath("/console/projects/project-a/chats/chat-7"),
    ).toBe("projects/project-a/chats/chat-7");
  });
});
