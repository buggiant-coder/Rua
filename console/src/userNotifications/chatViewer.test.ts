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
    rua_uri: "rua://chats/chat-42",
  };

  it("suppresses a completed reply while this visible window views the chat", () => {
    expect(
      shouldPresentChatNotification(request, {
        currentPathname: "/console/chats/chat-42",
        windowActive: true,
        remotelyViewedChats: new Set(),
      }),
    ).toBe(false);
  });

  it("presents a completed reply when this window views another page", () => {
    expect(
      shouldPresentChatNotification(request, {
        currentPathname: "/console/chats/another-chat",
        windowActive: true,
        remotelyViewedChats: new Set(),
      }),
    ).toBe(true);
  });

  it("presents a completed reply when the matching chat window is not active", () => {
    expect(
      shouldPresentChatNotification(request, {
        currentPathname: "/console/chats/chat-42",
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
        remotelyViewedChats: new Set(["chats/chat-42"]),
      }),
    ).toBe(false);
  });

  it("normalizes supported chat addresses from URI and console routes", () => {
    expect(chatAddressFromRuaUri(request.rua_uri)).toBe("chats/chat-42");
    expect(chatAddressFromRuaUri("rua://chats/%63hat-42?turn=7#output")).toBe(
      "chats/chat-42",
    );
    expect(
      chatAddressFromRuaUri("rua://agents/first/chats/chat-42"),
    ).toBeNull();
    expect(chatAddressFromRuaUri("rua:/chats/chat-42")).toBeNull();
    expect(chatAddressFromConsolePath("/console/chats/chat-7")).toBe(
      "chats/chat-7",
    );
  });
});
