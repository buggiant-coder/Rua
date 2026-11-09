import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("./routeAuth", () => ({
  requireRouteAccess: vi.fn(),
}));

import { useAgentStore } from "@/stores/agentStore";
import { landingLoader } from "./landingRoute";

describe("landingRoute", () => {
  beforeEach(() => {
    localStorage.clear();
    useAgentStore.setState({ selectedAgent: "default" });
    vi.restoreAllMocks();
  });

  it("redirects to the selected Agent's complete last Chat address", async () => {
    useAgentStore.setState({ selectedAgent: "agent-b" });
    localStorage.setItem("rua_last_chat_id_agent-b", "chat-2");

    const response = await landingLoader({
      request: new Request("http://localhost/"),
      params: {},
      context: {},
    });

    expect(response.headers.get("Location")).toBe(
      "/agents/agent-b/chats/chat-2",
    );
  });

  it("redirects to the last Project Chat when it was opened most recently", async () => {
    localStorage.setItem(
      "rua_last_chat_route",
      JSON.stringify({
        version: 1,
        kind: "project-chat",
        path: "/projects/project-1/chats/chat-2",
      }),
    );

    const response = await landingLoader({
      request: new Request("http://localhost/"),
      params: {},
      context: {},
    });

    expect(response.headers.get("Location")).toBe(
      "/projects/project-1/chats/chat-2",
    );
  });

  it("reserves a Lazy Chat when the selected Agent has no navigation hint", async () => {
    vi.spyOn(crypto, "randomUUID").mockReturnValue("lazy-chat");

    const response = await landingLoader({
      request: new Request("http://localhost/"),
      params: {},
      context: {},
    });

    expect(response.headers.get("Location")).toBe(
      "/agents/default/chats/lazy-chat",
    );
  });
});
